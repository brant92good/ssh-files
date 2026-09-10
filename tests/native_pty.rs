//! Real owned ConPTY / Unix PTY; never opens or focuses a desktop window.
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const F2: &str = "\x1bOQ";
const F3: &str = "\x1bOR";
const F5: &str = "\x1b[15~";
const F6: &str = "\x1b[17~";
const F8: &str = "\x1b[19~";
const F9: &str = "\x1b[20~";
const F10: &str = "\x1b[21~";

#[path = "support/gate.rs"]
mod gate;

struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _pair: portable_pty::PtyPair,
    input: Box<dyn Write + Send>,
    output: mpsc::Receiver<Vec<u8>>,
    parser: vt100::Parser,
    reader: Option<thread::JoinHandle<()>>,
}
impl Session {
    fn start(config: &Path, local: &Path, remote: &str) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 32,
                cols: 130,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let binary = std::env::var_os("SSH_FILES_UI_BINARY")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_ssh-files").into());
        let mut command = CommandBuilder::new(binary);
        command.args([
            "--host",
            "fixture",
            "--config",
            config.to_str().unwrap(),
            "--local",
            local.to_str().unwrap(),
            "--remote",
            remote,
            "--label",
            "Owned fixture",
        ]);
        command.env("TERM", "xterm-256color");
        command.cwd(local);
        let child = pair.slave.spawn_command(command).unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let input = pair.master.take_writer().unwrap();
        let (send, output) = mpsc::sync_channel(64);
        let reader = thread::spawn(move || {
            let mut buffer = [0; 8192];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 || send.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            _pair: pair,
            input,
            output,
            parser: vt100::Parser::new(32, 130, 0),
            reader: Some(reader),
        }
    }
    fn send(&mut self, text: &str) {
        self.input.write_all(text.as_bytes()).unwrap();
        self.input.flush().unwrap();
    }
    fn position(&mut self, text: &str) -> (u16, u16) {
        self.expect(text);
        let (rows, cols) = self.parser.screen().size();
        for row in 0..rows {
            let line: String = (0..cols)
                .map(|column| {
                    let contents = self.parser.screen().cell(row, column).unwrap().contents();
                    if contents.is_empty() { " " } else { contents }
                })
                .collect();
            if let Some(index) = line.find(text) {
                return (line[..index].chars().count() as u16, row);
            }
        }
        panic!("No visible {text:?}: {}", self.parser.screen().contents());
    }
    fn mouse(&mut self, button: u16, position: (u16, u16), release: bool) {
        self.send(&format!(
            "\x1b[<{};{};{}{}",
            button,
            position.0 + 1,
            position.1 + 1,
            if release { 'm' } else { 'M' }
        ));
    }
    fn click(&mut self, button: u16, position: (u16, u16)) {
        self.mouse(button, position, false);
        self.mouse(button, position, true);
    }
    fn resize(&mut self, rows: u16, cols: u16) {
        self._pair
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        self.parser.screen_mut().set_size(rows, cols);
    }
    fn pump(&mut self, timeout: Duration) {
        if let Ok(bytes) = self.output.recv_timeout(timeout) {
            if bytes.windows(4).any(|bytes| bytes == b"\x1b[6n") {
                self.send("\x1b[1;1R");
            }
            self.parser.process(&bytes);
        }
    }
    fn expect(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.parser.screen().contents().contains(text) {
            assert!(
                Instant::now() < deadline,
                "Missing {text:?}: {}",
                self.parser.screen().contents()
            );
            self.pump(Duration::from_millis(20));
        }
    }
    fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            self.pump(Duration::from_millis(20));
        }
    }
    fn clear_filter(&mut self) {
        self.send(F3);
        self.expect("Filter files");
        self.send("\x01\r");
        self.settle();
    }
    fn upload_review(&mut self, source: &Path) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.parser.screen().contents().contains("· loading") {
            assert!(
                Instant::now() < deadline,
                "Directory did not finish loading"
            );
            self.pump(Duration::from_millis(20));
        }
        self.send(F6);
        self.expect("Upload local paths");
        self.send(&format!("\"{}\"\r", source.display()));
        self.settle();
        assert!(
            self.parser
                .screen()
                .contents()
                .contains("Upload local paths")
        );
        self.send(F5);
        self.expect("Review transfers");
    }
    fn exit(&mut self) {
        self.send(F10);
        self.settle();
        if self.parser.screen().contents().contains("Close Files?") {
            self.send(F10);
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status:?}");
                break;
            }
            assert!(Instant::now() < deadline, "Files did not close");
            self.pump(Duration::from_millis(20));
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        // Drain after process exit so the bounded reader can finish. PTY EOF
        // differs on Windows; do not block cleanup on a reader join.
        let deadline = Instant::now() + Duration::from_secs(2);
        while self
            .reader
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
            && Instant::now() < deadline
        {
            self.pump(Duration::from_millis(20));
        }
        if self
            .reader
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
        {
            let _ = self.reader.take().unwrap().join();
        }
    }
}

#[test]
fn owned_terminal_printable_input_and_failed_connection_remain_safe() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    std::fs::write(&config, "Host fixture\n HostName 127.0.0.1\n Port 1\n IdentityAgent none\n UserKnownHostsFile none\n").unwrap();
    std::fs::write(root.path().join("quit upload.txt"), b"unchanged").unwrap();
    let local = root.path().canonicalize().unwrap();
    let mut session = Session::start(&config, &local, ".");
    session.expect("SSH FILES");
    session.expect("quit upload.txt");
    session.send("quit upload.txt\r");
    session.expect("filter: quit upload.txt");
    assert!(session.child.try_wait().unwrap().is_none());
    assert!(session.parser.screen().contents().contains("0 complete"));
    session.exit();
}

#[test]
fn owned_terminal_hidden_toggle_and_literal_filter_editor() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    std::fs::write(&config, "Host fixture\n HostName 127.0.0.1\n Port 1\n IdentityAgent none\n UserKnownHostsFile none\n").unwrap();
    let local = root.path().join("files");
    std::fs::create_dir(&local).unwrap();
    std::fs::write(local.join(".hidden-data"), "private fixture").unwrap();
    std::fs::write(local.join("visible-data"), "visible fixture").unwrap();
    let local = local.canonicalize().unwrap();
    let mut session = Session::start(&config, &local, ".");
    session.expect("visible-data");
    assert!(!session.parser.screen().contents().contains(".hidden-data"));
    session.send(".");
    session.expect(".hidden-data");
    session.expect("hidden on");
    session.send(".");
    session.expect("hidden off");
    assert!(!session.parser.screen().contents().contains(".hidden-data"));
    session.send(F3);
    session.expect("Filter files");
    session.send(".hidden-data");
    session.settle();
    assert!(session.parser.screen().contents().contains(".hidden-data"));
    assert!(session.parser.screen().contents().contains("Filter files"));
    session.send("\x1b");
    session.expect("visible-data");
    assert!(!session.parser.screen().contents().contains(".hidden-data"));
    session.exit();
}

#[test]
fn owned_terminal_sgr_mouse_selects_ranges_scrolls_hovered_pane_and_opens_folder() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    std::fs::write(&config, "Host fixture\n HostName 127.0.0.1\n Port 1\n IdentityAgent none\n UserKnownHostsFile none\n").unwrap();
    let local = root.path().join("files");
    let folder = local.join("a-folder");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("entered-folder-marker"), b"owned fixture").unwrap();
    for index in 0..40 {
        std::fs::write(local.join(format!("item-{index:02}")), b"owned").unwrap();
    }
    let local = local.canonicalize().unwrap();
    let mut session = Session::start(&config, &local, ".");
    session.expect("item-10");
    let two = session.position("item-02");
    let five = session.position("item-05");
    session.click(0, two);
    session.click(4, five); // Shift extends the exact clicked anchor.
    session.expect("4 marked");
    let three = session.position("item-03");
    session.click(16, three); // Ctrl toggles only one row.
    session.expect("3 marked");
    let one = session.position("item-01");
    let four = session.position("item-04");
    session.mouse(0, one, false);
    session.mouse(32, four, false);
    session.expect("4 marked");
    session.mouse(32, (100, four.1), false); // No range crosses to Remote.
    session.mouse(0, (100, four.1), true);
    session.settle();
    assert!(session.parser.screen().contents().contains("4 marked"));
    session.mouse(0, one, false);
    session.mouse(32, five, false);
    // A changed count proves this new drag was consumed before resizing.
    session.expect("5 marked");
    session.resize(12, 40);
    session.expect("at least 60");
    session.mouse(32, (5, 10), false); // A resize ends the old range gesture.
    session.resize(32, 130);
    session.expect("5 marked");
    let eight = session.position("item-08");
    session.mouse(32, eight, false);
    session.mouse(0, four, true);
    session.settle();
    assert!(session.parser.screen().contents().contains("5 marked"));
    let local_before = session.parser.screen().contents();
    session.mouse(65, (100, four.1), false); // Hovered empty Remote, Local stays put.
    session.settle();
    assert_eq!(session.parser.screen().contents(), local_before);
    session.mouse(65, four, false);
    session.expect("item-18");
    assert!(!session.parser.screen().contents().contains("a-folder"));
    // Mouse on a directory editor must not navigate or accept the dialog.
    session.send(F2);
    session.expect("Local directory");
    session.click(0, four);
    session.settle();
    assert!(
        session
            .parser
            .screen()
            .contents()
            .contains("Local directory")
    );
    session.send("\x1b");
    session.expect("item-07");
    for _ in 0..3 {
        session.mouse(64, four, false);
    }
    session.expect("a-folder");
    let directory = session.position("a-folder");
    session.click(0, directory);
    session.click(0, directory);
    session.expect("entered-folder-marker");
    session.exit();
}

#[test]
#[ignore = "Requires explicit disposable SFTP fixture environment; never uses user SSH files"]
fn actual_sftp_mouse_hover_and_remote_folder_navigation_with_hidden_entries() {
    let config = std::env::var_os("SSH_FILES_TEST_CONFIG").expect("SSH_FILES_TEST_CONFIG");
    let server = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_TEST_REMOTE").expect("SSH_FILES_TEST_REMOTE"),
    );
    let target = tempfile::Builder::new()
        .prefix("mouse-owned-")
        .tempdir_in(&server)
        .unwrap();
    let local = tempfile::tempdir().unwrap();
    let local_path = local.path().canonicalize().unwrap();
    std::fs::create_dir(target.path().join("a-remote-folder")).unwrap();
    std::fs::write(
        target.path().join("a-remote-folder/remote-entered-marker"),
        b"remote fixture",
    )
    .unwrap();
    std::fs::write(
        target.path().join("a-remote-folder/.remote-hidden"),
        b"hidden remote fixture",
    )
    .unwrap();
    std::fs::write(local_path.join(".local-hidden"), b"hidden local fixture").unwrap();
    for index in 0..40 {
        std::fs::write(local_path.join(format!("local-{index:02}")), b"local").unwrap();
        std::fs::write(target.path().join(format!("remote-{index:02}")), b"remote").unwrap();
    }
    let remote = target.path().to_str().unwrap().replace('\\', "/");
    let mut session = Session::start(Path::new(&config), &local_path, &remote);
    session.expect("local-10");
    session.expect("remote-10");
    let remote_row = session.position("remote-04");
    session.mouse(65, remote_row, false);
    session.expect("remote-18");
    assert!(session.parser.screen().contents().contains("local-00"));
    assert!(
        !session
            .parser
            .screen()
            .contents()
            .contains("a-remote-folder")
    );
    session.send(F2); // Hovering Remote has not changed Local keyboard focus.
    session.expect("Local directory");
    session.send("\x1b");
    session.expect("remote-18");
    session.mouse(64, remote_row, false);
    session.expect("a-remote-folder");
    let two = session.position("remote-02");
    let five = session.position("remote-05");
    session.click(0, two);
    session.click(4, five);
    session.expect("4 marked");
    let three = session.position("remote-03");
    session.click(16, three);
    session.expect("3 marked");
    let folder = session.position("a-remote-folder");
    session.click(0, folder);
    session.click(0, folder);
    session.expect("remote-entered-marker");
    assert!(
        !session
            .parser
            .screen()
            .contents()
            .contains(".remote-hidden")
    );
    assert!(!session.parser.screen().contents().contains(".local-hidden"));
    session.send(".");
    session.expect(".remote-hidden");
    session.expect(".local-hidden");
    session.send(".");
    session.expect("hidden off");
    assert!(
        !session
            .parser
            .screen()
            .contents()
            .contains(".remote-hidden")
    );
    assert!(!session.parser.screen().contents().contains(".local-hidden"));
    session.exit();
    assert_eq!(
        std::fs::read(target.path().join("a-remote-folder/remote-entered-marker")).unwrap(),
        b"remote fixture"
    );
}

#[test]
#[ignore = "Requires explicit disposable SFTP fixture environment; never uses user SSH files"]
fn actual_sftp_keyboard_paste_review_collisions_recursive_upload_and_download() {
    let config = std::env::var_os("SSH_FILES_TEST_CONFIG").expect("SSH_FILES_TEST_CONFIG");
    let server = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_TEST_REMOTE").expect("SSH_FILES_TEST_REMOTE"),
    );
    let target = tempfile::Builder::new()
        .prefix("ui-owned-")
        .tempdir_in(&server)
        .unwrap();
    let local = tempfile::tempdir().unwrap();
    let source = local.path().join("quit upload 開發.txt");
    let data: Vec<u8> = (0..131_079).map(|n| n as u8).collect();
    std::fs::write(&source, &data).unwrap();
    let remote = target.path().to_str().unwrap().replace('\\', "/");
    let ready_marker = target.path().join("ready-for-upload");
    std::fs::write(&ready_marker, b"fixture ready").unwrap();
    let mut session = Session::start(Path::new(&config), local.path(), &remote);
    session.expect("SSH FILES");
    // Either directory can finish first. Observe one actual entry in each
    // pane instead of assuming a fixed startup delay means both are ready.
    session.expect(source.file_name().unwrap().to_str().unwrap());
    session.expect("ready-for-upload");
    std::fs::remove_file(ready_marker).unwrap();
    session.send(F6);
    session.expect("Upload local paths");
    #[cfg(windows)]
    session.send(&format!("\"{}\"\r", source.display()));
    #[cfg(unix)]
    session.send(&format!("\x1b[200~\"{}\"\n\x1b[201~", source.display()));
    session.settle();
    assert_eq!(
        std::fs::read_dir(target.path()).unwrap().count(),
        0,
        "Multiline key input wrote files before review"
    );
    assert!(
        session
            .parser
            .screen()
            .contents()
            .contains("Upload local paths")
    );
    session.send(F5);
    session.expect("Review transfers");
    session.send("\r");
    session.send(F5);
    session.settle();
    assert_eq!(
        std::fs::read_dir(target.path()).unwrap().count(),
        0,
        "Enter or repeated F5 started a review"
    );
    session.send(F9);
    session.expect("1 complete");
    assert_eq!(
        std::fs::read(target.path().join(source.file_name().unwrap())).unwrap(),
        data
    );
    session.upload_review(&source);
    session.send(F9);
    session.expect("Stopped:");
    assert_eq!(
        std::fs::read(target.path().join(source.file_name().unwrap())).unwrap(),
        data
    );
    session.send(F2);
    session.expect("Destination name");
    session.send("alternate.txt\r");
    session.expect("Review transfers");
    session.send(F9);
    session.expect("2 complete");
    assert_eq!(
        std::fs::read(target.path().join("alternate.txt")).unwrap(),
        data
    );
    session.send("\t");
    session.send(F8);
    session.settle();
    session.send("alternate.txt");
    session.settle();
    session.send(F5);
    session.expect("Review transfers");
    session.send(F2);
    session.expect("Destination name");
    session.send("\x01downloaded.txt\r");
    session.expect("Review transfers");
    session.send(F9);
    session.expect("3 complete");
    assert_eq!(
        std::fs::read(local.path().join("downloaded.txt")).unwrap(),
        data
    );
    session.clear_filter();
    let folder = local.path().join("folder");
    std::fs::create_dir_all(folder.join("nested")).unwrap();
    std::fs::write(folder.join("nested/zero"), b"").unwrap();
    session.upload_review(&folder);
    session.send(F2);
    session.expect("Destination name");
    session.send("\x01renamed\r");
    session.expect("Review transfers");
    session.send(F9);
    session.expect("6 complete");
    assert_eq!(
        std::fs::read(target.path().join("renamed/nested/zero")).unwrap(),
        b""
    );
    assert!(!target.path().join("folder").exists());
    session.exit();
    println!(
        "PASS: actual OS PTY, Unicode multiline key input, explicit review/start, no-clobber pause/rename, downloaded bytes, recursive renamed directory, clean exit"
    );
}

#[test]
#[ignore = "Requires explicit disposable SFTP fixture environment; never uses user SSH files"]
fn actual_stalled_transfer_cancel_close_and_navigation_ownership() {
    use std::sync::atomic::Ordering;
    let config = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_TEST_CONFIG").expect("SSH_FILES_TEST_CONFIG"),
    );
    let server = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_TEST_REMOTE").expect("SSH_FILES_TEST_REMOTE"),
    );
    let original = std::fs::read_to_string(&config).unwrap();
    let port: u16 = original
        .lines()
        .find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == "Port").then(|| words.next().unwrap().parse().unwrap())
        })
        .unwrap();
    let public = std::fs::read_to_string(config.parent().unwrap().join("host.pub")).unwrap();
    let modes = if cfg!(windows) {
        vec!["cancel", "close", "abrupt-process-exit"]
    } else {
        vec!["cancel", "close"]
    };
    for mode in modes {
        let target = tempfile::Builder::new()
            .prefix("ui-stall-")
            .tempdir_in(&server)
            .unwrap();
        let local = tempfile::tempdir().unwrap();
        let source = local.path().join("large.bin");
        std::fs::write(&source, vec![0x55; 32 * 1024 * 1024]).unwrap();
        let gate = gate::Gate::start(port);
        let known = local.path().join("known_hosts");
        std::fs::write(&known, format!("fixture-key {public}")).unwrap();
        let selected_config = local.path().join("config");
        let text = original
            .lines()
            .map(|line| match line.split_whitespace().next() {
                Some("Port") => format!(" Port {}", gate.port),
                Some("UserKnownHostsFile") => format!(
                    " UserKnownHostsFile \"{}\"",
                    known.to_str().unwrap().replace('\\', "/")
                ),
                _ => line.into(),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n HostKeyAlias fixture-key\n";
        std::fs::write(&selected_config, text).unwrap();
        let remote = target.path().to_str().unwrap().replace('\\', "/");
        let mut session = Session::start(&selected_config, local.path(), &remote);
        session.expect("SSH FILES");
        session.expect("Choose files");
        session.settle();
        session.upload_review(&source);
        session.send(F9);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if std::fs::read_dir(target.path()).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".ssh-files-")
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "No actual transfer partial: {}",
                session.parser.screen().contents()
            );
            session.pump(Duration::from_millis(1));
        }
        gate.pause_transfer.store(true, Ordering::Release);
        session.settle();
        assert!(
            !target.path().join("large.bin").exists(),
            "Fixture did not pause before finalization"
        );
        // Browser has its own first transport. It must remain usable while
        // only the second transfer transport is paused.
        let marker = format!("after-pause-{}", uuid::Uuid::new_v4().simple());
        std::fs::write(target.path().join(&marker), b"new after pause").unwrap();
        session.send("\t");
        session.send(F8);
        session.expect(&marker);
        assert_eq!(gate.active.load(Ordering::Acquire), 2);
        gate.assert_clean();
        let started = Instant::now();
        gate.cancelling.store(true, Ordering::Release);
        match mode {
            "cancel" => {
                session.send("\x03");
                session.expect("Stopped:");
                session.exit();
            }
            "close" => session.exit(),
            _ => {
                session.child.kill().unwrap();
            }
        }
        gate.pause_transfer.store(false, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while gate.active.load(Ordering::Acquire) != 0 {
            assert!(
                Instant::now() < deadline,
                "{mode} left owned SSH transport alive"
            );
            session.pump(Duration::from_millis(20));
        }
        assert!(!target.path().join("large.bin").exists());
        gate.assert_clean();
        println!(
            "PASS: {mode} stopped both owned SSH transports in {:.2}s; browser navigation remained live, interrupted final absent",
            started.elapsed().as_secs_f64()
        );
        drop(session);
    }
}
