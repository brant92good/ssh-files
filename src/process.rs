//! Owned SSH processes. The platform launch code follows the independently
//! reviewed SSH Sessions process boundary; no shared live controller is used.
#[cfg(windows)]
use anyhow::ensure;
use anyhow::{Context, Result, bail};
#[cfg(windows)]
use std::io;
use std::{
    process::{Child, Command},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub struct OwnedChild {
    pub child: Child,
    group: Arc<Mutex<platform::Group>>,
    stop_deadline: Option<Instant>,
    stopped: bool,
}

#[cfg(test)]
mod tests;

impl OwnedChild {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        let (child, group) = platform::spawn(command)?;
        Ok(Self {
            child,
            group: Arc::new(Mutex::new(group)),
            stop_deadline: None,
            stopped: false,
        })
    }
    pub fn register(&mut self, registry: &Registry, role: Role) -> Result<()> {
        let handle = CancelHandle(self.group.clone());
        if let Err(error) = registry.register(role, handle) {
            let cleanup = self.stop();
            return Err(error.context(format!("Rejected SSH spawn cleanup: {cleanup:?}")));
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<()> {
        self.stop_until(Instant::now() + Duration::from_secs(2))
    }
    pub fn stop_until(&mut self, requested_deadline: Instant) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        let deadline = *self.stop_deadline.get_or_insert(requested_deadline);
        // Stop the owned group before reaping its leader, so a reused PID can
        // never become the target of later group cleanup on Unix.
        self.group.lock().unwrap_or_else(|p| p.into_inner()).stop();
        let _ = self.child.kill();
        loop {
            if self.child.try_wait()?.is_some() {
                self.stopped = true;
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("Owned SSH process did not exit within the cleanup deadline");
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Browser,
    Transfer,
}

#[derive(Clone)]
struct CancelHandle(Arc<Mutex<platform::Group>>);
impl CancelHandle {
    fn stop(&self) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).stop();
    }
    fn active(&self) -> bool {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).active()
    }
}

#[derive(Default)]
struct RegistryState {
    sealed: bool,
    groups: Vec<(Role, CancelHandle)>,
}
#[derive(Default)]
struct RegistryInner(Mutex<RegistryState>);
impl Drop for RegistryInner {
    fn drop(&mut self) {
        let state = self.0.get_mut().unwrap_or_else(|p| p.into_inner());
        for (_, group) in &state.groups {
            group.stop();
        }
    }
}

/// The UI can stop owned SSH processes even while a worker cannot poll. This
/// shares the existing sole group identity, never duplicates a Windows handle.
#[derive(Clone, Default)]
pub struct Registry(Arc<RegistryInner>);
impl Registry {
    fn register(&self, role: Role, handle: CancelHandle) -> Result<()> {
        let mut state = self.0.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.sealed {
            bail!("Files is closing; new SSH connections are disabled");
        }
        state.groups.retain(|(_, group)| group.active());
        if state.groups.len() >= 2 || state.groups.iter().any(|(active, _)| *active == role) {
            bail!("The owned SSH connection slot is already occupied");
        }
        state.groups.push((role, handle));
        Ok(())
    }
    pub(crate) fn cancel(&self, role: Role) {
        let groups: Vec<_> = self
            .0
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .groups
            .iter()
            .filter(|(active, _)| *active == role)
            .map(|(_, group)| group.clone())
            .collect();
        // Never hold the registry lock while stopping groups, waiting/reaping,
        // or joining a worker. The group lock only protects kill-and-clear.
        for group in groups {
            group.stop();
        }
    }
    pub fn close_all(&self) {
        let groups = {
            let mut state = self.0.0.lock().unwrap_or_else(|p| p.into_inner());
            state.sealed = true;
            state
                .groups
                .iter()
                .map(|(_, group)| group.clone())
                .collect::<Vec<_>>()
        };
        for group in groups {
            group.stop();
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::process::CommandExt;
    pub(super) struct Group(libc::pid_t);
    impl Group {
        pub(super) fn active(&self) -> bool {
            self.0 > 0
        }
        pub(super) fn stop(&mut self) {
            if self.0 > 0 {
                // Negative PID targets only this call's newly created group.
                unsafe {
                    libc::kill(-self.0, libc::SIGKILL);
                }
                self.0 = 0;
            }
        }
    }
    pub(super) fn spawn(command: &mut Command) -> Result<(Child, Group)> {
        command.process_group(0);
        let child = command.spawn().context("Could not start SSH")?;
        let group = Group(child.id() as libc::pid_t);
        Ok((child, group))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
            },
        },
    };
    // OwnedHandle carries the standard library's kernel-handle Send/Sync contract.
    struct Handle(OwnedHandle);
    impl Handle {
        fn new(value: HANDLE) -> io::Result<Self> {
            if value.is_null() || value == INVALID_HANDLE_VALUE {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self(unsafe { OwnedHandle::from_raw_handle(value) }))
            }
        }
    }
    pub(super) struct Group(Option<Handle>);
    impl Group {
        pub(super) fn active(&self) -> bool {
            self.0.is_some()
        }
        pub(super) fn stop(&mut self) {
            self.0.take();
        }
    }
    fn resume_primary(pid: u32) -> Result<()> {
        let snapshot = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut found = unsafe { Thread32First(snapshot.0.as_raw_handle(), &mut entry) };
        while found != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = Handle::new(unsafe {
                    OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID)
                })?;
                ensure!(
                    unsafe { ResumeThread(thread.0.as_raw_handle()) } != u32::MAX,
                    "Could not resume SSH: {}",
                    io::Error::last_os_error()
                );
                return Ok(());
            }
            found = unsafe { Thread32Next(snapshot.0.as_raw_handle(), &mut entry) };
        }
        bail!("Could not find the suspended SSH thread");
    }
    pub(super) fn spawn(command: &mut Command) -> Result<(Child, Group)> {
        let job = Handle::new(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        ensure!(
            unsafe {
                SetInformationJobObject(
                    job.0.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as _,
                    std::mem::size_of_val(&limits) as u32,
                )
            } != 0,
            "Could not configure SSH process ownership: {}",
            io::Error::last_os_error()
        );
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let mut child = command.spawn().context("Could not start SSH")?;
        // The child cannot execute code or create descendants before assignment.
        let setup = (|| -> Result<()> {
            ensure!(
                unsafe { AssignProcessToJobObject(job.0.as_raw_handle(), child.as_raw_handle()) }
                    != 0,
                "Could not own SSH process: {}",
                io::Error::last_os_error()
            );
            resume_primary(child.id())
        })();
        if let Err(error) = setup {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((child, Group(Some(job))))
    }
}
