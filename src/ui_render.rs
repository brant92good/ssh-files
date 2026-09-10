use crate::{
    browser::Kind,
    paths,
    plan::Direction,
    ui::App,
    ui_model::{EditKind, Modal, Pane},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap},
};

const INK: Color = Color::Rgb(216, 225, 239);
const DIM: Color = Color::Rgb(139, 158, 183);
const BLUE: Color = Color::Rgb(112, 185, 255);
const GREEN: Color = Color::Rgb(106, 220, 173);
const RED: Color = Color::Rgb(255, 153, 145);
const BACK: Color = Color::Rgb(18, 24, 35);
#[cfg(test)]
mod screenshots;

fn bytes(size: u64) -> String {
    if size >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}
fn border(title: impl Into<String>, active: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", title.into()))
        .border_style(Style::default().fg(if active { BLUE } else { DIM }))
}
fn pane_parts(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(border("", false).inner(area))
}
fn layout(area: Rect) -> Option<std::rc::Rc<[Rect]>> {
    (area.width >= 60 && area.height >= 16).then(|| {
        Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(4),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(area)
    })
}
pub(crate) fn mouse_geometry(
    area: Rect,
    local: &Pane,
    remote: &Pane,
) -> Option<crate::pointer::Geometry> {
    let parts = layout(area)?;
    let panes = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(parts[1]);
    Some(crate::pointer::Geometry {
        area,
        panes: std::array::from_fn(|index| {
            let pane = [local, remote][index];
            let list = pane_parts(panes[index])[1];
            crate::pointer::PaneGeometry {
                list,
                start: pane.view_start(usize::from(list.height)),
            }
        }),
        revisions: [local.revision, remote.revision],
    })
}
fn pane(frame: &mut Frame, area: Rect, pane: &Pane, title: &str, active: bool, busy: bool) {
    let block = border(
        format!("{title}{}", if busy { " · loading" } else { "" }),
        active,
    );
    frame.render_widget(block, area);
    let parts = pane_parts(area);
    frame.render_widget(
        Paragraph::new(paths::display(&pane.path))
            .style(Style::default().fg(BLUE))
            .wrap(Wrap { trim: false }),
        parts[0],
    );
    let height = usize::from(parts[1].height);
    let start = pane.view_start(height);
    let items: Vec<_> = pane
        .visible
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(row, index)| {
            let entry = &pane.entries[*index];
            let mark = if pane.marked.contains(&entry.name) {
                "●"
            } else {
                " "
            };
            let kind = if entry.rejected.is_some() {
                "!"
            } else if entry.kind == Kind::Directory {
                "/"
            } else {
                " "
            };
            let name = format!("{mark} {kind} {}", paths::display(&entry.name));
            let size = if entry.kind == Kind::Directory {
                "folder".into()
            } else {
                bytes(entry.size)
            };
            let style = Style::default().fg(if entry.rejected.is_some() {
                RED
            } else if entry.kind == Kind::Directory {
                BLUE
            } else {
                INK
            });
            let style = if row == pane.cursor {
                style
                    .bg(Color::Rgb(38, 53, 72))
                    .add_modifier(Modifier::BOLD)
            } else {
                style
            };
            ListItem::new(Line::from(vec![
                Span::styled(name, style),
                Span::styled(format!("  {size}"), style.fg(DIM)),
            ]))
            .style(style)
        })
        .collect();
    frame.render_widget(List::new(items), parts[1]);
    frame.render_widget(
        Paragraph::new(format!(
            "{} shown · {} marked · hidden {}{}",
            pane.visible.len(),
            pane.marked.len(),
            if pane.show_hidden { "on" } else { "off" },
            if pane.filter.is_empty() {
                String::new()
            } else {
                format!(" · filter: {}", paths::display(&pane.filter))
            }
        ))
        .style(Style::default().fg(DIM)),
        parts[2],
    );
}

pub(crate) fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().fg(INK).bg(BACK)),
        area,
    );
    let Some(parts) = layout(area) else {
        frame.render_widget(Paragraph::new("SSH Files\nMake this terminal at least 60 columns × 16 rows.\nF10 closes; Esc cancels active work.").wrap(Wrap { trim: false }), area);
        return;
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " SSH FILES ",
                    Style::default()
                        .fg(BACK)
                        .bg(BLUE)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {}", paths::display(&app.options.label))),
            ]),
            Line::from(Span::styled(
                format!("  Route: {}", paths::display(&app.options.route.host)),
                Style::default().fg(DIM),
            )),
        ]),
        parts[0],
    );
    let panes = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(parts[1]);
    pane(
        frame,
        panes[0],
        &app.local,
        "Local",
        app.pane == 0,
        app.local_job.is_some(),
    );
    pane(
        frame,
        panes[1],
        &app.remote,
        "Remote",
        app.pane == 1,
        app.remote_job.is_some(),
    );
    let block = border(
        format!(
            "Transfers · {} complete · {} waiting",
            app.completed,
            app.queue.len()
        ),
        false,
    );
    let inner = block.inner(parts[2]);
    frame.render_widget(block, parts[2]);
    if let Some(job) = app.active.as_ref().or(app.failed.as_ref()) {
        let title = format!(
            "{} {} → {}",
            if job.direction == Direction::Upload {
                "↑"
            } else {
                "↓"
            },
            paths::display(&job.source),
            paths::display(&job.destination)
        );
        frame.render_widget(Paragraph::new(title), Rect { height: 1, ..inner });
        let outcome = app.progress.as_ref();
        let done = outcome.map(|p| p.bytes).unwrap_or(0);
        let total = job.bytes;
        let ratio = if total == 0 {
            0.0
        } else {
            (done as f64 / total as f64).clamp(0.0, 1.0)
        };
        let phase = if app.failed.is_some() {
            "paused"
        } else {
            outcome.map(|p| p.phase).unwrap_or("preparing")
        };
        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .gauge_style(Style::default().fg(if app.failed.is_some() { RED } else { GREEN }))
                .label(format!("{phase} · {} / {}", bytes(done), bytes(total))),
            Rect {
                y: inner.y + 1,
                height: inner.height.saturating_sub(1),
                ..inner
            },
        );
    } else {
        frame.render_widget(
            Paragraph::new("Select files in either pane. F5 reviews; F9 starts after review.")
                .style(Style::default().fg(DIM)),
            inner,
        );
    }
    frame.render_widget(
        Paragraph::new(paths::display(&app.status))
            .style(Style::default().fg(if app.safety.uncertain { RED } else { INK }))
            .wrap(Wrap { trim: false }),
        parts[3],
    );
    frame.render_widget(Paragraph::new(" Tab switch   Enter open   Space mark   F5 review   . hidden   Esc stop\n F2 path   F3 filter   F4 details   F6 paste paths   F1 help   F10 quit").style(Style::default().fg(DIM)), parts[4]);
    if let Some(modal) = &app.modal {
        draw_modal(frame, modal);
    }
}

fn popup(frame: &mut Frame, title: &str) -> Rect {
    let area = frame.area();
    let width = area.width.saturating_sub(6).min(110);
    let height = area.height.saturating_sub(4);
    let area = Rect::new(
        (area.width - width) / 2,
        (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let block = border(title, true).style(Style::default().bg(BACK));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}
fn draw_modal(frame: &mut Frame, modal: &Modal) {
    match modal {
        Modal::Review { jobs, cursor } => {
            let area = popup(frame, "Review transfers");
            let parts = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(area);
            frame.render_widget(
                Paragraph::new(format!(
                    "{} items · {}\nExisting files will be kept. A collision pauses the queue.",
                    jobs.len(),
                    bytes(jobs.iter().map(|j| j.bytes).fold(0u64, u64::saturating_add))
                ))
                .style(Style::default().fg(DIM)),
                parts[0],
            );
            let height = usize::from(parts[1].height) / 2;
            let start = cursor.saturating_sub(height.saturating_sub(1));
            let items: Vec<_> = jobs
                .iter()
                .enumerate()
                .skip(start)
                .take(height)
                .map(|(index, job)| {
                    let style = Style::default().fg(INK).bg(if index == *cursor {
                        Color::Rgb(38, 53, 72)
                    } else {
                        BACK
                    });
                    ListItem::new(vec![
                        Line::from(format!(
                            "{} {}",
                            if job.directory {
                                "DIR"
                            } else if job.direction == Direction::Upload {
                                "UP "
                            } else {
                                "GET"
                            },
                            paths::display(&job.source)
                        )),
                        Line::from(Span::styled(
                            format!("  → {}", paths::display(&job.destination)),
                            Style::default().fg(BLUE),
                        )),
                    ])
                    .style(style)
                })
                .collect();
            frame.render_widget(List::new(items), parts[1]);
            frame.render_widget(Paragraph::new("↑↓ select   F2 change destination name   Delete skip\nF9 / Ctrl+S start transfers   Esc cancel review\nEnter does not start a transfer.").style(Style::default().fg(GREEN)), parts[2]);
        }
        Modal::Edit { kind, text } => {
            let title = match kind {
                EditKind::LocalPath => "Local directory",
                EditKind::RemotePath => "Remote directory",
                EditKind::LocalFilter | EditKind::RemoteFilter => "Filter files",
                EditKind::Paste => "Upload local paths",
                EditKind::Rename => "Destination name",
            };
            let area = popup(frame, title);
            let parts = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(area);
            let hint = if matches!(kind, EditKind::Paste) {
                "Paste paths here: one per line, or quote each path with spaces.\nNothing is uploaded until the separate transfer review.\nPress F5 to prepare that review."
            } else {
                "Edit the value below. Enter or F5 applies it.\nCtrl+A clears this field; Backspace removes a character."
            };
            frame.render_widget(
                Paragraph::new(hint)
                    .style(Style::default().fg(DIM))
                    .wrap(Wrap { trim: false }),
                parts[0],
            );
            frame.render_widget(
                Paragraph::new(
                    text.lines()
                        .map(paths::display)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
                .style(Style::default().fg(BLUE))
                .wrap(Wrap { trim: false }),
                parts[1],
            );
            frame.render_widget(Paragraph::new("Esc back   Ctrl+A clear\nPaths are literal text; shell syntax is never executed.").style(Style::default().fg(DIM)), parts[2]);
        }
        Modal::Help => {
            let area = popup(frame, "Keyboard");
            frame.render_widget(Paragraph::new("Tab                 Switch local / remote pane\nArrows, Enter       Select / open a folder\nBackspace           Remove filter text, otherwise go up\nType                Filter the current pane\n.                   Show/hide hidden files (empty filter)\nSpace               Mark or unmark an entry\nMouse wheel         Scroll the pane under the pointer\nClick / double-click  Select / open a folder\nCtrl-click, Shift-click, drag   Toggle / select a range\nF2                  Enter a directory path\nF3                  Edit / clear the filter\nF4                  Connection details and retained paths\nF5                  Review selected uploads / downloads\nF6                  Paste local paths for upload\nF7                  Request copy path (terminal clipboard)\nF8                  Refresh / reconnect browser\nF9 or Ctrl+S        Start only from transfer review\nEsc or Ctrl+C       Cancel active work and queued items\nF10 or Ctrl+Q       Close Files (confirm active transfers)\n\n! marks unsupported entries. F5 reviews before F9 transfers.\nExisting final files are kept. Esc closes this help.").wrap(Wrap { trim: false }), area);
        }
        Modal::Quit => {
            let area = popup(frame, "Close Files?");
            frame.render_widget(Paragraph::new("A transfer or queue is active. Closing stops both owned SSH sessions.\n\nCompleted files remain. Interrupted work may leave a partial file;\nits path is printed when Files closes. An uncertain finalization\nmust be checked before retrying.\n\nF10 or Ctrl+Q again: stop and close\nEsc: keep working").wrap(Wrap { trim: false }), area);
        }
        Modal::Details { body, scroll } => {
            let area = popup(frame, "Details · arrows scroll · Esc back");
            frame.render_widget(
                Paragraph::new(body.as_str())
                    .wrap(Wrap { trim: false })
                    .scroll((*scroll, 0)),
                area,
            );
        }
    }
}
