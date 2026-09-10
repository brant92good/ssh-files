use std::{
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

pub struct Gate {
    pub port: u16,
    pub pause_transfer: Arc<AtomicBool>,
    pub active: Arc<AtomicUsize>,
    pub cancelling: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
}
impl Gate {
    pub fn start(target: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let cancelling = Arc::new(AtomicBool::new(false));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let (cancel, failures) = (cancelling.clone(), errors.clone());
        let (stopping, pause, count) = (stop.clone(), paused.clone(), active.clone());
        let task = thread::spawn(move || {
            let mut workers = Vec::new();
            let mut accepted = 0;
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((client, _)) => {
                        accepted += 1;
                        assert!(accepted <= 8, "Fixture transport unexpectedly reconnects");
                        let remote = TcpStream::connect(("127.0.0.1", target)).unwrap();
                        for stream in [&client, &remote] {
                            // Windows accepted sockets inherit listener mode.
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_millis(20)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_millis(200)))
                                .unwrap();
                        }
                        let transfer = accepted > 1;
                        let (stop, pause, count) = (stopping.clone(), pause.clone(), count.clone());
                        let (cancel, failures) = (cancel.clone(), failures.clone());
                        count.fetch_add(1, Ordering::AcqRel);
                        workers.push(thread::spawn(move || {
                            let mut client = client;
                            let mut remote = remote;
                            let mut data = [0; 16384];
                            let failure = |error: std::io::Error| {
                                let expected = (cancel.load(Ordering::Acquire)
                                    || stop.load(Ordering::Acquire))
                                    && matches!(
                                        error.kind(),
                                        std::io::ErrorKind::ConnectionReset
                                            | std::io::ErrorKind::ConnectionAborted
                                            | std::io::ErrorKind::BrokenPipe
                                    );
                                if !expected {
                                    failures
                                        .lock()
                                        .unwrap()
                                        .push(format!("relay error: {error}"));
                                }
                            };
                            while !stop.load(Ordering::Acquire) {
                                if transfer && pause.load(Ordering::Acquire) {
                                    thread::sleep(Duration::from_millis(5));
                                    continue;
                                }
                                let mut alive = true;
                                for direction in 0..2 {
                                    let (source, destination) = if direction == 0 {
                                        (&mut client, &mut remote)
                                    } else {
                                        (&mut remote, &mut client)
                                    };
                                    match source.read(&mut data) {
                                        Ok(0) => {
                                            alive = false;
                                            break;
                                        }
                                        Ok(size) => {
                                            if let Err(error) = destination.write_all(&data[..size])
                                            {
                                                failure(error);
                                                alive = false;
                                                break;
                                            }
                                        }
                                        Err(error)
                                            if matches!(
                                                error.kind(),
                                                std::io::ErrorKind::WouldBlock
                                                    | std::io::ErrorKind::TimedOut
                                            ) => {}
                                        Err(error) => {
                                            failure(error);
                                            alive = false;
                                            break;
                                        }
                                    }
                                }
                                if !alive {
                                    break;
                                }
                            }
                            let _ = client.shutdown(Shutdown::Both);
                            let _ = remote.shutdown(Shutdown::Both);
                            count.fetch_sub(1, Ordering::AcqRel);
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("Owned gate accept: {error}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            port,
            pause_transfer: paused,
            active,
            cancelling,
            errors,
            stop,
            task: Some(task),
        }
    }
    pub fn assert_clean(&self) {
        assert!(
            self.errors.lock().unwrap().is_empty(),
            "Unexpected fixture relay failures: {:?}",
            self.errors.lock().unwrap()
        );
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.pause_transfer.store(false, Ordering::Release);
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
        if !thread::panicking() {
            self.assert_clean();
        }
    }
}
