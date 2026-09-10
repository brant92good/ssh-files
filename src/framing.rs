//! A size boundary, not a replacement SFTP decoder. Reject the length before
//! handing its header to a dependency which otherwise allocates that length.
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

pub const MAX_PACKET: usize = 262_144;
pub type Failure = Arc<Mutex<Option<String>>>;

pub struct CappedReader<R> {
    inner: R,
    header: [u8; 4],
    collected: usize,
    emitted: usize,
    remaining: usize,
    failed: bool,
    failure: Failure,
}

impl<R> CappedReader<R> {
    pub fn new(inner: R) -> (Self, Failure) {
        let failure = Arc::new(Mutex::new(None));
        (
            Self {
                inner,
                header: [0; 4],
                collected: 0,
                emitted: 0,
                remaining: 0,
                failed: false,
                failure: failure.clone(),
            },
            failure,
        )
    }
    fn fail(&mut self, reason: String) -> Poll<io::Result<()>> {
        self.failed = true;
        *self.failure.lock().expect("framing failure mutex") = Some(reason.clone());
        // russh-sftp breaks its background reader on UnexpectedEof. Returning
        // InvalidData repeatedly would make that reader log and poll forever.
        Poll::Ready(Err(io::Error::new(io::ErrorKind::UnexpectedEof, reason)))
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for CappedReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.failed || output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.remaining == 0 && this.emitted == 4 {
            this.collected = 0;
            this.emitted = 0;
        }
        while this.collected < 4 {
            let mut header = ReadBuf::new(&mut this.header[this.collected..]);
            match Pin::new(&mut this.inner).poll_read(cx, &mut header) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => {
                    return this.fail(format!("SFTP transport read failed: {error}"));
                }
                Poll::Ready(Ok(())) => {
                    let count = header.filled().len();
                    if count == 0 {
                        if this.collected == 0 {
                            return Poll::Ready(Ok(()));
                        }
                        return this.fail("SFTP frame ended inside its length header".into());
                    }
                    this.collected += count;
                }
            }
        }
        if this.emitted == 0 {
            let length = u32::from_be_bytes(this.header) as usize;
            if !(5..=MAX_PACKET).contains(&length) {
                return this.fail(format!(
                    "Rejected SFTP packet length {length}; allowed 5..={MAX_PACKET}"
                ));
            }
            this.remaining = length;
        }
        if this.emitted < 4 {
            let count = (4 - this.emitted).min(output.remaining());
            output.put_slice(&this.header[this.emitted..this.emitted + count]);
            this.emitted += count;
            return Poll::Ready(Ok(()));
        }
        let limit = this.remaining.min(output.remaining());
        let mut body = ReadBuf::new(output.initialize_unfilled_to(limit));
        match Pin::new(&mut this.inner).poll_read(cx, &mut body) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => this.fail(format!("SFTP frame read failed: {error}")),
            Poll::Ready(Ok(())) => {
                let count = body.filled().len();
                if count == 0 {
                    return this.fail("SFTP frame ended before its declared length".into());
                }
                this.remaining -= count;
                output.advance(count);
                Poll::Ready(Ok(()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn rejects_header_before_exposing_or_allocating_body() {
        for length in [0, 1, 4, MAX_PACKET as u32 + 1, u32::MAX] {
            let raw = length.to_be_bytes();
            let (mut reader, failure) = CappedReader::new(raw.as_slice());
            let mut buffer = [0_u8; 16];
            assert_eq!(
                reader.read(&mut buffer).await.unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
            assert_eq!(buffer, [0; 16]);
            assert!(
                failure
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .contains("Rejected")
            );
            assert_eq!(reader.read(&mut buffer).await.unwrap(), 0);
        }
    }

    #[tokio::test]
    async fn preserves_fragmented_headers_payloads_and_multiple_frames() {
        let mut expected = vec![];
        for body in [vec![1; 5], vec![2; MAX_PACKET], vec![3; 19]] {
            expected.extend((body.len() as u32).to_be_bytes());
            expected.extend(body);
        }
        let (mut writer, input) = tokio::io::duplex(7);
        let bytes = expected.clone();
        let task = tokio::spawn(async move {
            for chunk in bytes.chunks(3) {
                writer.write_all(chunk).await.unwrap();
            }
        });
        let (mut reader, failure) = CappedReader::new(input);
        let mut actual = vec![];
        reader.read_to_end(&mut actual).await.unwrap();
        task.await.unwrap();
        assert_eq!(actual, expected);
        assert!(failure.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn truncated_header_and_payload_fail_closed() {
        for bytes in [vec![0, 0], vec![0, 0, 0, 5, 2, 0]] {
            let (mut reader, failure) = CappedReader::new(bytes.as_slice());
            assert!(reader.read_to_end(&mut vec![]).await.is_err());
            assert!(failure.lock().unwrap().is_some());
        }
    }
}
