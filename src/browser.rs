use crate::{REQUEST, Route, connection, paths};
use anyhow::{Context, Result, ensure};
use russh_sftp::{
    client::{RawSftpSession, error::Error},
    protocol::{FileAttributes, StatusCode},
};
use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File,
    Other,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    pub rejected: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Listing {
    pub inspected: usize,
    pub path: String,
    pub entries: Vec<Entry>,
}

fn finish_entries(entries: &mut [Entry]) {
    let mut seen = HashMap::<String, usize>::new();
    for index in 0..entries.len() {
        let key = paths::collision_key(&entries[index].name);
        if let Some(previous) = seen.insert(key, index) {
            let reason = "Names collide by case or Unicode normalization".to_owned();
            entries[previous].rejected = Some(reason.clone());
            entries[index].rejected = Some(reason);
        }
    }
    entries.sort_by_key(|entry| {
        (
            entry.kind != Kind::Directory,
            paths::collision_key(&entry.name),
        )
    });
}

pub fn list_local(path: &Path, cancel: &AtomicBool) -> Result<Listing> {
    let path = paths::checked_local(path, false)?;
    ensure!(path.is_dir(), "Local path is not a directory");
    let mut entries = Vec::new();
    for (count, item) in std::fs::read_dir(&path)?.enumerate() {
        ensure!(!cancel.load(Ordering::Acquire), "Local listing cancelled");
        ensure!(
            count < paths::ENTRY_LIMIT,
            "Directory exceeds the 10,000-entry limit"
        );
        let item = item?;
        let name = item.file_name();
        let metadata = std::fs::symlink_metadata(item.path())?;
        let kind = if paths::is_link(&metadata) {
            Kind::Other
        } else if metadata.is_dir() {
            Kind::Directory
        } else if metadata.is_file() {
            Kind::File
        } else {
            Kind::Other
        };
        let mut rejected = name
            .to_str()
            .and_then(|name| paths::child_problem(name, cfg!(windows)))
            .map(str::to_owned);
        if name.to_str().is_none() {
            rejected = Some("Filename is not valid UTF-8".into());
        }
        if kind == Kind::Other {
            rejected = Some("Links, reparse points and special files are not followed".into());
        }
        entries.push(Entry {
            name: name.to_string_lossy().chars().take(1024).collect(),
            kind,
            size: metadata.len(),
            rejected,
        });
    }
    finish_entries(&mut entries);
    Ok(Listing {
        inspected: entries.len(),
        path: path.to_string_lossy().into_owned(),
        entries,
    })
}

pub struct Browser {
    session: RawSftpSession,
    connection: connection::Connection,
}

impl Browser {
    pub async fn connect(route: &Route) -> Result<Self> {
        Self::connect_in(route, &crate::ProcessRegistry::default()).await
    }

    pub async fn connect_in(route: &Route, registry: &crate::ProcessRegistry) -> Result<Self> {
        let (connection, stream) =
            connection::Connection::spawn(route, registry, crate::process::Role::Browser)?;
        let session = RawSftpSession::new_with_config(stream, connection::config());
        match tokio::time::timeout(REQUEST, session.init()).await {
            Ok(Ok(_)) => Ok(Self {
                session,
                connection,
            }),
            error => {
                let cleanup = connection.shutdown().await;
                anyhow::bail!("SFTP browser initialization failed: {error:?}; {cleanup:?}");
            }
        }
    }

    pub async fn list(&self, path: &str) -> Result<Listing> {
        self.list_with_budget(path, paths::ENTRY_LIMIT).await
    }

    pub async fn list_with_budget(&self, path: &str, budget: usize) -> Result<Listing> {
        tokio::time::timeout(Duration::from_secs(30), self.list_inner(path, budget))
            .await
            .context("Directory scan exceeded 30 seconds; connection will be closed")?
    }

    async fn list_inner(&self, path: &str, budget: usize) -> Result<Listing> {
        crate::valid_remote(path)?;
        ensure!(path.len() <= 4096, "Remote path is too long");
        let attrs = self.session.lstat(path).await?.attrs;
        ensure!(
            attrs.file_type().is_dir(),
            "Remote path is not a regular directory (links are not followed)"
        );
        let canonical = self
            .session
            .realpath(path)
            .await?
            .files
            .into_iter()
            .next()
            .context("Server returned no canonical directory")?
            .filename;
        crate::valid_remote(&canonical)?;
        ensure!(
            canonical.len() <= 4096 && !canonical.contains('\u{fffd}'),
            "Server returned an unsupported directory path"
        );
        let handle = self.session.opendir(&canonical).await?.handle;
        let mut entries = Vec::new();
        let mut count = 0;
        let result: Result<()> = async {
            loop {
                match self.session.readdir(&handle).await {
                    Ok(page) => append_page(&mut entries, &mut count, page.files, budget)?,
                    Err(Error::Status(status)) if status.status_code == StatusCode::Eof => break,
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        }
        .await;
        let closed = tokio::time::timeout(REQUEST, self.session.close(handle)).await;
        result?;
        closed.context("Directory handle close timed out")??;
        finish_entries(&mut entries);
        Ok(Listing {
            inspected: count,
            path: canonical,
            entries,
        })
    }

    pub async fn attributes(&self, path: &str) -> Result<Option<FileAttributes>> {
        crate::valid_remote(path)?;
        match self.session.lstat(path).await {
            Ok(attrs) => Ok(Some(attrs.attrs)),
            Err(Error::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn create_directory(&self, path: &str) -> Result<()> {
        if let Some(attrs) = self.attributes(path).await? {
            ensure!(
                attrs.file_type().is_dir(),
                "Destination exists and is not a regular directory: {}",
                paths::display(path)
            );
            return Ok(());
        }
        self.session
            .mkdir(
                path,
                FileAttributes {
                    permissions: Some(0o700),
                    ..FileAttributes::default()
                },
            )
            .await?;
        Ok(())
    }

    pub async fn shutdown(self) -> Result<String> {
        let _ = self.session.close_session();
        self.connection.shutdown().await
    }
}

fn append_page(
    entries: &mut Vec<Entry>,
    count: &mut usize,
    page: Vec<russh_sftp::protocol::File>,
    budget: usize,
) -> Result<()> {
    ensure!(
        !page.is_empty(),
        "Server returned an empty directory page without EOF"
    );
    ensure!(
        page.len() <= budget.min(paths::ENTRY_LIMIT).saturating_sub(*count),
        "Directory exceeds the 10,000-entry limit"
    );
    *count += page.len();
    for file in page {
        if file.filename == "." || file.filename == ".." {
            continue;
        }
        let mut rejected = paths::child_problem(&file.filename, false).map(str::to_owned);
        let kind = if file.attrs.file_type().is_dir() {
            Kind::Directory
        } else if file.attrs.is_regular() {
            Kind::File
        } else {
            Kind::Other
        };
        if kind == Kind::Other {
            rejected = Some("Links and special files are not followed".into());
        }
        entries.push(Entry {
            name: file.filename.chars().take(1024).collect(),
            kind,
            size: file.attrs.size.unwrap_or(0),
            rejected,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_limit_applies_before_accumulation_and_counts_rejected_entries() {
        let mut entries = Vec::new();
        let mut count = paths::ENTRY_LIMIT;
        let file = russh_sftp::protocol::File {
            filename: "../unsafe".into(),
            longname: String::new(),
            attrs: FileAttributes::default(),
        };
        assert!(append_page(&mut entries, &mut count, vec![file], paths::ENTRY_LIMIT).is_err());
        assert!(entries.is_empty());
        count = 0;
        assert!(append_page(&mut entries, &mut count, vec![], paths::ENTRY_LIMIT).is_err());
        let file = russh_sftp::protocol::File {
            filename: "../unsafe".into(),
            longname: String::new(),
            attrs: FileAttributes::default(),
        };
        append_page(&mut entries, &mut count, vec![file], paths::ENTRY_LIMIT).unwrap();
        assert_eq!(count, 1);
        assert!(entries[0].rejected.is_some());
    }
    #[test]
    fn invisible_dot_entries_consume_the_shared_plan_budget() {
        let dot = || russh_sftp::protocol::File {
            filename: ".".into(),
            longname: String::new(),
            attrs: FileAttributes::default(),
        };
        let mut count = 0;
        let mut entries = Vec::new();
        append_page(&mut entries, &mut count, vec![dot(), dot()], 2).unwrap();
        assert_eq!(count, 2);
        assert!(entries.is_empty());
        assert!(append_page(&mut entries, &mut count, vec![dot()], 2).is_err());
        let mut next_count = 0;
        assert!(append_page(&mut Vec::new(), &mut next_count, vec![dot()], 2 - count).is_err());
    }
}
