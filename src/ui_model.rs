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
        }
    }
    pub fn replace(&mut self, listing: Listing) {
        self.path = listing.path;
        self.entries = listing.entries;
        self.marked.clear();
        self.refilter();
    }
    pub fn refilter(&mut self) {
        let query = paths::collision_key(&self.filter);
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| paths::collision_key(&entry.name).contains(&query))
            .map(|(index, _)| index)
            .collect();
        self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
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
