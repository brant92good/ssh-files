pub mod browser;
mod connection;
pub mod framing;
pub mod paths;
pub mod plan;
mod pointer;
mod process;
pub use process::Registry as ProcessRegistry;
mod clipboard;
pub mod ui;
pub mod ui_model;
mod ui_render;
pub mod workers;

use anyhow::{Context, Result, bail, ensure};
use russh_sftp::{
    client::SftpSession,
    protocol::{FileAttributes, OpenFlags},
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const REQUEST: Duration = Duration::from_secs(15);
const IDLE: Duration = Duration::from_secs(30);
const CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct Route {
    pub host: String,
    pub hostname: Option<String>,
    pub config: Option<PathBuf>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

impl Route {
    pub fn command(&self) -> Result<Command> {
        for value in std::iter::once(self.host.as_str()).chain(self.hostname.as_deref()) {
            ensure!(
                !value.is_empty()
                    && !value.starts_with('-')
                    && !value.chars().any(|c| c.is_whitespace() || c.is_control()),
                "Invalid explicit SSH host or hostname override"
            );
        }
        let mut command = Command::new(which::which("ssh").context("OpenSSH ssh is required")?);
        command.args(["-T", "-s"]);
        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "ClearAllForwardings=yes",
            "PermitLocalCommand=no",
            "ForwardAgent=no",
            "ForwardX11=no",
            "RemoteCommand=none",
            "StdinNull=no",
            "ForkAfterAuthentication=no",
            "ControlMaster=no",
            "ControlPath=none",
            "ControlPersist=no",
            "ConnectTimeout=10",
            "ConnectionAttempts=1",
            "LogLevel=ERROR",
        ] {
            command.args(["-o", option]);
        }
        if let Some(config) = &self.config {
            ensure!(
                config.is_file(),
                "Selected SSH configuration does not exist"
            );
            command.arg("-F").arg(config);
        }
        if let Some(user) = &self.user {
            ensure!(
                !user.is_empty()
                    && !user.starts_with('-')
                    && !user.chars().any(|c| c.is_whitespace() || c.is_control()),
                "Invalid SSH username"
            );
            command.arg("-l").arg(user);
        }
        if let Some(port) = self.port {
            ensure!(port != 0, "SSH port must be positive");
            command.arg("-p").arg(port.to_string());
        }
        if let Some(hostname) = &self.hostname {
            command.arg("-o").arg(format!("HostName={hostname}"));
        }
        command.args(["--", &self.host, "sftp"]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(command)
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Outcome {
    pub phase: &'static str,
    pub partial: Option<String>,
    pub destination: Option<String>,
    pub bytes: u64,
    pub local_io_pending: bool,
}

pub struct Transport {
    session: Option<SftpSession>,
    connection: connection::Connection,
    pub outcome: Outcome,
    progress: Option<tokio::sync::watch::Sender<Outcome>>,
}

impl Transport {
    pub async fn connect(route: &Route) -> Result<Self> {
        Self::connect_in(route, &ProcessRegistry::default()).await
    }

    pub async fn connect_in(route: &Route, registry: &ProcessRegistry) -> Result<Self> {
        let (connection, stream) =
            connection::Connection::spawn(route, registry, process::Role::Transfer)?;
        let session = tokio::time::timeout(
            REQUEST,
            SftpSession::new_with_config(stream, connection::config()),
        )
        .await;
        match session {
            Ok(Ok(session)) => Ok(Self {
                session: Some(session),
                connection,
                progress: None,
                outcome: Outcome {
                    phase: "connected",
                    ..Outcome::default()
                },
            }),
            result => {
                let error = match result {
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => error.to_string(),
                    Ok(Ok(_)) => unreachable!(),
                };
                let cleanup = connection.shutdown().await;
                bail!("SFTP initialization failed ({error}); {cleanup:?}");
            }
        }
    }

    pub fn observe_progress(&mut self, sender: tokio::sync::watch::Sender<Outcome>) {
        self.progress = Some(sender);
        self.publish();
    }

    fn publish(&self) {
        if let Some(sender) = &self.progress {
            sender.send_replace(self.outcome.clone());
        }
    }

    fn set_phase(&mut self, phase: &'static str) {
        self.outcome.phase = phase;
        self.publish();
    }

    fn session(&self) -> &SftpSession {
        self.session.as_ref().expect("active SFTP session")
    }

    pub async fn metadata(&self, remote: &str) -> Result<FileAttributes> {
        valid_remote(remote)?;
        Ok(
            tokio::time::timeout(REQUEST, self.session().symlink_metadata(remote))
                .await
                .context("SFTP metadata timed out")??,
        )
    }

    pub async fn upload(&mut self, local: &Path, destination: &str) -> Result<()> {
        valid_remote(destination)?;
        ensure!(
            tokio::time::timeout(
                REQUEST,
                local_work(
                    &mut self.outcome,
                    &self.progress,
                    tokio::fs::symlink_metadata(local)
                )
            )
            .await
            .context("Local source metadata timed out")??
            .file_type()
            .is_file(),
            "Upload source must be a regular file, not a symlink"
        );
        let mut source_options = tokio::fs::OpenOptions::new();
        source_options.read(true);
        #[cfg(unix)]
        source_options.custom_flags(libc::O_NOFOLLOW);
        #[cfg(windows)]
        source_options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
        let mut source = tokio::time::timeout(
            REQUEST,
            local_work(
                &mut self.outcome,
                &self.progress,
                source_options.open(local),
            ),
        )
        .await
        .context("Local source open timed out")??;
        let metadata = tokio::time::timeout(
            REQUEST,
            local_work(&mut self.outcome, &self.progress, source.metadata()),
        )
        .await
        .context("Local source handle metadata timed out")??;
        ensure!(
            metadata.file_type().is_file(),
            "Opened upload source is not a regular file"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "Upload source is a reparse point"
            );
        }
        let partial = remote_partial(destination)?;
        self.outcome = Outcome {
            phase: "creating-partial",
            partial: Some(partial.clone()),
            destination: Some(destination.into()),
            bytes: 0,
            local_io_pending: false,
        };
        self.publish();
        let mut output = tokio::time::timeout(
            REQUEST,
            self.session().open_with_flags_and_attributes(
                &partial,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes {
                    permissions: Some(0o600),
                    ..FileAttributes::default()
                },
            ),
        )
        .await
        .context("SFTP exclusive create timed out")??;
        self.set_phase("transferring");
        let mut buffer = vec![0_u8; CHUNK];
        loop {
            let count = tokio::time::timeout(
                IDLE,
                local_work(&mut self.outcome, &self.progress, source.read(&mut buffer)),
            )
            .await
            .context("Local read made no progress")??;
            if count == 0 {
                break;
            }
            tokio::time::timeout(IDLE, output.write_all(&buffer[..count]))
                .await
                .context("SFTP upload made no progress")??;
            self.outcome.bytes += count as u64;
            self.publish();
        }
        self.set_phase("closing-partial");
        tokio::time::timeout(REQUEST, output.close())
            .await
            .context("SFTP close timed out; partial retained")??;
        // Once a commit request is sent, timeout/cancel means UNKNOWN until an
        // explicit inspection. Never delete the final path in any error path.
        self.set_phase("committing");
        let linked = tokio::time::timeout(REQUEST, self.session().hardlink(&partial, destination))
            .await
            .context("Finalization response missing; completion unknown")?;
        match linked {
            Ok(true) => self.set_phase("completed-partial-retained"),
            Ok(false) => {
                self.set_phase("unsupported-finalization");
                bail!(
                    "Server lacks hardlink finalization; original destination preserved, partial retained"
                );
            }
            Err(error) => {
                // An explicit server rejection is definite. A lost response is
                // still conservatively unknown, as reflected by committing.
                if matches!(error, russh_sftp::client::error::Error::Status(_)) {
                    self.set_phase("finalization-rejected");
                }
                return Err(error.into());
            }
        }
        tokio::time::timeout(REQUEST, self.session().remove_file(&partial))
            .await
            .context("Upload completed; partial cleanup timed out")??;
        self.outcome.partial = None;
        self.set_phase("completed");
        Ok(())
    }

    pub async fn download(&mut self, remote: &str, destination: &Path) -> Result<()> {
        ensure!(
            self.metadata(remote).await?.is_regular(),
            "Download source must be a regular file, not a symlink"
        );
        let parent = destination
            .parent()
            .context("Destination needs a directory")?;
        let parent_metadata = tokio::time::timeout(
            REQUEST,
            local_work(
                &mut self.outcome,
                &self.progress,
                tokio::fs::metadata(parent),
            ),
        )
        .await
        .context("Local destination metadata timed out")??;
        ensure!(
            parent_metadata.is_dir() && destination.file_name().is_some(),
            "Invalid local destination"
        );
        let partial = parent.join(format!(".ssh-files-{}.partial", uuid::Uuid::new_v4()));
        self.outcome = Outcome {
            phase: "creating-partial",
            partial: Some(partial.display().to_string()),
            destination: Some(destination.display().to_string()),
            bytes: 0,
            local_io_pending: false,
        };
        self.publish();
        let mut local_options = tokio::fs::OpenOptions::new();
        local_options.write(true).create_new(true);
        #[cfg(unix)]
        local_options.mode(0o600);
        let mut output = tokio::time::timeout(
            REQUEST,
            local_work(
                &mut self.outcome,
                &self.progress,
                local_options.open(&partial),
            ),
        )
        .await
        .context("Local partial creation timed out; inspect reported path")??;
        self.set_phase("transferring");
        let mut source = tokio::time::timeout(REQUEST, self.session().open(remote))
            .await
            .context("SFTP open timed out")??;
        ensure!(
            tokio::time::timeout(REQUEST, source.metadata())
                .await
                .context("SFTP source metadata timed out")??
                .is_regular(),
            "Opened remote source is not a regular file"
        );
        let mut buffer = vec![0_u8; CHUNK];
        loop {
            let count = tokio::time::timeout(IDLE, source.read(&mut buffer))
                .await
                .context("SFTP download made no progress")??;
            if count == 0 {
                break;
            }
            tokio::time::timeout(
                IDLE,
                local_work(&mut self.outcome, &self.progress, async {
                    output.write_all(&buffer[..count]).await?;
                    // Tokio File may acknowledge a write before its blocking work
                    // finishes. Flush waits for that work before another network read.
                    output.flush().await
                }),
            )
            .await
            .context("Local write made no progress")??;
            self.outcome.bytes += count as u64;
            self.publish();
        }
        self.set_phase("closing-partial");
        tokio::time::timeout(REQUEST, source.close())
            .await
            .context("SFTP source close timed out")??;
        tokio::time::timeout(
            REQUEST,
            local_work(&mut self.outcome, &self.progress, output.sync_all()),
        )
        .await
        .context("Local file flush timed out")??;
        drop(output);
        self.set_phase("committing");
        // hard_link is create-new on the destination. Unlike rename on Unix,
        // an existing file/symlink is never replaced, including concurrent creation.
        if let Err(error) = tokio::time::timeout(
            REQUEST,
            local_work(
                &mut self.outcome,
                &self.progress,
                tokio::fs::hard_link(&partial, destination),
            ),
        )
        .await
        .context("Local finalization response missing; completion unknown")?
        {
            self.set_phase("finalization-rejected");
            return Err(error.into());
        }
        self.set_phase("completed-partial-retained");
        tokio::time::timeout(
            REQUEST,
            local_work(
                &mut self.outcome,
                &self.progress,
                tokio::fs::remove_file(&partial),
            ),
        )
        .await
        .context("Download completed; partial cleanup uncertain")??;
        self.outcome.partial = None;
        self.set_phase("completed");
        Ok(())
    }

    pub async fn shutdown(mut self) -> Result<String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        if let Some(session) = self.session.take() {
            // The pinned dependency currently only queues a close marker. Keep
            // the outer bound even if its implementation changes later.
            let _ = tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                session.close(),
            )
            .await;
            drop(session);
        }
        // Successful operations already awaited their replies. On cancellation,
        // close this entire connection, including its ProxyCommand children.
        self.connection.shutdown_until(deadline).await
    }
}

fn valid_remote(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.chars().any(char::is_control),
        "Invalid remote path"
    );
    Ok(())
}

fn remote_partial(destination: &str) -> Result<String> {
    let (parent, name) = destination.rsplit_once('/').unwrap_or((".", destination));
    ensure!(
        !name.is_empty() && name != "." && name != "..",
        "Invalid remote destination filename"
    );
    let parent = if parent.is_empty() { "/" } else { parent };
    Ok(format!(
        "{}/.ssh-files-{}.partial",
        parent.trim_end_matches('/'),
        uuid::Uuid::new_v4()
    ))
}

/// Only actual completion clears this marker. Cancellation drops this future
/// before the clear, so the UI must quarantine later local filesystem actions.
async fn local_work<T>(
    outcome: &mut Outcome,
    progress: &Option<tokio::sync::watch::Sender<Outcome>>,
    future: impl std::future::Future<Output = T>,
) -> T {
    outcome.local_io_pending = true;
    if let Some(sender) = progress {
        sender.send_replace(outcome.clone());
    }
    let result = future.await;
    outcome.local_io_pending = false;
    if let Some(sender) = progress {
        sender.send_replace(outcome.clone());
    }
    result
}

#[cfg(test)]
mod local_io_tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_local_work_remains_quarantined_but_completed_work_clears() {
        let mut outcome = Outcome::default();
        let (sender, receiver) = tokio::sync::watch::channel(outcome.clone());
        let progress = Some(sender);
        let result = tokio::time::timeout(
            Duration::from_millis(1),
            local_work(&mut outcome, &progress, std::future::pending::<()>()),
        )
        .await;
        assert!(result.is_err());
        assert!(outcome.local_io_pending);
        assert!(receiver.borrow().local_io_pending);
        // Another worker must not clear the first worker's uncertain state.
        let mut independent = Outcome::default();
        local_work(&mut independent, &None, async {}).await;
        assert!(!independent.local_io_pending);
        assert!(outcome.local_io_pending);
    }
    #[tokio::test]
    async fn network_only_cancel_has_no_unfinished_local_work() {
        let mut outcome = Outcome::default();
        local_work(&mut outcome, &None, async {}).await;
        let _ = tokio::time::timeout(Duration::from_millis(1), std::future::pending::<()>()).await;
        assert!(!outcome.local_io_pending);
    }
}
