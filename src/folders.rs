//! Explicit create-new directory requests, separate from transfer planning.
use crate::{browser::Browser, paths};
use anyhow::{Context, Result, ensure};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub struct Request {
    pub local: bool,
    pub parent: String,
    pub name: String,
}
impl Request {
    pub fn destination(&self) -> Result<String> {
        ensure!(
            paths::child_problem(&self.name, true).is_none(),
            "Use one portable folder name without separators or reserved characters."
        );
        if self.local {
            let path = PathBuf::from(&self.parent).join(&self.name);
            ensure!(
                path.is_absolute() && path.as_os_str().len() <= 4096,
                "Folder path is not an absolute supported local path."
            );
            Ok(path.to_str().context("Folder path is not UTF-8")?.into())
        } else {
            crate::valid_remote(&self.parent)?;
            paths::remote_join(&self.parent, &self.name)
        }
    }
}

pub fn create_local(
    request: &Request,
    cancel: &AtomicBool,
    started: &AtomicBool,
) -> Result<String> {
    ensure!(request.local, "Expected a local folder request");
    let destination = request.destination()?;
    let parent = paths::checked_local(&PathBuf::from(&request.parent), false)?;
    let metadata = std::fs::symlink_metadata(&parent)?;
    ensure!(
        metadata.is_dir() && !paths::is_link(&metadata),
        "Parent is no longer a regular directory"
    );
    let name = paths::collision_key(&request.name);
    for (count, entry) in std::fs::read_dir(&parent)?.enumerate() {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Folder creation cancelled before writing"
        );
        ensure!(
            count < paths::ENTRY_LIMIT,
            "Directory exceeds the 10,000-entry limit"
        );
        let entry = entry?;
        ensure!(
            entry
                .file_name()
                .to_str()
                .is_none_or(|value| paths::collision_key(value) != name),
            "An entry with this name already exists"
        );
    }
    let destination = paths::checked_local(&PathBuf::from(destination), true)?;
    let metadata = std::fs::symlink_metadata(paths::checked_local(&parent, false)?)?;
    ensure!(
        metadata.is_dir() && !paths::is_link(&metadata),
        "Parent is no longer a regular directory"
    );
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Folder creation cancelled before writing"
    );
    started.store(true, Ordering::Release);
    #[allow(unused_mut)]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&destination)
        .context("Could not create a new folder; existing entries are kept")?;
    Ok(destination
        .to_str()
        .context("Folder path is not UTF-8")?
        .into())
}

pub async fn create_remote(
    browser: &Browser,
    request: &Request,
    started: &AtomicBool,
) -> Result<String> {
    ensure!(!request.local, "Expected a remote folder request");
    let destination = request.destination()?;
    let listing = browser.list(&request.parent).await?;
    ensure!(
        !listing
            .entries
            .iter()
            .any(|entry| paths::collision_key(&entry.name) == paths::collision_key(&request.name)),
        "An entry with this name already exists"
    );
    browser
        .create_new_directory(&request.parent, &destination, started)
        .await?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creates_only_new_portable_children_and_preserves_collisions() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        let mut request = Request {
            local: true,
            parent: parent.to_str().unwrap().into(),
            name: "new 開發 folder".into(),
        };
        let started = AtomicBool::new(false);
        let cancel = AtomicBool::new(false);
        let created = create_local(&request, &cancel, &started).unwrap();
        assert!(PathBuf::from(created).is_dir());
        assert!(started.load(Ordering::Acquire));
        started.store(false, Ordering::Release);
        assert!(create_local(&request, &cancel, &started).is_err());
        assert!(!started.load(Ordering::Acquire));
        std::fs::write(parent.join("kept"), b"original").unwrap();
        request.name = "KEPT".into();
        assert!(create_local(&request, &cancel, &started).is_err());
        assert_eq!(std::fs::read(parent.join("kept")).unwrap(), b"original");
        for name in [
            "../escape",
            "a/b",
            "a\\b",
            "",
            "CON",
            "trailing.",
            "bad\nname",
        ] {
            request.name = name.into();
            assert!(create_local(&request, &cancel, &started).is_err(), "{name}");
        }
        request.name = "cancelled".into();
        cancel.store(true, Ordering::Release);
        assert!(create_local(&request, &cancel, &started).is_err());
        assert!(!parent.join("cancelled").exists());
        assert!(!started.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[test]
    fn creation_rejects_symlinked_parent() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir(temporary.path().join("real")).unwrap();
        std::os::unix::fs::symlink(temporary.path().join("real"), temporary.path().join("link"))
            .unwrap();
        let request = Request {
            local: true,
            parent: temporary.path().join("link").to_str().unwrap().into(),
            name: "no-write".into(),
        };
        assert!(create_local(&request, &AtomicBool::new(false), &AtomicBool::new(false)).is_err());
        assert!(!temporary.path().join("real/no-write").exists());
    }
}
