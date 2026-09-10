//! Bounded, reviewable plans. Planning never writes a destination.
use crate::{
    browser::{Browser, Kind},
    paths,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub direction: Direction,
    pub directory: bool,
    pub source: String,
    pub destination: String,
    pub bytes: u64,
}

impl Job {
    pub fn label(&self) -> &'static str {
        if self.directory {
            "Folder"
        } else if self.direction == Direction::Upload {
            "Upload"
        } else {
            "Download"
        }
    }
    pub fn rename_destination(&mut self, name: &str) -> Result<()> {
        ensure!(
            paths::child_problem(name, self.direction == Direction::Download && cfg!(windows))
                .is_none(),
            "Unsupported destination filename"
        );
        self.destination = if self.direction == Direction::Upload {
            paths::remote_join(&paths::remote_parent(&self.destination), name)?
        } else {
            Path::new(&self.destination)
                .parent()
                .context("Destination needs a parent")?
                .join(name)
                .to_str()
                .context("Destination must be UTF-8")?
                .to_owned()
        };
        Ok(())
    }
}

pub fn rename(jobs: &mut [Job], index: usize, name: &str) -> Result<()> {
    let original = jobs.get(index).context("No selected transfer")?.clone();
    let mut changed = jobs.to_vec();
    changed[index].rename_destination(name)?;
    let replacement = changed[index].destination.clone();
    if original.directory {
        for (position, job) in changed.iter_mut().enumerate() {
            if position == index {
                continue;
            }
            if original.direction == Direction::Upload {
                if let Some(relative) = job
                    .destination
                    .strip_prefix(&(original.destination.clone() + "/"))
                {
                    job.destination = format!("{replacement}/{relative}");
                }
            } else if let Ok(relative) =
                Path::new(&job.destination).strip_prefix(&original.destination)
            {
                job.destination = Path::new(&replacement)
                    .join(relative)
                    .to_str()
                    .context("Destination must be UTF-8")?
                    .into();
            }
        }
    }
    let mut seen = HashSet::new();
    for job in &changed {
        ensure!(
            job.destination.len() <= 4096 && seen.insert(paths::collision_key(&job.destination)),
            "Renamed destinations would collide or exceed the path limit"
        );
    }
    jobs.clone_from_slice(&changed);
    Ok(())
}

pub fn skip(jobs: &mut Vec<Job>, index: usize) {
    if let Some(job) = jobs.get(index).cloned() {
        jobs.remove(index);
        if job.directory {
            jobs.retain(|item| {
                if job.direction == Direction::Upload {
                    !item
                        .destination
                        .starts_with(&(job.destination.clone() + "/"))
                } else {
                    !Path::new(&item.destination).starts_with(&job.destination)
                }
            });
        }
    }
}

fn add(jobs: &mut Vec<Job>, seen: &mut HashSet<String>, job: Job) -> Result<()> {
    ensure!(
        jobs.len() < paths::QUEUE_LIMIT,
        "Selection exceeds the 1,000-item queue limit"
    );
    ensure!(
        seen.insert(paths::collision_key(&job.destination)),
        "Two selected paths would share a destination: {}",
        paths::display(&job.destination)
    );
    jobs.push(job);
    Ok(())
}

pub fn upload(sources: &[PathBuf], remote: &str, cancel: &AtomicBool) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    let mut seen = HashSet::new();
    let mut inspected = 0;
    let mut stack = Vec::new();
    for path in sources.iter().rev() {
        let path = paths::checked_local(path, false)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("Select a named UTF-8 file or folder")?;
        stack.push((path.clone(), paths::remote_join(remote, name)?, 0));
    }
    while let Some((source, destination, depth)) = stack.pop() {
        ensure!(!cancel.load(Ordering::Acquire), "Selection scan cancelled");
        inspected += 1;
        ensure!(
            inspected <= paths::ENTRY_LIMIT && depth <= paths::DEPTH_LIMIT,
            "Selection exceeds the scan/depth limit"
        );
        let metadata = std::fs::symlink_metadata(&source)?;
        ensure!(
            !paths::is_link(&metadata) && (metadata.is_file() || metadata.is_dir()),
            "Links/reparse/special files are not transferred: {}",
            source.display()
        );
        add(
            &mut jobs,
            &mut seen,
            Job {
                direction: Direction::Upload,
                directory: metadata.is_dir(),
                source: source.to_str().context("Source path must be UTF-8")?.into(),
                destination: destination.clone(),
                bytes: metadata.len(),
            },
        )?;
        if metadata.is_dir() {
            let mut children = Vec::new();
            for child in std::fs::read_dir(&source)? {
                ensure!(!cancel.load(Ordering::Acquire), "Selection scan cancelled");
                inspected += 1;
                ensure!(
                    children.len() + stack.len() + inspected < paths::ENTRY_LIMIT,
                    "Selection exceeds the 10,000-entry scan limit"
                );
                let child = child?;
                let name = child
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("Folder contains a non-UTF-8 filename"))?;
                children.push((
                    child.path(),
                    paths::remote_join(&destination, &name)?,
                    depth + 1,
                ));
            }
            children.sort_by(|a, b| a.1.cmp(&b.1));
            stack.extend(children.into_iter().rev());
        }
    }
    Ok(jobs)
}

pub async fn download(browser: &Browser, sources: &[String], local: &Path) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    let mut seen = HashSet::new();
    let mut inspected = 0;
    let mut stack = Vec::new();
    for source in sources.iter().rev() {
        let name = source
            .rsplit('/')
            .next()
            .context("Remote source needs a filename")?;
        ensure!(
            paths::child_problem(name, cfg!(windows)).is_none(),
            "Unsupported local filename: {}",
            paths::display(name)
        );
        stack.push((source.clone(), local.join(name), 0));
    }
    while let Some((source, destination, depth)) = stack.pop() {
        inspected += 1;
        ensure!(
            inspected <= paths::ENTRY_LIMIT && depth <= paths::DEPTH_LIMIT,
            "Selection exceeds the scan/depth limit"
        );
        let attrs = browser
            .attributes(&source)
            .await?
            .context("Remote source no longer exists")?;
        let directory = attrs.file_type().is_dir();
        ensure!(
            directory || attrs.is_regular(),
            "Remote links and special files are not transferred"
        );
        add(
            &mut jobs,
            &mut seen,
            Job {
                direction: Direction::Download,
                directory,
                source: source.clone(),
                destination: destination
                    .to_str()
                    .context("Local destination must be UTF-8")?
                    .into(),
                bytes: attrs.size.unwrap_or(0),
            },
        )?;
        if directory {
            let listing = browser
                .list_with_budget(&source, paths::ENTRY_LIMIT.saturating_sub(inspected))
                .await?;
            inspected += listing.inspected;
            ensure!(
                listing.entries.len() + stack.len() + inspected <= paths::ENTRY_LIMIT,
                "Selection exceeds the 10,000-entry scan limit"
            );
            for entry in listing.entries.into_iter().rev() {
                ensure!(
                    entry.rejected.is_none() && entry.kind != Kind::Other,
                    "Folder contains an unsupported entry: {}",
                    paths::display(&entry.name)
                );
                ensure!(
                    paths::child_problem(&entry.name, cfg!(windows)).is_none(),
                    "Filename cannot be downloaded here: {}",
                    paths::display(&entry.name)
                );
                stack.push((
                    paths::remote_join(&source, &entry.name)?,
                    destination.join(&entry.name),
                    depth + 1,
                ));
            }
        }
    }
    Ok(jobs)
}

/// Explorer's quoted paths and one path per line. Never apply shell escaping
/// or execute pasted text. Unquoted text with spaces denotes one path.
pub fn pasted_paths(value: &str) -> Result<Vec<PathBuf>> {
    ensure!(value.len() <= 64 * 1024, "Paste is too large");
    let mut paths = Vec::new();
    for line in value.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let mut rest = line;
        while !rest.is_empty() {
            let (path, following) = if rest.starts_with(['\"', '\'']) {
                let quote = rest.chars().next().unwrap();
                let end = rest[1..]
                    .find(quote)
                    .context("Pasted path has an unclosed quote")?
                    + 1;
                (&rest[1..end], rest[end + 1..].trim_start())
            } else {
                (rest, "")
            };
            ensure!(
                !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control),
                "Invalid pasted path"
            );
            ensure!(paths.len() < paths::QUEUE_LIMIT, "Too many pasted paths");
            paths.push(PathBuf::from(path));
            rest = following;
        }
    }
    ensure!(!paths.is_empty(), "Paste one or more local file paths");
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pasted_shell_characters_are_literal_data() {
        let paths = pasted_paths("\"C:\\space 開發\\x$;&.txt\" \"C:\\other.txt\"\n/tmp/space file")
            .unwrap();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("C:\\space 開發\\x$;&.txt"),
                PathBuf::from("C:\\other.txt"),
                PathBuf::from("/tmp/space file")
            ]
        );
        assert!(pasted_paths("\"unclosed").is_err());
    }
    #[test]
    fn queue_caps_collisions_and_directory_order_are_checked_before_transfer() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let folder = root.join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("item"), b"hello").unwrap();
        let jobs = upload(&[folder], "/remote", &AtomicBool::new(false)).unwrap();
        assert!(jobs[0].directory);
        assert_eq!(jobs[1].destination, "/remote/folder/item");
        let mut renamed = jobs.clone();
        rename(&mut renamed, 0, "other").unwrap();
        assert_eq!(renamed[1].destination, "/remote/other/item");
        assert_eq!(renamed[1].source, jobs[1].source);
        skip(&mut renamed, 0);
        assert!(renamed.is_empty());
        let first = root.join("same");
        std::fs::write(&first, b"x").unwrap();
        assert!(upload(&[first.clone(), first], "/remote", &AtomicBool::new(false)).is_err());
    }
}
