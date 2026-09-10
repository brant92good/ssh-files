use anyhow::{Result, bail, ensure};
use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const ENTRY_LIMIT: usize = 10_000;
pub const QUEUE_LIMIT: usize = 1_000;
pub const DEPTH_LIMIT: usize = 64;

pub fn child_problem(name: &str, windows: bool) -> Option<&'static str> {
    if name.is_empty() || name == "." || name == ".." {
        return Some("not a child filename");
    }
    if name.len() > 1024 || name.contains('\u{fffd}') {
        return Some("unsupported or undecodable filename");
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Some("filename contains a separator or control character");
    }
    if name.as_bytes().get(1) == Some(&b':') && name.as_bytes()[0].is_ascii_alphabetic() {
        return Some("filename resembles an absolute drive path");
    }
    if windows {
        if name.ends_with(['.', ' ']) || name.chars().any(|c| "<>:\"|?*".contains(c)) {
            return Some("filename is not supported by Windows");
        }
        let stem = name.split('.').next().unwrap_or(name).to_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(
                    &stem[3..],
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                ))
        {
            return Some("filename is reserved by Windows");
        }
    }
    None
}

pub fn display(value: &str) -> String {
    value
        .chars()
        .take(4096)
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
            .chars()
            .collect::<Vec<_>>()
        })
        .collect()
}

pub fn collision_key(value: &str) -> String {
    caseless::default_case_fold_str(&value.nfd().collect::<String>())
        .nfd()
        .collect()
}

pub fn remote_join(parent: &str, child: &str) -> Result<String> {
    ensure!(
        child_problem(child, false).is_none(),
        "Unsafe remote child name: {}",
        display(child)
    );
    let result = format!("{}/{}", parent.trim_end_matches('/'), child);
    ensure!(result.len() <= 4096, "Remote path is too long");
    Ok(result)
}

pub fn remote_parent(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((parent, _)) => parent.into(),
        None => ".".into(),
    }
}

pub fn is_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

/// Reject existing link/reparse ancestors. This is a check, not an atomic
/// filesystem sandbox: another local process can still race a path replacement.
pub fn checked_local(path: &Path, allow_missing_leaf: bool) -> Result<PathBuf> {
    let path = std::path::absolute(path)?;
    ensure!(
        path.to_str().is_some(),
        "Local path is not valid UTF-8; it cannot be used as an action path"
    );
    let mut current = PathBuf::new();
    let count = path.components().count();
    for (index, component) in path.components().enumerate() {
        ensure!(
            !matches!(component, Component::ParentDir),
            "Parent traversal is not allowed"
        );
        current.push(component);
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                !is_link(&metadata),
                "Link or reparse path is not supported: {}",
                current.display()
            ),
            Err(error)
                if allow_missing_leaf
                    && index + 1 == count
                    && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if path.as_os_str().len() > 4096 {
        bail!("Local path is too long");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hostile_names_are_not_action_paths() {
        for name in [
            "",
            ".",
            "..",
            "/tmp",
            "x/y",
            "x\\y",
            "C:other",
            "a\0b",
            "bad\u{fffd}",
        ] {
            assert!(child_problem(name, false).is_some(), "{name:?}");
        }
        for name in [
            "nul.txt",
            "COM1",
            "lpt².log",
            "x:stream",
            "trailing.",
            "trailing ",
        ] {
            assert!(child_problem(name, true).is_some(), "{name:?}");
        }
        for name in ["space 開發", "literal$;&[].bin", "report-01"] {
            assert!(child_problem(name, true).is_none(), "{name}");
        }
        assert_eq!(display("x\u{1b}[31m\n"), "x\\u{1b}[31m\\n");
    }
    #[test]
    fn portable_collisions_include_case_and_normalization() {
        assert_eq!(collision_key("Straße"), collision_key("STRASSE"));
        assert_eq!(collision_key("é.txt"), collision_key("e\u{301}.txt"));
    }
    #[cfg(unix)]
    #[test]
    fn undecodable_parent_is_never_replaced_with_a_neighboring_action_path() {
        use std::os::unix::ffi::OsStringExt;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let invalid = root.join(std::ffi::OsString::from_vec(vec![0xff]));
        let neighbor = root.join("\u{fffd}");
        match std::fs::create_dir(&invalid) {
            Ok(()) => {}
            // APFS itself rejects byte paths that cannot be decoded. The
            // app must still reject the supplied path without selecting its
            // valid replacement-character neighbor.
            Err(error)
                if cfg!(target_os = "macos") && error.raw_os_error() == Some(libc::EILSEQ) => {}
            Err(error) => panic!("Could not create invalid-byte fixture: {error}"),
        }
        std::fs::create_dir(&neighbor).unwrap();
        assert!(checked_local(&invalid, false).is_err());
        assert!(
            crate::browser::list_local(&invalid, &std::sync::atomic::AtomicBool::new(false))
                .is_err()
        );
        assert!(neighbor.is_dir());
    }
}
