use super::*;
use std::{fs, path::Path, process::Stdio};
use tokio::io::AsyncReadExt;

fn fixture(role: &str, root: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "process::tests::helper",
            "--ignored",
            "--nocapture",
        ])
        .env("SFTP_TEST_HELPER_ROLE", role)
        .env("SFTP_TEST_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
#[ignore = "Owned process fixture entry only"]
fn helper() {
    let Ok(role) = std::env::var("SFTP_TEST_HELPER_ROLE") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("SFTP_TEST_ROOT").unwrap());
    match role.as_str() {
        "parent" => {
            let mut command = fixture("hold", &root);
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
            }
            let child = command.spawn().unwrap();
            fs::write(root.join("descendant.pid"), child.id().to_string()).unwrap();
            std::process::exit(0);
        }
        "hold" => loop {
            thread::sleep(Duration::from_secs(1));
        },
        _ => panic!("Invalid owned helper"),
    }
}

#[tokio::test]
async fn cancelling_blocked_windows_or_unix_pipes_kills_only_owned_descendants() {
    let root = tempfile::tempdir().unwrap();
    let control_root = tempfile::tempdir().unwrap();
    let mut control = OwnedChild::spawn(&mut fixture("hold", control_root.path())).unwrap();
    let mut owned = OwnedChild::spawn(&mut fixture("parent", root.path())).unwrap();
    let mut stdout =
        tokio::process::ChildStdout::from_std(owned.child.stdout.take().unwrap()).unwrap();
    let mut stderr =
        tokio::process::ChildStderr::from_std(owned.child.stderr.take().unwrap()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !root.path().join("descendant.pid").exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let descendant: u32 = fs::read_to_string(root.path().join("descendant.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let mut received = vec![];
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            stdout.read_to_end(&mut received)
        )
        .await
        .is_err()
    );
    let start = Instant::now();
    owned.stop().unwrap();
    tokio::time::timeout(Duration::from_secs(1), stdout.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), stderr.read_to_end(&mut vec![]))
        .await
        .unwrap()
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        control.child.try_wait().unwrap().is_none(),
        "Unrelated owned control session was killed"
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    while running(descendant) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !running(descendant),
        "Owned descendant retained pipes after cancellation"
    );
    control.stop().unwrap();
}

#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        CloseHandle(handle);
        alive
    }
}

#[test]
fn ui_can_stop_group_while_owner_cannot_poll_and_repeat_after_reap() {
    let root = tempfile::tempdir().unwrap();
    let control_root = tempfile::tempdir().unwrap();
    let mut control = OwnedChild::spawn(&mut fixture("hold", control_root.path())).unwrap();
    let registry = Registry::default();
    let mut owned = OwnedChild::spawn(&mut fixture("hold", root.path())).unwrap();
    owned.register(&registry, Role::Transfer).unwrap();
    let pid = owned.child.id();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        receive.recv().unwrap(); // Simulates a worker unable to poll cancellation.
        owned.stop().unwrap();
    });
    registry.close_all();
    let deadline = Instant::now() + Duration::from_secs(2);
    while running(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !running(pid),
        "UI registry failed to stop the worker's owned child"
    );
    assert!(control.child.try_wait().unwrap().is_none());
    send.send(()).unwrap();
    worker.join().unwrap();
    registry.close_all();
    assert!(
        control.child.try_wait().unwrap().is_none(),
        "Repeated stop targeted another child"
    );
    control.stop().unwrap();
}

#[test]
fn registration_racing_close_is_stopped_or_rejected_before_connection_use() {
    for _ in 0..8 {
        let root = tempfile::tempdir().unwrap();
        let registry = Registry::default();
        let mut owned = OwnedChild::spawn(&mut fixture("hold", root.path())).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let other_barrier = barrier.clone();
        let closing = registry.clone();
        let closer = thread::spawn(move || {
            other_barrier.wait();
            closing.close_all();
        });
        barrier.wait();
        let _ = owned.register(&registry, Role::Transfer);
        closer.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while running(owned.child.id()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !running(owned.child.id()),
            "Registry race left its child running"
        );
        owned.stop().unwrap();
        assert!(owned.child.try_wait().unwrap().is_some());
        let mut late = OwnedChild::spawn(&mut fixture("hold", root.path())).unwrap();
        assert!(late.register(&registry, Role::Browser).is_err());
        assert!(
            late.child.try_wait().unwrap().is_some(),
            "Sealed registration left a late child running"
        );
    }
}
#[cfg(unix)]
fn running(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat"))
        && stat
            .rsplit_once(')')
            .is_some_and(|(_, fields)| fields.trim_start().starts_with('Z'))
    {
        return false;
    }
    #[cfg(target_os = "macos")]
    unsafe {
        // A stopped but deliberately unreaped own child still passes kill(0).
        // Observe its exit without stealing the worker's later reap.
        let mut status = std::mem::zeroed::<libc::siginfo_t>();
        if libc::waitid(
            libc::P_PID,
            pid,
            &mut status,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        ) == 0
            && status.si_pid != 0
        {
            return false;
        }
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}
