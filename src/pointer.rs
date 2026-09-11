//! Mouse gestures operate only on the last rendered, versioned file lists.
use crate::{browser::Kind, paths, ui_model::Pane};
use anyhow::{Result, ensure};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PaneGeometry {
    pub list: Rect,
    pub start: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub area: Rect,
    pub panes: [PaneGeometry; 2],
    pub revisions: [u64; 2],
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Anchor {
    pane: usize,
    directory: String,
    name: String,
    revision: u64,
}
impl Anchor {
    fn new(pane: usize, data: &Pane) -> Option<Self> {
        let entry = data.current()?;
        entry.rejected.is_none().then(|| Self {
            pane,
            directory: data.path.clone(),
            name: entry.name.clone(),
            revision: data.revision,
        })
    }
    fn row(&self, pane: usize, data: &Pane) -> Option<usize> {
        if self.pane != pane || self.directory != data.path || self.revision != data.revision {
            return None;
        }
        data.visible.iter().position(|index| {
            let entry = &data.entries[*index];
            entry.name == self.name && entry.rejected.is_none()
        })
    }
}
struct Drag {
    anchor: Anchor,
    initial: BTreeSet<String>,
}
#[derive(Default)]
pub(crate) struct Pointer {
    geometry: Option<Geometry>,
    anchor: Option<Anchor>,
    drag: Option<Drag>,
    click: Option<(Anchor, Instant)>,
}
impl Pointer {
    pub fn clear(&mut self) {
        self.anchor = None;
        self.drag = None;
        self.click = None;
    }
    pub fn sync(&mut self, geometry: Option<Geometry>, panes: [&Pane; 2], modal: bool) {
        if modal
            || geometry.is_none()
            || self.geometry.is_some_and(|old| {
                geometry.is_some_and(|new| {
                    old.area != new.area
                        || old
                            .panes
                            .iter()
                            .zip(new.panes)
                            .any(|(a, b)| a.list != b.list)
                })
            })
        {
            self.clear();
        }
        if self
            .anchor
            .as_ref()
            .is_some_and(|a| a.row(a.pane, panes[a.pane]).is_none())
        {
            self.clear();
        }
        // A scrolling list invalidates double-click coordinates, but a drag may
        // deliberately scroll by wheel while retaining its versioned anchor.
        if self.geometry != geometry {
            self.click = None;
            if self.drag.is_none() {
                self.anchor = None;
            }
        }
        self.geometry = geometry;
    }
    pub fn event(
        &mut self,
        event: MouseEvent,
        mut panes: [&mut Pane; 2],
        focus: &mut usize,
        now: Instant,
    ) -> Result<Option<usize>> {
        let Some(geometry) = self.geometry else {
            return Ok(None);
        };
        if panes
            .iter()
            .enumerate()
            .any(|(index, pane)| pane.revision != geometry.revisions[index])
        {
            self.clear();
            return Ok(None);
        }
        let point = (event.column, event.row).into();
        let hovered = geometry
            .panes
            .iter()
            .position(|pane| pane.list.contains(point));
        match event.kind {
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let Some(pane) = hovered else {
                    return Ok(None);
                };
                if self
                    .drag
                    .as_ref()
                    .is_some_and(|drag| drag.anchor.pane != pane)
                {
                    return Ok(None);
                }
                let amount = if event.kind == MouseEventKind::ScrollUp {
                    -3
                } else {
                    3
                };
                let data = &mut panes[pane];
                data.move_by(amount);
                data.offset = data.offset.saturating_add_signed(amount);
                self.click = None;
                if let Some(drag) = &self.drag {
                    range(data, pane, &drag.anchor, data.cursor, &drag.initial)?;
                } else {
                    self.anchor = None;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(pane) = hovered else {
                    self.clear();
                    return Ok(None);
                };
                let data = &mut panes[pane];
                let row = geometry.panes[pane].start
                    + usize::from(event.row - geometry.panes[pane].list.y);
                if row >= data.visible.len() {
                    self.clear();
                    return Ok(None);
                }
                *focus = pane;
                data.cursor = row;
                let Some(anchor) = Anchor::new(pane, data) else {
                    self.clear();
                    return Ok(None);
                };
                let control = event.modifiers.contains(KeyModifiers::CONTROL);
                let shift = event.modifiers.contains(KeyModifiers::SHIFT);
                if event.modifiers.contains(KeyModifiers::ALT) {
                    self.clear();
                    return Ok(None);
                }
                if shift {
                    let start = self
                        .anchor
                        .clone()
                        .filter(|a| a.row(pane, data).is_some())
                        .unwrap_or_else(|| anchor.clone());
                    let initial = if control {
                        data.marked.clone()
                    } else {
                        BTreeSet::new()
                    };
                    range(data, pane, &start, row, &initial)?;
                    self.drag = Some(Drag {
                        anchor: start.clone(),
                        initial,
                    });
                    self.anchor = Some(start);
                    self.click = None;
                } else if control {
                    let initial = data.marked.clone();
                    data.toggle()?;
                    self.drag = Some(Drag {
                        anchor: anchor.clone(),
                        initial,
                    });
                    self.anchor = Some(anchor);
                    self.click = None;
                } else {
                    let double = self.click.as_ref().is_some_and(|(last, time)| {
                        *last == anchor
                            && now.saturating_duration_since(*time) <= Duration::from_millis(400)
                    });
                    data.marked.clear();
                    self.anchor = Some(anchor.clone());
                    if double {
                        self.click = None;
                        self.drag = None;
                        if data
                            .current()
                            .is_some_and(|entry| entry.kind == Kind::Directory)
                        {
                            return Ok(Some(pane));
                        }
                    } else {
                        self.click = Some((anchor.clone(), now));
                        self.drag = Some(Drag {
                            anchor,
                            initial: BTreeSet::new(),
                        });
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(drag) = &self.drag else {
                    return Ok(None);
                };
                let pane = drag.anchor.pane;
                let area = geometry.panes[pane].list;
                // Vertical movement may reach the list edge, never the other pane.
                if event.column < area.x || event.column >= area.right() || area.height == 0 {
                    return Ok(None);
                }
                let data = &mut panes[pane];
                if data.visible.is_empty() {
                    return Ok(None);
                }
                let y = event.row.clamp(area.y, area.bottom() - 1);
                let row = (geometry.panes[pane].start + usize::from(y - area.y))
                    .min(data.visible.len() - 1);
                range(data, pane, &drag.anchor, row, &drag.initial)?;
                data.cursor = row;
                self.click = None;
            }
            _ => {}
        }
        Ok(None)
    }
}

fn range(
    data: &mut Pane,
    pane: usize,
    anchor: &Anchor,
    end: usize,
    initial: &BTreeSet<String>,
) -> Result<()> {
    let Some(start) = anchor.row(pane, data) else {
        return Ok(());
    };
    let mut marked = initial.clone();
    for row in start.min(end)..=start.max(end) {
        let entry = &data.entries[data.visible[row]];
        if entry.rejected.is_none() {
            marked.insert(entry.name.clone());
        }
        ensure!(
            marked.len() <= paths::QUEUE_LIMIT,
            "Select at most 1,000 entries; selection was unchanged"
        );
    }
    data.marked = marked;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{Entry, Listing};

    fn panes(count: usize) -> [Pane; 2] {
        ["/local", "/remote"].map(|path| {
            let mut pane = Pane::new(path.into());
            pane.replace(Listing {
                inspected: count,
                path: path.into(),
                entries: (0..count)
                    .map(|i| Entry {
                        name: format!("file-{i:04}"),
                        kind: if i == 0 { Kind::Directory } else { Kind::File },
                        size: 0,
                        hidden: false,
                        rejected: None,
                    })
                    .collect(),
            });
            pane
        })
    }
    fn sync(pointer: &mut Pointer, panes: &mut [Pane; 2]) -> Geometry {
        let geometry =
            crate::ui_render::mouse_geometry(Rect::new(0, 0, 100, 30), &panes[0], &panes[1])
                .unwrap();
        for (index, pane) in panes.iter_mut().enumerate() {
            pane.offset = geometry.panes[index].start;
        }
        pointer.sync(Some(geometry), [&panes[0], &panes[1]], false);
        geometry
    }
    fn event(
        pointer: &mut Pointer,
        panes: &mut [Pane; 2],
        focus: &mut usize,
        kind: MouseEventKind,
        position: (u16, u16),
        modifiers: KeyModifiers,
        time: Instant,
    ) -> Result<Option<usize>> {
        let [left, right] = panes;
        pointer.event(
            MouseEvent {
                kind,
                column: position.0,
                row: position.1,
                modifiers,
            },
            [left, right],
            focus,
            time,
        )
    }
    fn position(geometry: Geometry, pane: usize, row: u16) -> (u16, u16) {
        (
            geometry.panes[pane].list.x + 2,
            geometry.panes[pane].list.y + row,
        )
    }
    #[test]
    fn wheel_changes_only_hovered_list_and_header_is_ignored() {
        let mut panes = panes(40);
        let mut pointer = Pointer::default();
        let mut focus = 0;
        let geometry = sync(&mut pointer, &mut panes);
        let now = Instant::now();
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::ScrollDown,
            position(geometry, 1, 2),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        assert_eq!((panes[0].cursor, panes[1].cursor, focus), (0, 3, 0));
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::ScrollDown,
            (geometry.panes[0].list.x, geometry.panes[0].list.y - 1),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        assert_eq!(panes[0].cursor, 0);
        assert!(
            crate::ui_render::mouse_geometry(Rect::new(0, 0, 40, 12), &panes[0], &panes[1])
                .is_none()
        );
    }
    #[test]
    fn click_ctrl_shift_and_drag_share_stable_rows_without_crossing_panes() {
        let mut panes = panes(40);
        let mut pointer = Pointer::default();
        let mut focus = 1;
        let geometry = sync(&mut pointer, &mut panes);
        let now = Instant::now();
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Down(MouseButton::Left),
            position(geometry, 0, 2),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Up(MouseButton::Left),
            position(geometry, 0, 2),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        assert_eq!((focus, panes[0].cursor), (0, 2));
        assert_eq!(sync(&mut pointer, &mut panes).panes[0].start, 0);
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Down(MouseButton::Left),
            position(geometry, 0, 5),
            KeyModifiers::SHIFT,
            now,
        )
        .unwrap();
        assert_eq!(panes[0].marked.len(), 4);
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Drag(MouseButton::Left),
            position(geometry, 1, 7),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        assert_eq!((panes[0].marked.len(), panes[1].marked.len()), (4, 0));
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Drag(MouseButton::Left),
            position(geometry, 0, 7),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        assert_eq!(panes[0].marked.len(), 6);
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Up(MouseButton::Left),
            position(geometry, 0, 7),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Down(MouseButton::Left),
            position(geometry, 0, 4),
            KeyModifiers::CONTROL,
            now,
        )
        .unwrap();
        assert_eq!(panes[0].marked.len(), 5);
        assert!(!panes[0].marked.contains("file-0004"));
    }
    #[test]
    fn double_click_needs_same_safe_path_revision_and_plain_button() {
        let mut panes = panes(5);
        let mut pointer = Pointer::default();
        let mut focus = 0;
        let geometry = sync(&mut pointer, &mut panes);
        let now = Instant::now();
        let pos = position(geometry, 1, 0);
        let down = MouseEventKind::Down(MouseButton::Left);
        assert_eq!(
            event(
                &mut pointer,
                &mut panes,
                &mut focus,
                down,
                pos,
                KeyModifiers::NONE,
                now
            )
            .unwrap(),
            None
        );
        assert_eq!(
            event(
                &mut pointer,
                &mut panes,
                &mut focus,
                down,
                pos,
                KeyModifiers::NONE,
                now + Duration::from_millis(100)
            )
            .unwrap(),
            Some(1)
        );
        pointer.clear();
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            down,
            pos,
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        panes[1].refilter();
        assert_eq!(
            event(
                &mut pointer,
                &mut panes,
                &mut focus,
                down,
                pos,
                KeyModifiers::NONE,
                now
            )
            .unwrap(),
            None
        );
        sync(&mut pointer, &mut panes);
        for modifier in [KeyModifiers::CONTROL, KeyModifiers::SHIFT] {
            pointer.clear();
            for _ in 0..2 {
                assert_eq!(
                    event(
                        &mut pointer,
                        &mut panes,
                        &mut focus,
                        down,
                        pos,
                        modifier,
                        now
                    )
                    .unwrap(),
                    None
                );
            }
        }
        panes[1].entries[0].rejected = Some("unsafe".into());
        pointer.clear();
        for _ in 0..2 {
            assert_eq!(
                event(
                    &mut pointer,
                    &mut panes,
                    &mut focus,
                    down,
                    pos,
                    KeyModifiers::NONE,
                    now
                )
                .unwrap(),
                None
            );
        }
    }
    #[test]
    fn range_limit_is_atomic_and_rejected_entries_are_excluded() {
        let mut panes = panes(1002);
        let pane = &mut panes[0];
        let anchor = Anchor::new(0, pane).unwrap();
        pane.marked.insert("file-0005".into());
        let previous = pane.marked.clone();
        assert!(range(pane, 0, &anchor, 1001, &BTreeSet::new()).is_err());
        assert_eq!(pane.marked, previous);
        pane.entries[1].rejected = Some("unsafe".into());
        range(pane, 0, &anchor, 3, &BTreeSet::new()).unwrap();
        assert_eq!(pane.marked.len(), 3);
        assert!(!pane.marked.contains("file-0001"));
    }
    #[test]
    fn modal_resize_filter_and_mouse_up_end_drag_ownership() {
        let mut panes = panes(40);
        let mut pointer = Pointer::default();
        let mut focus = 0;
        let geometry = sync(&mut pointer, &mut panes);
        let now = Instant::now();
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Down(MouseButton::Left),
            position(geometry, 0, 1),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        pointer.sync(Some(geometry), [&panes[0], &panes[1]], true);
        assert!(pointer.drag.is_none() && pointer.anchor.is_none());
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Down(MouseButton::Left),
            position(geometry, 0, 1),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        let resized =
            crate::ui_render::mouse_geometry(Rect::new(0, 0, 120, 32), &panes[0], &panes[1]);
        pointer.sync(resized, [&panes[0], &panes[1]], false);
        assert!(pointer.drag.is_none());
        sync(&mut pointer, &mut panes);
        event(
            &mut pointer,
            &mut panes,
            &mut focus,
            MouseEventKind::Down(MouseButton::Left),
            position(geometry, 0, 1),
            KeyModifiers::NONE,
            now,
        )
        .unwrap();
        panes[0].filter = "0002".into();
        panes[0].refilter();
        sync(&mut pointer, &mut panes);
        assert!(pointer.drag.is_none());
    }
}
