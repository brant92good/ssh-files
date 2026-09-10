//! Process, framing and bounded diagnostics shared by browsing and transfers.
use crate::{Route, framing, process};
use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use tokio::{io::AsyncReadExt, task::JoinHandle};

pub(crate) type Pipe =
    tokio::io::Join<framing::CappedReader<tokio::process::ChildStdout>, tokio::process::ChildStdin>;

pub(crate) struct Connection {
    owned: process::OwnedChild,
    stderr: JoinHandle<std::io::Result<Vec<u8>>>,
    framing_failure: framing::Failure,
    _registry: process::Registry,
}

impl Connection {
    pub(crate) fn spawn(
        route: &Route,
        registry: &process::Registry,
        role: process::Role,
    ) -> Result<(Self, Pipe)> {
        let mut owned = process::OwnedChild::spawn(&mut route.command()?)?;
        owned.register(registry, role)?;
        let stdin = tokio::process::ChildStdin::from_std(
            owned.child.stdin.take().context("Missing SSH stdin")?,
        )?;
        let stdout = tokio::process::ChildStdout::from_std(
            owned.child.stdout.take().context("Missing SSH stdout")?,
        )?;
        let mut stderr = tokio::process::ChildStderr::from_std(
            owned.child.stderr.take().context("Missing SSH stderr")?,
        )?;
        let drain = tokio::spawn(async move {
            let mut saved = Vec::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let count = stderr.read(&mut buffer).await?;
                if count == 0 {
                    return Ok(saved);
                }
                let retain = count.min((64 * 1024_usize).saturating_sub(saved.len()));
                saved.extend_from_slice(&buffer[..retain]);
            }
        });
        let (stdout, framing_failure) = framing::CappedReader::new(stdout);
        Ok((
            Self {
                owned,
                stderr: drain,
                framing_failure,
                _registry: registry.clone(),
            },
            tokio::io::join(stdout, stdin),
        ))
    }

    pub(crate) async fn shutdown(self) -> Result<String> {
        self.shutdown_until(Instant::now() + Duration::from_secs(2))
            .await
    }

    pub(crate) async fn shutdown_until(mut self, deadline: Instant) -> Result<String> {
        self.owned.stop_until(deadline)?;
        let output = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            &mut self.stderr,
        )
        .await
        .context("SSH stderr did not close within cleanup deadline")???;
        if let Some(failure) = self.framing_failure.lock().unwrap().as_ref() {
            anyhow::bail!("{failure}");
        }
        Ok(String::from_utf8_lossy(&output).into_owned())
    }
}

pub(crate) fn config() -> russh_sftp::client::Config {
    russh_sftp::client::Config {
        max_packet_len: framing::MAX_PACKET as u32,
        max_concurrent_writes: 2,
        request_timeout_secs: 15,
    }
}
