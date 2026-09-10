use crate::{
    Outcome, Route, Transport,
    browser::{Browser, Listing},
    paths,
    plan::{self, Direction, Job},
};
use anyhow::{Context, Result, ensure};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{sync::watch, task::JoinHandle};

pub enum LocalAction {
    List(PathBuf),
    Plan(Vec<PathBuf>, String),
    Directory(PathBuf),
}
pub enum LocalValue {
    Listing(Listing),
    Plan(Vec<Job>),
    Directory,
}
pub struct LocalJob {
    pub generation: u64,
    pub started: std::time::Instant,
    pub cancel: Arc<AtomicBool>,
    pub task: JoinHandle<Result<LocalValue>>,
}

impl LocalJob {
    pub fn start(generation: u64, action: LocalAction) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let task = tokio::task::spawn_blocking(move || {
            ensure!(!flag.load(Ordering::Acquire), "Local operation cancelled");
            match action {
                LocalAction::List(path) => Ok(LocalValue::Listing(crate::browser::list_local(
                    &path, &flag,
                )?)),
                LocalAction::Plan(paths, remote) => {
                    Ok(LocalValue::Plan(plan::upload(&paths, &remote, &flag)?))
                }
                LocalAction::Directory(path) => {
                    let path = paths::checked_local(&path, true)?;
                    match std::fs::symlink_metadata(&path) {
                        Ok(metadata) => ensure!(
                            metadata.is_dir() && !paths::is_link(&metadata),
                            "Destination exists and is not a regular directory"
                        ),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            #[allow(unused_mut)]
                            let mut builder = std::fs::DirBuilder::new();
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::DirBuilderExt;
                                builder.mode(0o700);
                            }
                            builder.create(path)?;
                        }
                        Err(error) => return Err(error.into()),
                    }
                    Ok(LocalValue::Directory)
                }
            }
        });
        Self {
            generation,
            started: std::time::Instant::now(),
            cancel,
            task,
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

pub enum RemoteAction {
    List(String),
    Plan(Vec<String>, PathBuf),
    Directory(String),
}
pub enum RemoteValue {
    Listing(Listing),
    Plan(Vec<Job>),
    Directory,
}
pub struct RemoteJob {
    pub generation: u64,
    registry: crate::ProcessRegistry,
    pub cancel: watch::Sender<bool>,
    pub task: JoinHandle<(Option<Browser>, Result<RemoteValue>)>,
}

async fn cancelled(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

impl RemoteJob {
    pub fn start(
        generation: u64,
        route: Route,
        registry: crate::ProcessRegistry,
        browser: Option<Browser>,
        action: RemoteAction,
    ) -> Self {
        let (cancel, mut receiver) = watch::channel(false);
        let connection_registry = registry.clone();
        let task = tokio::spawn(async move {
            let connected = async {
                match browser {
                    Some(browser) => Ok(browser),
                    None => Browser::connect_in(&route, &connection_registry).await,
                }
            };
            let browser = tokio::select! {
                _ = cancelled(&mut receiver) => return (None, Err(anyhow::anyhow!("Remote operation cancelled"))),
                result = connected => match result { Ok(value) => value, Err(error) => return (None, Err(error)) },
            };
            let result = tokio::select! {
                _ = cancelled(&mut receiver) => Err(anyhow::anyhow!("Remote operation cancelled")),
                result = async {
                    match action {
                        RemoteAction::List(path) => Ok(RemoteValue::Listing(browser.list(&path).await?)),
                        RemoteAction::Plan(sources, local) => Ok(RemoteValue::Plan(plan::download(&browser, &sources, &local).await?)),
                        RemoteAction::Directory(path) => { browser.create_directory(&path).await?; Ok(RemoteValue::Directory) },
                    }
                } => result,
            };
            if result.is_ok() {
                (Some(browser), result)
            } else {
                let cleanup = browser.shutdown().await;
                let result = match (result, cleanup) {
                    (Err(error), Err(cleanup)) => {
                        Err(error.context(format!("SSH browser cleanup: {cleanup:#}")))
                    }
                    (result, _) => result,
                };
                (None, result)
            }
        });
        Self {
            generation,
            registry,
            cancel,
            task,
        }
    }
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
        self.registry.cancel(crate::process::Role::Browser);
    }
}

pub struct TransferReport {
    pub transport: Option<Transport>,
    pub outcome: Outcome,
    pub result: Result<()>,
}
pub struct TransferJob {
    registry: crate::ProcessRegistry,
    pub cancel: watch::Sender<bool>,
    pub progress: watch::Receiver<Outcome>,
    pub task: JoinHandle<TransferReport>,
}

impl TransferJob {
    pub fn start(
        route: Route,
        registry: crate::ProcessRegistry,
        transport: Option<Transport>,
        job: Job,
    ) -> Self {
        let (cancel, mut receiver) = watch::channel(false);
        let (progress, observer) = watch::channel(Outcome {
            phase: "connecting",
            destination: Some(job.destination.clone()),
            ..Outcome::default()
        });
        let connection_registry = registry.clone();
        let task = tokio::spawn(async move {
            let connecting = async {
                match transport {
                    Some(value) => Ok(value),
                    None => Transport::connect_in(&route, &connection_registry).await,
                }
            };
            let mut transport = tokio::select! {
                _ = cancelled(&mut receiver) => return TransferReport { transport: None, outcome: Outcome::default(), result: Err(anyhow::anyhow!("Transfer cancelled before connection")) },
                result = connecting => match result { Ok(value) => value, Err(error) => return TransferReport { transport: None, outcome: Outcome::default(), result: Err(error) } },
            };
            transport.outcome = Outcome {
                phase: "preparing",
                destination: Some(job.destination.clone()),
                ..Outcome::default()
            };
            transport.observe_progress(progress);
            let result = tokio::select! {
                _ = cancelled(&mut receiver) => Err(anyhow::anyhow!("Transfer cancelled")),
                result = async {
                    let local = PathBuf::from(if job.direction == Direction::Upload { &job.source } else { &job.destination });
                    let missing = job.direction == Direction::Download;
                    let checked = tokio::task::spawn_blocking(move || paths::checked_local(&local, missing));
                    tokio::time::timeout(crate::REQUEST, crate::local_work(&mut transport.outcome, &transport.progress, checked))
                        .await.context("Local path inspection timed out; later local work is disabled")?
                        .context("Local path worker failed")??;
                    match job.direction {
                        Direction::Upload => transport.upload(std::path::Path::new(&job.source), &job.destination).await,
                        Direction::Download => transport.download(&job.source, std::path::Path::new(&job.destination)).await,
                    }
                } => result,
            };
            let outcome = transport.outcome.clone();
            if result.is_ok() {
                TransferReport {
                    transport: Some(transport),
                    outcome,
                    result,
                }
            } else {
                let cleanup = transport.shutdown().await;
                let result = match (result, cleanup) {
                    (Err(error), Err(cleanup)) => {
                        Err(error.context(format!("SSH cleanup: {cleanup:#}")))
                    }
                    (result, _) => result,
                };
                TransferReport {
                    transport: None,
                    outcome,
                    result,
                }
            }
        });
        Self {
            registry,
            cancel,
            progress: observer,
            task,
        }
    }
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
        self.registry.cancel(crate::process::Role::Transfer);
    }
}

#[derive(Default)]
pub struct LocalSafety {
    pub uncertain: bool,
}
impl LocalSafety {
    pub fn record(&mut self, outcome: &Outcome) {
        self.uncertain |= outcome.local_io_pending;
    }
    pub fn permits(&self, local: Option<&LocalJob>) -> bool {
        !self.uncertain && !local.is_some_and(|job| job.cancel.load(Ordering::Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_other_work_cannot_release_quarantined_local_actions() {
        let mut safety = LocalSafety::default();
        safety.record(&Outcome {
            local_io_pending: true,
            ..Outcome::default()
        });
        safety.record(&Outcome::default());
        assert!(!safety.permits(None));
        let mut network_only = LocalSafety::default();
        network_only.record(&Outcome::default());
        assert!(network_only.permits(None));
    }
    #[tokio::test]
    async fn cancelled_blocking_worker_retains_its_slot_until_real_completion() {
        let (send, receive) = std::sync::mpsc::channel();
        let job = LocalJob {
            generation: 1,
            started: std::time::Instant::now(),
            cancel: Arc::new(AtomicBool::new(false)),
            task: tokio::task::spawn_blocking(move || {
                receive.recv().unwrap();
                Ok(LocalValue::Directory)
            }),
        };
        job.cancel();
        assert!(!LocalSafety::default().permits(Some(&job)));
        assert!(!job.task.is_finished());
        send.send(()).unwrap();
        job.task.await.unwrap().unwrap();
    }
}
