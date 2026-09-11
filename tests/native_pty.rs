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
    fn expect_absent(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.parser.screen().contents().contains(text) {
            assert!(
                Instant::now() < deadline,
                "Still visible {text:?}: {}",
                self.parser.screen().contents()
            );
            self.pump(Duration::from_millis(20));
        }
    }
    fn clear_filter(&mut self) {
        self.filter("");
    }
    fn filter(&mut self, text: &str) {
        self.send(F3);
        self.expect("Filter files");
        self.send(&format!("\x01{text}\r"));
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
    session.expect(env!("CARGO_PKG_VERSION"));
    if env!("CARGO_PKG_VERSION").contains("-beta.") {
        session.expect("BETA");
    }
    session.expect("quit upload.txt");
    session.send("quit upload.txt\r");
    session.expect("Upload local paths");
    session.expect("quit upload.txt");
    assert!(
        !session
            .parser
            .screen()
            .contents()
            .contains("filter: quit upload.txt")
    );
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
fn owned_terminal_select_all_sort_and_create_folder_use_actual_keys() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    std::fs::write(&config, "Host fixture\n HostName 127.0.0.1\n Port 1\n IdentityAgent none\n UserKnownHostsFile none\n").unwrap();
    let local = root.path().join("files");
    std::fs::create_dir(&local).unwrap();
    std::fs::write(local.join("aaa-small"), b"x").unwrap();
    std::fs::write(local.join("zzz-large"), b"longer contents").unwrap();
    std::fs::write(local.join(".hidden"), b"keep hidden").unwrap();
    let local = local.canonicalize().unwrap();
    let mut session = Session::start(&config, &local, ".");
    session.expect("zzz-large");
    session.expect("SSH:"); // The other pane has failed; local creation is still available.
    session.send("\x01");
    session.expect("2 marked");
    #[cfg(windows)]
    session.send("\x1b[65;30;1;1;24;1_\x1b[65;30;1;0;24;1_");
    #[cfg(unix)]
    session.send("\x1b[65;6u");
    session.expect("Selection cleared.");
    session.expect("0 marked");
    session.send("\x0f");
    session.expect("Sort: Name ↓");
    assert!(session.position("zzz-large").1 < session.position("aaa-small").1);
    session.send("\x0f");
    session.expect("Sort: Size ↑");
    assert!(session.position("aaa-small").1 < session.position("zzz-large").1);
    session.send("\x1b[2~");
    session.expect("Create local folder");
    session.send("wrong-name\x01new folder\r");
    session.settle();
    assert!(
        session
            .parser
            .screen()
            .contents()
            .contains("Name: new folder")
    );
    assert!(!local.join("new folder").exists());
    session.send(F9);
    session.expect("3 shown");
    assert!(local.join("new folder").is_dir());
    assert!(!local.join("wrong-name").exists());
    session.send("\x1b[2~new folder");
    session.expect("Name: new folder");
    session.send(F9);
    session.expect("already exists");
    assert_eq!(std::fs::read(local.join("aaa-small")).unwrap(), b"x");
    session.resize(16, 60);
    session.send("\x1bOP");
    session.expect("Keyboard");
    session.send("\x1b[6~\x1b[6~\x1b[6~");
    session.expect("Esc closes this help.");
    session.send("\x1b[H");
    session.expect("Switch local / remote pane");
    session.exit();
}

#[test]
#[ignore = "Requires explicit disposable SFTP fixture environment; never uses user SSH files"]
fn actual_sftp_create_folder_has_explicit_confirmation_and_preserves_existing_entries() {
    let config = std::env::var_os("SSH_FILES_TEST_CONFIG").expect("SSH_FILES_TEST_CONFIG");
    let server = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_TEST_REMOTE").expect("SSH_FILES_TEST_REMOTE"),
    );
    let target = tempfile::Builder::new()
        .prefix("mkdir-owned-")
        .tempdir_in(&server)
        .unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("kept.txt"), b"original bytes").unwrap();
    std::fs::write(local.path().join("local-ready"), b"ready").unwrap();
    let remote = target.path().to_str().unwrap().replace('\\', "/");
    let mut session = Session::start(
        Path::new(&config),
        &local.path().canonicalize().unwrap(),
        &remote,
    );
    session.expect("kept.txt");
    session.expect("local-ready");
    session.send("\t\x1b[2~");
    session.expect("Create remote folder");
    session.send("folder 開發\r");
    session.settle();
    assert!(!target.path().join("folder 開發").exists());
    session.send(F9);
    session.expect("2 shown");
    assert!(target.path().join("folder 開發").is_dir());
    session.send("\x1b[2~kept.txt");
    session.expect("Name: kept.txt");
    session.send(F9);
    session.expect("already exists");
    assert_eq!(
        std::fs::read(target.path().join("kept.txt")).unwrap(),
        b"original bytes"
    );
    session.send("\x1b");
    // The footer also exists beneath the modal. Observe its actual dismissal
    // before F3, so Unix does not receive adjacent ESC + ESC OR as one sequence.
    session.expect_absent("Create remote folder");
    session.filter("folder 開發");
    session.send("\r");
    session.expect("0 shown");
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
    // Start this new range on an unmarked row. A marked-row drag now carries
    // its complete existing batch through source-pane movement.
    let zero = session.position("item-00");
    session.mouse(0, zero, false);
    session.mouse(32, four, false);
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
    session.filter("alternate.txt");
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
        let ready = target.path().join("direct-drag-ready");
        std::fs::write(&ready, b"owned readiness marker").unwrap();
        let mut session = Session::start(&selected_config, local.path(), &remote);
        session.expect("SSH FILES");
        session.expect("large.bin");
        session.expect("direct-drag-ready");
        std::fs::remove_file(ready).unwrap();
        // This direct drag enters the same cancellable worker/queue used by
        // reviewed transfers, without an intervening F5/F9 confirmation.
        let from = session.position("large.bin");
        let to = (100, from.1 + 4); // Blank space in the remote file list.
        session.mouse(0, from, false);
        session.mouse(32, to, false);
        session.expect("Release to upload");
        session.mouse(0, to, true);
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

#[test]
#[ignore = "Requires explicit disposable SFTP fixture environment; never uses user SSH files"]
fn actual_sftp_cross_pane_drop_upload_download_and_no_clobber() {
    let config = std::env::var_os("SSH_FILES_TEST_CONFIG").expect("SSH_FILES_TEST_CONFIG");
    let server = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_TEST_REMOTE").expect("SSH_FILES_TEST_REMOTE"),
    );
    let target = tempfile::Builder::new()
        .prefix("pane-drop-owned-")
        .tempdir_in(server)
        .unwrap();
    let local = tempfile::tempdir().unwrap();
    let local = local.path().canonicalize().unwrap();
    let download = local.join("a-download");
    let destination = target.path().join("a-destination");
    std::fs::create_dir(&download).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(local.join("upload-a.txt"), b"first upload").unwrap();
    std::fs::write(local.join("upload-b.txt"), b"second upload").unwrap();
    std::fs::write(local.join("z-stay-local.txt"), b"not selected").unwrap();
    std::fs::write(local.join(".hidden-kept"), b"hidden").unwrap();
    std::fs::write(target.path().join("remote-data.txt"), b"download bytes").unwrap();
    let remote = target.path().to_str().unwrap().replace('\\', "/");
    let mut session = Session::start(Path::new(&config), &local, &remote);
    session.expect("upload-b.txt");
    session.expect("remote-data.txt");
    // Left/Right choose an absolute pane, including repeated keys.
    session.send("\x1b[C\x1b[C");
    session.send(F2);
    session.expect("Remote directory");
    session.send("\x1b");
    session.expect("F6 paste paths");
    session.send("\x1b[D\x1b[D");
    session.send(F2);
    session.expect("Local directory");
    session.send("\x1b");
    session.expect("F6 paste paths");
    #[cfg(windows)]
    {
        session.send(&format!("\"{}\"", local.join("upload-a.txt").display()));
        session.expect("Upload local paths");
        session.settle();
        assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
        assert!(!session.parser.screen().contents().contains("filter:"));
        session.send("\x1b");
        session.expect("F6 paste paths");
    }
    let a = session.position("upload-a.txt");
    let b = session.position("upload-b.txt");
    session.click(0, a);
    session.click(4, b);
    session.expect("2 marked");
    let remote_folder = session.position("a-destination");
    session.mouse(0, a, false);
    session.mouse(32, (a.0 + 3, a.1), false);
    session.expect("2 marked");
    let unmarked = session.position("z-stay-local.txt");
    session.mouse(32, unmarked, false);
    session.expect("2 marked");
    session.mouse(32, remote_folder, false);
    session.expect("Release to upload");
    session.mouse(0, remote_folder, true);
    session.expect("2 complete");
    assert!(
        !session
            .parser
            .screen()
            .contents()
            .contains("Review transfers")
    );
    assert_eq!(
        std::fs::read(destination.join("upload-a.txt")).unwrap(),
        b"first upload"
    );
    assert_eq!(
        std::fs::read(destination.join("upload-b.txt")).unwrap(),
        b"second upload"
    );
    assert!(!destination.join(".hidden-kept").exists());
    assert!(!destination.join("z-stay-local.txt").exists());
    session.mouse(0, remote_folder, true);
    session.settle(); // Duplicate release cannot dispatch twice.
    assert!(session.parser.screen().contents().contains("2 complete"));
    assert!(!session.parser.screen().contents().contains("Stopped:"));
    // A new gesture to the existing names must pause, keeping original bytes.
    session.mouse(0, a, false);
    session.mouse(32, remote_folder, false);
    session.mouse(0, remote_folder, true);
    session.expect("Stopped:");
    assert_eq!(
        std::fs::read(destination.join("upload-a.txt")).unwrap(),
        b"first upload"
    );
    assert_eq!(
        std::fs::read(destination.join("upload-b.txt")).unwrap(),
        b"second upload"
    );
    session.send("\x03");
    session.expect("Stopping requested work");
    let from = session.position("remote-data.txt");
    session.mouse(0, from, false);
    session.mouse(32, a, false);
    session.mouse(0, a, true);
    session.expect("a file is not a destination folder");
    assert!(!local.join("remote-data.txt").exists());
    let to = session.position("a-download");
    session.settle(); // Separate the new drag from the previous plain click.
    session.mouse(0, from, false);
    session.mouse(32, to, false);
    session.expect("Release to download");
    session.mouse(0, to, true);
    session.expect("3 complete");
    assert_eq!(
        std::fs::read(download.join("remote-data.txt")).unwrap(),
        b"download bytes"
    );
    assert!(!local.join("remote-data.txt").exists());
    session.exit();
    println!(
        "PASS: actual PTY arrows, range-marked direct upload, one release, no-clobber pause, file-target refusal, direct download to named folder; no Explorer gesture simulated"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "Requires explicit disposable SFTP fixture environment and Unix bracketed paste"]
fn actual_sftp_complete_absolute_path_paste_uploads_directly() {
    let config = std::env::var_os("SSH_FILES_TEST_CONFIG").expect("SSH_FILES_TEST_CONFIG");
    let server = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_TEST_REMOTE").expect("SSH_FILES_TEST_REMOTE"),
    );
    let target = tempfile::Builder::new()
        .prefix("path-paste-owned-")
        .tempdir_in(server)
        .unwrap();
    let local = tempfile::tempdir().unwrap();
    let local_path = local.path().canonicalize().unwrap();
    let first = local_path.join("pasted 開發.txt");
    let second = local_path.join("other.txt");
    std::fs::write(&first, b"complete path paste").unwrap();
    std::fs::write(&second, b"second").unwrap();
    std::fs::write(target.path().join("ready-marker"), b"ready").unwrap();
    let remote = target.path().to_str().unwrap();
    let mut session = Session::start(Path::new(&config), &local_path, remote);
    session.expect("pasted 開發.txt");
    session.expect("ready-marker");
    session.send(&format!(
        "\x1b[200~\"{}\" \"{}\"\x1b[201~",
        first.display(),
        second.display()
    ));
    session.expect("2 complete");
    assert_eq!(
        std::fs::read(target.path().join(first.file_name().unwrap())).unwrap(),
        b"complete path paste"
    );
    assert_eq!(
        std::fs::read(target.path().join(second.file_name().unwrap())).unwrap(),
        b"second"
    );
    assert!(
        !session
            .parser
            .screen()
            .contents()
            .contains("Review transfers")
    );
    session.exit();
}

#[cfg(windows)]
fn owned_ssh_handles(parent: u32) -> Vec<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
                QueryFullProcessImageNameW,
            },
        },
    };
    let expected = which::which("ssh").unwrap().canonicalize().unwrap();
    let mut result = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        assert_ne!(snapshot, INVALID_HANDLE_VALUE);
        let _snapshot = OwnedHandle::from_raw_handle(snapshot);
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut present = Process32FirstW(snapshot, &mut entry);
        while present != 0 {
            if entry.th32ParentProcessID == parent {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry.szExeFile.iter().position(|c| *c == 0).unwrap()],
                );
                if name.eq_ignore_ascii_case("ssh.exe") {
                    let raw = OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                        0,
                        entry.th32ProcessID,
                    );
                    assert!(!raw.is_null(), "Cannot inspect owned SSH child");
                    let handle = OwnedHandle::from_raw_handle(raw);
                    let mut image = vec![0u16; 32768];
                    let mut size = image.len() as u32;
                    assert_ne!(
                        QueryFullProcessImageNameW(raw, 0, image.as_mut_ptr(), &mut size),
                        0
                    );
                    let actual =
                        std::path::PathBuf::from(String::from_utf16_lossy(&image[..size as usize]));
                    assert_eq!(actual.canonicalize().unwrap(), expected);
                    result.push(handle);
                }
            }
            present = Process32NextW(snapshot, &mut entry);
        }
    }
    assert_eq!(
        result.len(),
        1,
        "Folder creation must own only its browser SSH session"
    );
    result
}

#[cfg(windows)]
#[test]
#[ignore = "Requires an explicit owned Windows SFTP fixture with --drop-mkdir-ack"]
fn actual_mkdir_lost_ack_cancel_and_close_retain_path_and_stop_owned_ssh() {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    let config = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_TEST_CONFIG").expect("SSH_FILES_TEST_CONFIG"),
    );
    let fixture = config.parent().unwrap().canonicalize().unwrap();
    let ready: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("ready.json")).unwrap()).unwrap();
    assert_eq!(
        ready["drop_mkdir_ack"], true,
        "Requires the explicit MKDIR fault fixture"
    );
    assert_eq!(
        std::path::PathBuf::from(ready["config"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        config.canonicalize().unwrap()
    );
    let server = std::path::PathBuf::from(
        std::env::var_os("SSH_FILES_TEST_REMOTE").expect("SSH_FILES_TEST_REMOTE"),
    );
    assert_eq!(
        server.canonicalize().unwrap(),
        std::path::PathBuf::from(ready["remote"].as_str().unwrap())
            .canonicalize()
            .unwrap()
    );
    assert_eq!(server.canonicalize().unwrap().parent().unwrap(), fixture);
    let marker = fixture.join("suppressed-ack.json");
    for mode in ["cancel", "close"] {
        let target = tempfile::Builder::new()
            .prefix("mkdir-ack-")
            .tempdir_in(&server)
            .unwrap();
        let local = tempfile::tempdir().unwrap();
        std::fs::write(local.path().join("local-ready"), b"owned").unwrap();
        std::fs::write(target.path().join("kept.txt"), b"unchanged").unwrap();
        let child_name = format!("lost-ack-{mode}-開發");
        let final_path = target.path().join(&child_name);
        // Windows OpenSSH REALPATH returns /C:/... for its POSIX SFTP namespace.
        // The owned directory still has the exact C:/... Windows identity below.
        let wire_final = format!("/{}", final_path.to_str().unwrap().replace('\\', "/"));
        let remote = target.path().to_str().unwrap().replace('\\', "/");
        let mut session = Session::start(&config, &local.path().canonicalize().unwrap(), &remote);
        session.expect("local-ready");
        session.expect("kept.txt");
        session.send("\t\x1b[2~");
        session.expect("Create remote folder");
        session.send(&child_name);
        session.send(F9);
        let deadline = Instant::now() + Duration::from_secs(10);
        let receipt = loop {
            if let Some(receipt) = std::fs::read(&marker)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                && receipt["destination"].as_str() == Some(wire_final.as_str())
            {
                break receipt;
            }
            assert!(
                Instant::now() < deadline,
                "No exact successful MKDIR reply was withheld: {}",
                session.parser.screen().contents()
            );
            session.pump(Duration::from_millis(10));
        };
        assert_eq!(
            receipt["event"],
            "actual-server-successful-mkdir-status-suppressed"
        );
        assert_eq!(receipt["request_type"], 14);
        assert_eq!(receipt["status"], 0);
        assert!(receipt["request_id"].as_u64().is_some());
        assert!(
            final_path.is_dir(),
            "Real server must create the folder before dropping its ACK"
        );
        let mut handles = owned_ssh_handles(session.child.process_id().unwrap());
        for key in ["relay_pid", "server_pid"] {
            let pid = u32::try_from(receipt[key].as_u64().unwrap()).unwrap();
            let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            assert!(!raw.is_null(), "Cannot inspect exact fixture {key}");
            handles.push(unsafe { OwnedHandle::from_raw_handle(raw) });
        }
        for handle in &handles {
            assert_eq!(
                unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
                WAIT_TIMEOUT
            );
        }
        let started = Instant::now();
        if mode == "cancel" {
            session.send("\x1b");
            session.expect("could not be confirmed");
        }
        session.exit();
        // Read the primary-screen report after the real terminal guard restores it.
        session.expect("could not be confirmed");
        session.expect(&child_name);
        assert!(!session.parser.screen().alternate_screen());
        for handle in &handles {
            assert_eq!(
                unsafe { WaitForSingleObject(handle.as_raw_handle(), 4000) },
                WAIT_OBJECT_0,
                "Owned client/relay/server process survived {mode}"
            );
        }
        assert!(started.elapsed() < Duration::from_secs(8));
        assert!(
            final_path.is_dir(),
            "Uncertain creation must not be removed"
        );
        assert_eq!(
            std::fs::read(target.path().join("kept.txt")).unwrap(),
            b"unchanged"
        );
        assert_eq!(
            std::fs::read_dir(target.path()).unwrap().count(),
            2,
            "No automatic retry or extra mutation"
        );
        println!(
            "PASS: actual MKDIR status 0 withheld for exact request {}; {mode} printed uncertain path, kept created folder and closed owned SSH/relay/server in {:.2}s",
            receipt["request_id"],
            started.elapsed().as_secs_f64()
        );
    }
}
