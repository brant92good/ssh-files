use crate::{
    browser::{Entry, Listing},
    paths,
};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Name,
    NameDescending,
    Size,
    SizeDescending,
}
impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name ↑",
            Self::NameDescending => "Name ↓",
            Self::Size => "Size ↑",
            Self::SizeDescending => "Size ↓",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Name => Self::NameDescending,
            Self::NameDescending => Self::Size,
            Self::Size => Self::SizeDescending,
            Self::SizeDescending => Self::Name,
        }
    }
}

pub struct Pane {
    pub path: String,
    pub entries: Vec<Entry>,
    pub filter: String,
    pub visible: Vec<usize>,
    pub cursor: usize,
    pub marked: BTreeSet<String>,
    pub show_hidden: bool,
    pub revision: u64,
    pub offset: usize,
    pub sort: Sort,
}

impl Pane {
    pub fn new(path: String) -> Self {
        Self {
            path,
            entries: Vec::new(),
            filter: String::new(),
            visible: Vec::new(),
            cursor: 0,
            marked: BTreeSet::new(),
            show_hidden: false,
            revision: 0,
            offset: 0,
            sort: Sort::default(),
        }
    }
    pub fn replace(&mut self, listing: Listing) {
        let previous = if self.path == listing.path {
            self.current().map(|entry| entry.name.clone())
        } else {
            None
        };
        self.path = listing.path;
        self.entries = listing.entries;
        self.visible.clear();
        self.cursor = 0;
        self.offset = 0;
        self.marked.clear();
        self.refilter();
        if let Some(name) = previous {
            self.restore_cursor(&name);
        }
    }
    pub fn refilter(&mut self) {
        let previous = self.current().map(|entry| entry.name.clone());
        let query = paths::collision_key(&self.filter);
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                (self.show_hidden || !entry.hidden)
                    && paths::collision_key(&entry.name).contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
        use std::cmp::Reverse;
        let entries = &self.entries;
        match self.sort {
            Sort::Name => self.visible.sort_by_cached_key(|i| {
                (
                    entries[*i].kind != crate::browser::Kind::Directory,
                    paths::collision_key(&entries[*i].name),
                )
            }),
            Sort::NameDescending => self.visible.sort_by_cached_key(|i| {
                (
                    entries[*i].kind != crate::browser::Kind::Directory,
                    Reverse(paths::collision_key(&entries[*i].name)),
                )
            }),
            Sort::Size => self.visible.sort_by_cached_key(|i| {
                (
                    entries[*i].kind != crate::browser::Kind::Directory,
                    entries[*i].size,
                    paths::collision_key(&entries[*i].name),
                )
            }),
            Sort::SizeDescending => self.visible.sort_by_cached_key(|i| {
                (
                    entries[*i].kind != crate::browser::Kind::Directory,
                    Reverse(entries[*i].size),
                    paths::collision_key(&entries[*i].name),
                )
            }),
        }
        self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
        if let Some(name) = previous {
            self.restore_cursor(&name);
        }
        let visible_names: BTreeSet<_> = self
            .visible
            .iter()
            .map(|index| &self.entries[*index].name)
            .collect();
        self.marked.retain(|name| visible_names.contains(name));
        self.revision = self.revision.wrapping_add(1);
    }
    fn restore_cursor(&mut self, name: &str) {
        if let Some(row) = self
            .visible
            .iter()
            .position(|index| self.entries[*index].name == name)
        {
            self.cursor = row;
        }
    }
    pub fn set_show_hidden(&mut self, show: bool) {
        self.show_hidden = show;
        self.refilter();
    }
    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.refilter();
    }
    pub fn select_all(&mut self) -> Result<()> {
        let selected: BTreeSet<_> = self
            .visible
            .iter()
            .map(|i| &self.entries[*i])
            .filter(|entry| entry.rejected.is_none() && entry.kind != crate::browser::Kind::Other)
            .map(|entry| entry.name.clone())
            .collect();
        ensure!(
            selected.len() <= paths::QUEUE_LIMIT,
            "Select at most 1,000 entries; selection was unchanged"
        );
        self.marked = selected;
        Ok(())
    }
    pub fn current(&self) -> Option<&Entry> {
        self.visible
            .get(self.cursor)
            .and_then(|index| self.entries.get(*index))
    }
    pub fn move_by(&mut self, amount: isize) {
        self.cursor = self
            .cursor
            .saturating_add_signed(amount)
            .min(self.visible.len().saturating_sub(1));
    }
    pub fn view_start(&self, height: usize) -> usize {
        let mut offset = self.offset.min(self.visible.len().saturating_sub(height));
        if self.cursor < offset {
            offset = self.cursor;
        } else if height > 0 && self.cursor >= offset.saturating_add(height) {
            offset = self.cursor.saturating_sub(height - 1);
        }
        offset
    }
    pub fn toggle(&mut self) -> Result<()> {
        let Some(entry) = self.current() else {
            return Ok(());
        };
        ensure!(
            entry.rejected.is_none(),
            "{}",
            entry.rejected.as_deref().unwrap_or("Unsupported entry")
        );
        let name = entry.name.clone();
        if !self.marked.remove(&name) {
            ensure!(
                self.marked.len() < paths::QUEUE_LIMIT,
                "Select at most 1,000 entries"
            );
            self.marked.insert(name);
        }
        Ok(())
    }
    pub fn selected(&self) -> Result<Vec<String>> {
        let entries: Vec<&Entry> = if self.marked.is_empty() {
            self.current().into_iter().collect()
        } else {
            self.entries
                .iter()
                .filter(|entry| self.marked.contains(&entry.name))
                .collect()
        };
        ensure!(!entries.is_empty(), "Select a file or folder first");
        for entry in &entries {
            ensure!(
                entry.rejected.is_none(),
                "{}",
                entry.rejected.as_deref().unwrap_or("Unsupported entry")
            );
        }
        Ok(entries
            .into_iter()
            .map(|entry| entry.name.clone())
            .collect())
    }
}

#[derive(Clone, Copy)]
pub enum EditKind {
    LocalPath,
    RemotePath,
    LocalFilter,
    RemoteFilter,
    Paste,
    Rename,
}
#[derive(Clone)]
pub enum Modal {
    CreateFolder(crate::folders::Request),
    Review {
        jobs: Vec<crate::plan::Job>,
        cursor: usize,
    },
    Edit {
        kind: EditKind,
        text: String,
    },
    Help {
        scroll: u16,
    },
    Quit,
    Details {
        body: String,
        scroll: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::Kind;
    fn listing() -> Listing {
        Listing {
            inspected: 3,
            path: "/fixture".into(),
            entries: [(".env", true), ("main.rs", false), ("system.txt", true)]
                .into_iter()
                .map(|(name, hidden)| Entry {
                    name: name.into(),
                    kind: Kind::File,
                    size: 0,
                    hidden,
                    rejected: None,
                })
                .collect(),
        }
    }
    #[test]
    fn hidden_default_toggle_preserves_cursor_and_clears_invisible_marks() {
        let mut pane = Pane::new("/fixture".into());
        pane.replace(listing());
        assert_eq!(pane.visible.len(), 1);
        assert_eq!(pane.current().unwrap().name, "main.rs");
        pane.set_show_hidden(true);
        assert_eq!(pane.current().unwrap().name, "main.rs");
        pane.cursor = 0;
        pane.toggle().unwrap();
        assert_eq!(pane.selected().unwrap(), [".env"]);
        let revision = pane.revision;
        pane.set_show_hidden(false);
        assert!(pane.revision > revision);
        assert!(pane.marked.is_empty());
        assert_eq!(pane.selected().unwrap(), ["main.rs"]);
        pane.filter = ".env".into();
        pane.refilter();
        assert!(pane.selected().is_err());
        pane.set_show_hidden(true);
        assert_eq!(pane.current().unwrap().name, ".env");
    }
    #[test]
    fn hidden_setting_survives_directory_reload_without_losing_underlying_entries() {
        let mut pane = Pane::new("/fixture".into());
        pane.set_show_hidden(true);
        pane.replace(listing());
        assert_eq!(pane.visible.len(), 3);
        pane.cursor = 2;
        pane.replace(listing());
        assert_eq!(pane.current().unwrap().name, "system.txt");
        pane.set_show_hidden(false);
        assert_eq!(pane.entries.len(), 3);
        assert_eq!(pane.visible.len(), 1);
    }
    #[test]
    fn stale_marks_are_removed_when_listing_changes() {
        let mut pane = Pane::new("/a".into());
        pane.replace(Listing {
            inspected: 0,
            path: "/a".into(),
            entries: vec![Entry {
                name: "one".into(),
                kind: Kind::File,
                size: 0,
                hidden: false,
                rejected: None,
            }],
        });
        pane.toggle().unwrap();
        pane.replace(Listing {
            inspected: 0,
            path: "/b".into(),
            entries: vec![],
        });
        assert!(pane.marked.is_empty());
        assert!(pane.selected().is_err());
    }

    #[test]
    fn select_all_is_filtered_supported_and_atomic_at_the_limit() {
        let mut pane = Pane::new("/fixture".into());
        let mut data = listing();
        let mut rejected = data.entries[1].clone();
        rejected.name = "unsafe.rs".into();
        rejected.rejected = Some("unsafe".into());
        data.entries.push(rejected);
        let mut other = data.entries[1].clone();
        other.name = "pipe.rs".into();
        other.kind = Kind::Other;
        data.entries.push(other);
        pane.replace(data);
        pane.select_all().unwrap();
        assert_eq!(pane.marked, BTreeSet::from(["main.rs".into()]));
        pane.set_show_hidden(true);
        pane.filter = ".rs".into();
        pane.refilter();
        pane.select_all().unwrap();
        assert_eq!(pane.marked.len(), 1);
        let mut data = listing();
        data.entries = (0..=paths::QUEUE_LIMIT)
            .map(|i| Entry {
                name: format!("file-{i:04}"),
                kind: Kind::File,
                size: i as u64,
                hidden: false,
                rejected: None,
            })
            .collect();
        pane.filter.clear();
        pane.replace(data);
        pane.marked.insert("file-0001".into());
        let before = pane.marked.clone();
        assert!(pane.select_all().is_err());
        assert_eq!(pane.marked, before);
    }

    #[test]
    fn sorting_keeps_folders_first_and_selection_by_identity() {
        let mut pane = Pane::new("/fixture".into());
        pane.replace(Listing {
            path: "/fixture".into(),
            inspected: 3,
            entries: [
                ("folder", Kind::Directory, 999),
                ("small", Kind::File, 1),
                ("large", Kind::File, 42),
            ]
            .into_iter()
            .map(|(name, kind, size)| Entry {
                name: name.into(),
                kind,
                size,
                hidden: false,
                rejected: None,
            })
            .collect(),
        });
        pane.cursor = 1;
        let selected = pane.current().unwrap().name.clone();
        pane.toggle().unwrap();
        let revision = pane.revision;
        for (sort, names) in [
            (Sort::NameDescending, vec!["folder", "small", "large"]),
            (Sort::Size, vec!["folder", "small", "large"]),
            (Sort::SizeDescending, vec!["folder", "large", "small"]),
            (Sort::Name, vec!["folder", "large", "small"]),
        ] {
            pane.cycle_sort();
            assert_eq!(pane.sort, sort);
            assert_eq!(
                pane.visible
                    .iter()
                    .map(|i| pane.entries[*i].name.as_str())
                    .collect::<Vec<_>>(),
                names
            );
            assert_eq!(pane.current().unwrap().name, selected);
            assert!(pane.marked.contains(&selected));
        }
        assert_eq!(pane.revision, revision + 4);
    }
}
