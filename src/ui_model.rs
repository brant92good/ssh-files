use crate::{
    browser::{Entry, Listing},
    paths,
};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

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
    Review {
        jobs: Vec<crate::plan::Job>,
        cursor: usize,
    },
    Edit {
        kind: EditKind,
        text: String,
    },
    Help,
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
}
