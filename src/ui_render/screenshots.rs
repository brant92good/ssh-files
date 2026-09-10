//! Actual native widget output with public demonstration names; no SSH/files.
use super::*;
use crate::{
    Route,
    browser::{Entry, Listing},
    plan::Job,
    ui::Options,
};
use std::{fmt::Write, path::Path};

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn color(color: Color, fallback: &str) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => fallback.into(),
    }
}
fn save(app: &App, path: &Path, title: &str) {
    let (columns, rows) = (112u16, 29u16);
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(columns, rows)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let (width, height) = (columns as u32 * 9 + 32, rows as u32 * 19 + 32);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\"><title>{}</title><rect width=\"100%\" height=\"100%\" rx=\"12\" fill=\"#121823\"/><g font-family=\"Consolas, 'DejaVu Sans Mono', monospace\" font-size=\"15\" xml:space=\"preserve\">",
        xml(title)
    );
    let buffer = terminal.backend().buffer();
    for y in 0..rows {
        for x in 0..columns {
            let cell = &buffer[(x, y)];
            let (px, py) = (u32::from(x) * 9 + 16, u32::from(y) * 19 + 16);
            write!(
                svg,
                "<rect x=\"{px}\" y=\"{py}\" width=\"9\" height=\"19\" fill=\"{}\"/>",
                color(cell.bg, "#121823")
            )
            .unwrap();
            if !cell.symbol().trim().is_empty() {
                write!(
                    svg,
                    "<text x=\"{px}\" y=\"{}\" fill=\"{}\" font-weight=\"{}\">{}</text>",
                    py + 15,
                    color(cell.fg, "#d8e1ef"),
                    if cell.modifier.contains(Modifier::BOLD) {
                        700
                    } else {
                        400
                    },
                    xml(cell.symbol())
                )
                .unwrap();
            }
        }
    }
    svg.push_str("</g></svg>\n");
    std::fs::write(path, svg).unwrap();
}
fn entry(name: &str, kind: Kind, size: u64) -> Entry {
    Entry {
        name: name.into(),
        kind,
        size,
        hidden: name.starts_with('.'),
        rejected: None,
    }
}

#[test]
#[ignore = "Explicit native-widget screenshot export; writes only requested output directory"]
fn capture_native_widgets() {
    let output = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_SCREENSHOTS").expect("Set screenshot output directory"),
    );
    std::fs::create_dir_all(&output).unwrap();
    let mut app = App::new(Options {
        route: Route {
            host: "devbox".into(),
            hostname: None,
            config: None,
            user: None,
            port: None,
        },
        label: "Development server".into(),
        machine_id: String::new(),
        route_id: "home-lan".into(),
        local: "/home/sam/project".into(),
        remote: "/srv/project".into(),
    })
    .unwrap();
    app.local.replace(Listing {
        path: "/home/sam/project".into(),
        inspected: 6,
        entries: vec![
            entry("assets", Kind::Directory, 0),
            entry("src", Kind::Directory, 0),
            entry("app.toml", Kind::File, 1248),
            entry("model.safetensors", Kind::File, 512 * 1024 * 1024),
            entry("README.md", Kind::File, 3456),
            entry("results.csv", Kind::File, 18340),
        ],
    });
    app.remote.replace(Listing {
        path: "/srv/project".into(),
        inspected: 5,
        entries: vec![
            entry("checkpoints", Kind::Directory, 0),
            entry("logs", Kind::Directory, 0),
            entry("outputs", Kind::Directory, 0),
            entry("metrics.json", Kind::File, 2168),
            entry("train.py", Kind::File, 5930),
        ],
    });
    app.local.cursor = 3;
    app.local.marked.insert("model.safetensors".into());
    app.status =
        "Choose files in either pane. Review destinations before starting a transfer.".into();
    save(
        &app,
        &output.join("browser.svg"),
        "SSH Files: local and remote file panes rendered by the native application",
    );
    app.modal = Some(Modal::Review {
        jobs: vec![
            Job {
                direction: Direction::Upload,
                directory: false,
                source: "/home/sam/project/model.safetensors".into(),
                destination: "/srv/project/model.safetensors".into(),
                bytes: 512 * 1024 * 1024,
            },
            Job {
                direction: Direction::Upload,
                directory: false,
                source: "/home/sam/project/app.toml".into(),
                destination: "/srv/project/app.toml".into(),
                bytes: 1248,
            },
        ],
        cursor: 0,
    });
    save(
        &app,
        &output.join("review.svg"),
        "SSH Files: actual transfer review with explicit F9 start and destination rename",
    );
    app.modal = None;
    app.active = Some(Job {
        direction: Direction::Upload,
        directory: false,
        source: "/home/sam/project/model.safetensors".into(),
        destination: "/srv/project/model.safetensors".into(),
        bytes: 512 * 1024 * 1024,
    });
    app.progress = Some(crate::Outcome {
        phase: "transferring",
        bytes: 187 * 1024 * 1024,
        ..crate::Outcome::default()
    });
    app.completed = 2;
    app.status = "Browse either pane while the transfer runs. Esc stops the queue.".into();
    save(
        &app,
        &output.join("transfer.svg"),
        "SSH Files: native transfer progress and independent browser panes",
    );
}
