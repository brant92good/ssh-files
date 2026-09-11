use crate::{
    Outcome, ProcessRegistry, Route, Transport,
    browser::Browser,
    paths,
    plan::{self, Direction, Job},
    ui_model::{EditKind, Modal, Pane},
    workers::{
        LocalAction, LocalJob, LocalSafety, LocalValue, RemoteAction, RemoteJob, RemoteValue,
        TransferJob,
    },
};
use anyhow::{Context, Result, ensure};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, terminal,
};
use std::{
    cell::Cell,
    collections::VecDeque,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub struct Options {
    pub route: Route,
    pub label: String,
    pub machine_id: String,
    pub route_id: String,
    pub local: PathBuf,
    pub remote: String,
}

pub(crate) struct App {
    pub options: Options,
    pub local: Pane,
    pub remote: Pane,
    pub pane: usize,
    pub status: String,
    pub modal: Option<Modal>,
    pub active: Option<Job>,
    pub failed: Option<Job>,
    pub queue: VecDeque<Job>,
    pub completed: usize,
    pub progress: Option<Outcome>,
    pub local_job: Option<LocalJob>,
    pub remote_job: Option<RemoteJob>,
    pub transfer: Option<TransferJob>,
    pub safety: LocalSafety,
    registry: ProcessRegistry,
    browser: Option<Browser>,
    transport: Option<Transport>,
    local_generation: u64,
    remote_generation: u64,
    local_ready: bool,
    remote_ready: bool,
    cancelling_queue: bool,
    editor_return: Option<Modal>,
    history: VecDeque<String>,
    retained: Vec<String>,
    pending_review: Option<Vec<Job>>,
    plan_intent: Option<PlanIntent>,
    pending_draft: Option<String>,
    pub(crate) pointer: crate::pointer::Pointer,
    creating: Option<Creation>,
    shutting_down: bool,
    help_scroll_limit: u16,
}

struct Creation {
    request: crate::folders::Request,
    started: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlanMode {
    Review,
    StartOnReady,
}
struct PlanIntent {
    local: bool,
    generation: u64,
    mode: PlanMode,
    directories: [String; 2],
    revisions: [u64; 2],
    draft: Option<String>,
}

impl App {
    pub(crate) fn new(options: Options) -> Result<Self> {
        let local = Pane::new(
            options
                .local
                .to_str()
                .context("Local start path is not UTF-8")?
                .into(),
        );
        let remote = Pane::new(options.remote.clone());
        Ok(Self {
            options,
            local,
            remote,
            pane: 0,
            status: "Opening directories…".into(),
            modal: None,
            active: None,
            failed: None,
            queue: VecDeque::new(),
            completed: 0,
            progress: None,
            local_job: None,
            remote_job: None,
            transfer: None,
            safety: LocalSafety::default(),
            registry: ProcessRegistry::default(),
            browser: None,
            transport: None,
            local_generation: 0,
            remote_generation: 0,
            local_ready: false,
            remote_ready: false,
            cancelling_queue: false,
            editor_return: None,
            history: VecDeque::new(),
            retained: Vec::new(),
            pending_review: None,
            plan_intent: None,
            pending_draft: None,
            pointer: crate::pointer::Pointer::default(),
            creating: None,
            shutting_down: false,
            help_scroll_limit: 0,
        })
    }

    fn note(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = text.clone();
        self.history.push_back(text);
        if self.history.len() > 100 {
            self.history.pop_front();
        }
    }
    fn permits_local(&self) -> bool {
        self.safety.permits(self.local_job.as_ref())
            && !self
                .transfer
                .as_ref()
                .is_some_and(|job| *job.cancel.borrow() && job.progress.borrow().local_io_pending)
    }
    fn start_local(&mut self, action: LocalAction) -> Result<()> {
        ensure!(
            self.permits_local(),
            "Local filesystem work is unresolved. Close this view before starting more local work."
        );
        ensure!(
            self.local_job.is_none(),
            "A local operation is still finishing"
        );
        self.local_generation = self.local_generation.wrapping_add(1);
        self.local_job = Some(LocalJob::start(self.local_generation, action));
        Ok(())
    }
    fn start_remote(&mut self, action: RemoteAction) -> Result<()> {
        ensure!(
            self.remote_job.is_none(),
            "A remote operation is still finishing"
        );
        self.remote_generation = self.remote_generation.wrapping_add(1);
        self.remote_job = Some(RemoteJob::start(
            self.remote_generation,
            self.options.route.clone(),
            self.registry.clone(),
            self.browser.take(),
            action,
        ));
        Ok(())
    }
    fn current(&self) -> &Pane {
        if self.pane == 0 {
            &self.local
        } else {
            &self.remote
        }
    }
    fn current_mut(&mut self) -> &mut Pane {
        if self.pane == 0 {
            &mut self.local
        } else {
            &mut self.remote
        }
    }
    fn check_idle(&self) -> Result<()> {
        ensure!(
            self.creating.is_none(),
            "Wait for folder creation to finish"
        );
        ensure!(
            self.local_job.is_none()
                && self.remote_job.is_none()
                && self.pending_review.is_none()
                && self.plan_intent.is_none()
                && self.pending_draft.is_none(),
            "Wait for the current directory or transfer review to finish preparing"
        );
        ensure!(
            self.permits_local(),
            "Local filesystem work is unresolved; reopen Files before transferring"
        );
        ensure!(
            self.active.is_none() && self.queue.is_empty() && self.failed.is_none(),
            "Finish or cancel the current queue first"
        );
        ensure!(
            self.retained.len() < paths::QUEUE_LIMIT,
            "Retained-path report is full; close this view to export it"
        );
        Ok(())
    }
    fn check_queue_available(&self) -> Result<()> {
        self.check_idle()?;
        ensure!(
            self.local_ready && self.remote_ready,
            "Wait for both directories to open first"
        );
        Ok(())
    }
    fn check_folder_available(&self, local: bool) -> Result<()> {
        self.check_idle()?;
        ensure!(
            if local {
                self.local_ready
            } else {
                self.remote_ready
            },
            "Wait for this directory to open first"
        );
        Ok(())
    }
    fn create_folder(&mut self, request: crate::folders::Request) -> Result<()> {
        self.check_folder_available(request.local)?;
        let pane = if request.local {
            &self.local
        } else {
            &self.remote
        };
        ensure!(
            pane.path == request.parent,
            "The parent directory changed. Open Create folder again."
        );
        let destination = request.destination()?;
        let started = Arc::new(AtomicBool::new(false));
        if request.local {
            self.start_local(LocalAction::CreateFolder(request.clone(), started.clone()))?;
        } else {
            self.start_remote(RemoteAction::CreateFolder(request.clone(), started.clone()))?;
        }
        self.creating = Some(Creation { request, started });
        self.note(format!("Creating folder: {}", paths::display(&destination)));
        Ok(())
    }
    fn finish_folder(&mut self, result: Result<String>, unfinished: bool) {
        let Some(creation) = self.creating.take() else {
            return;
        };
        match result {
            Ok(path) => {
                self.note(format!("Created folder: {}", paths::display(&path)));
                let pane = if creation.request.local {
                    &self.local
                } else {
                    &self.remote
                };
                if !self.shutting_down && pane.path == creation.request.parent {
                    let result = if creation.request.local {
                        self.start_local(LocalAction::List(PathBuf::from(&creation.request.parent)))
                    } else {
                        self.start_remote(RemoteAction::List(creation.request.parent.clone()))
                    };
                    if let Err(error) = result {
                        self.note(format!(
                            "Folder created; refresh could not start: {error:#}"
                        ));
                    }
                }
            }
            Err(error) => {
                let path = creation
                    .request
                    .destination()
                    .unwrap_or_else(|_| creation.request.parent.clone());
                let status = if unfinished || creation.started.load(Ordering::Acquire) {
                    let message = format!(
                        "Folder creation could not be confirmed: {}. Refresh to check it before retrying. {error:#}",
                        paths::display(&path)
                    );
                    self.retained.push(message.clone());
                    message
                } else {
                    format!("Folder was not created: {error:#}")
                };
                self.note(status);
            }
        }
    }
    fn prepare(&mut self) -> Result<()> {
        self.check_queue_available()?;
        let names = self.current().selected()?;
        if self.pane == 0 {
            let sources = names
                .into_iter()
                .map(|name| PathBuf::from(&self.local.path).join(name))
                .collect();
            self.plan_upload(sources, self.remote.path.clone(), PlanMode::Review, None)?;
        } else {
            let sources = names
                .iter()
                .map(|name| paths::remote_join(&self.remote.path, name))
                .collect::<Result<Vec<_>>>()?;
            self.plan_download(sources, PathBuf::from(&self.local.path), PlanMode::Review)?;
        }
        self.note(
            "Preparing a transfer list. No destination is changed until you start the review.",
        );
        Ok(())
    }
    fn remember_plan(&mut self, local: bool, mode: PlanMode, draft: Option<String>) {
        self.plan_intent = Some(PlanIntent {
            local,
            generation: if local {
                self.local_generation
            } else {
                self.remote_generation
            },
            mode,
            directories: [self.local.path.clone(), self.remote.path.clone()],
            revisions: [self.local.revision, self.remote.revision],
            draft,
        });
    }
    fn plan_upload(
        &mut self,
        sources: Vec<PathBuf>,
        destination: String,
        mode: PlanMode,
        draft: Option<String>,
    ) -> Result<()> {
        self.check_queue_available()?;
        self.start_local(LocalAction::Plan(sources, destination.clone()))?;
        self.remember_plan(true, mode, draft);
        if mode == PlanMode::StartOnReady {
            self.note(format!(
                "Preparing direct upload to {}. Escape cancels; existing files are preserved.",
                paths::display(&destination)
            ));
        }
        Ok(())
    }
    fn plan_download(
        &mut self,
        sources: Vec<String>,
        destination: PathBuf,
        mode: PlanMode,
    ) -> Result<()> {
        self.check_queue_available()?;
        self.start_remote(RemoteAction::Plan(sources, destination.clone()))?;
        self.remember_plan(false, mode, None);
        if mode == PlanMode::StartOnReady {
            self.note(format!(
                "Preparing direct download to {}. Escape cancels; existing files are preserved.",
                destination.display()
            ));
        }
        Ok(())
    }
    fn finish_plan(&mut self, local: bool, generation: u64, result: Result<Vec<Job>>) {
        if !self
            .plan_intent
            .as_ref()
            .is_some_and(|intent| intent.local == local && intent.generation == generation)
        {
            return;
        }
        let intent = self.plan_intent.take().unwrap();
        let result = result.and_then(|jobs| {
            ensure!(
                intent.directories == [self.local.path.clone(), self.remote.path.clone()]
                    && intent.revisions == [self.local.revision, self.remote.revision],
                "Directories or selection view changed during planning; nothing was transferred"
            );
            ensure!(
                jobs.len() <= paths::QUEUE_LIMIT,
                "Transfer plan exceeds the queue limit"
            );
            self.check_queue_available()?;
            Ok(jobs)
        });
        match result {
            Ok(jobs) if intent.mode == PlanMode::Review => self.pending_review = Some(jobs),
            Ok(jobs) => {
                self.queue = jobs.into();
                self.note("Direct transfer queue started. Esc stops it; collisions pause without overwrite.");
            }
            Err(error) => {
                self.note(format!("Transfer preparation stopped: {error:#}"));
                if let Some(text) = intent.draft {
                    self.pending_draft = Some(text);
                }
            }
        }
    }
    fn drop_selection(&mut self, selection: crate::pointer::DropSelection) -> Result<()> {
        self.check_queue_available()?;
        ensure!(
            selection.directories == [self.local.path.clone(), self.remote.path.clone()]
                && selection.revisions == [self.local.revision, self.remote.revision],
            "Directories changed during the drag; nothing was transferred"
        );
        if selection.source_pane == 0 {
            let sources = selection
                .names
                .iter()
                .map(|name| PathBuf::from(&selection.directories[0]).join(name))
                .collect();
            self.plan_upload(sources, selection.destination, PlanMode::StartOnReady, None)
        } else {
            let sources = selection
                .names
                .iter()
                .map(|name| paths::remote_join(&selection.directories[1], name))
                .collect::<Result<Vec<_>>>()?;
            self.plan_download(
                sources,
                PathBuf::from(selection.destination),
                PlanMode::StartOnReady,
            )
        }
    }
    fn navigate(&mut self, path: String) -> Result<()> {
        self.pointer.clear();
        if self.pane == 0 {
            self.start_local(LocalAction::List(PathBuf::from(path)))?;
        } else {
            self.start_remote(RemoteAction::List(path))?;
        }
        self.note("Opening directory…");
        Ok(())
    }
    fn open_current(&mut self) -> Result<()> {
        if let Some(entry) = self.current().current() {
            ensure!(
                entry.rejected.is_none(),
                "{}",
                entry.rejected.as_deref().unwrap_or("Unsupported entry")
            );
            if entry.kind == crate::browser::Kind::Directory {
                let path = if self.pane == 0 {
                    PathBuf::from(&self.local.path)
                        .join(&entry.name)
                        .to_str()
                        .context("Local directory path is not UTF-8")?
                        .to_owned()
                } else {
                    paths::remote_join(&self.remote.path, &entry.name)?
                };
                self.navigate(path)?;
            }
        }
        Ok(())
    }
    fn remember_layout(&mut self, area: ratatui::layout::Rect) {
        self.help_scroll_limit = crate::ui_render::help_scroll_limit(area);
        if let Some(Modal::Help { scroll }) = &mut self.modal {
            *scroll = (*scroll).min(self.help_scroll_limit);
        }
        let geometry = crate::ui_render::mouse_geometry(area, &self.local, &self.remote);
        if let Some(geometry) = geometry {
            self.local.offset = geometry.panes[0].start;
            self.remote.offset = geometry.panes[1].start;
        }
        self.pointer
            .sync(geometry, [&self.local, &self.remote], self.modal.is_some());
    }
    fn mouse(&mut self, event: event::MouseEvent) -> Result<bool> {
        if self.modal.is_some() {
            self.pointer.clear();
            return Ok(false);
        }
        let action = self.pointer.event(
            event,
            [&mut self.local, &mut self.remote],
            &mut self.pane,
            Instant::now(),
        );
        if action.is_err() {
            self.pointer.clear();
        }
        if let Some(action) = action? {
            match action {
                crate::pointer::Action::OpenDirectory(pane) => {
                    self.pane = pane;
                    self.open_current()?;
                }
                crate::pointer::Action::Transfer(selection) => self.drop_selection(selection)?,
            }
        }
        Ok(false)
    }
    fn cancel(&mut self) {
        self.queue.clear();
        self.pending_review = None;
        self.plan_intent = None;
        self.pending_draft = None;
        self.failed = None;
        self.cancelling_queue = self.active.is_some();
        if let Some(transfer) = &self.transfer {
            self.safety.record(&transfer.progress.borrow());
            transfer.cancel();
        } else if let Some(active) = &self.active {
            if active.direction == Direction::Upload {
                if let Some(job) = &self.remote_job {
                    job.cancel();
                }
            } else if let Some(job) = &self.local_job {
                job.cancel();
            }
        } else {
            self.local_generation = self.local_generation.wrapping_add(1);
            self.remote_generation = self.remote_generation.wrapping_add(1);
            if let Some(job) = &self.local_job {
                job.cancel();
            }
            if let Some(job) = &self.remote_job {
                job.cancel();
            }
        }
        self.note("Stopping requested work; any completed or uncertain paths remain in Details.");
    }
    fn finish_active(&mut self, result: Result<()>, outcome: Option<Outcome>) {
        let Some(job) = self.active.take() else {
            return;
        };
        if let Some(outcome) = outcome {
            self.safety.record(&outcome);
            if outcome.partial.is_some() || outcome.phase == "committing" {
                let message = format!(
                    "{}: {}\nDestination: {}\nPartial: {}",
                    if outcome.phase == "committing" {
                        "Completion unknown; inspect destination before retrying"
                    } else {
                        "Retained or uncertain partial"
                    },
                    paths::display(&job.source),
                    paths::display(outcome.destination.as_deref().unwrap_or(&job.destination)),
                    paths::display(outcome.partial.as_deref().unwrap_or("not reported"))
                );
                if self.retained.len() < paths::QUEUE_LIMIT {
                    self.retained.push(message);
                }
            }
            self.progress = Some(outcome);
        }
        if let Err(error) = result {
            if job.directory {
                let message = format!(
                    "Folder operation stopped; the folder may exist: {}",
                    paths::display(&job.destination)
                );
                if self.retained.len() < paths::QUEUE_LIMIT {
                    self.retained.push(message);
                }
            }
            self.note(format!(
                "Stopped: {error:#}. F4 details; F2 change destination; Delete skip."
            ));
            if !self.cancelling_queue {
                self.failed = Some(job);
            }
        } else {
            self.completed += 1;
            self.note(format!("Completed: {}", paths::display(&job.destination)));
        }
        self.cancelling_queue = false;
    }

    async fn collect(&mut self) {
        if self.local_job.as_ref().is_some_and(|job| {
            job.started.elapsed() >= crate::IDLE
                && !job.cancel.load(std::sync::atomic::Ordering::Acquire)
        }) {
            if self.plan_intent.as_ref().is_some_and(|intent| intent.local) {
                self.pending_draft = self.plan_intent.take().and_then(|intent| intent.draft);
            }
            self.local_generation = self.local_generation.wrapping_add(1);
            self.local_job.as_ref().unwrap().cancel();
            self.note("Local operation timed out; waiting for its filesystem worker before accepting more local work.");
        }
        if self
            .local_job
            .as_ref()
            .is_some_and(|job| job.task.is_finished())
        {
            let job = self.local_job.take().unwrap();
            let generation = job.generation;
            let result = job
                .task
                .await
                .context("Local worker stopped unexpectedly")
                .and_then(|value| value);
            let directory = self
                .active
                .as_ref()
                .is_some_and(|job| job.directory && job.direction == Direction::Download);
            if self
                .creating
                .as_ref()
                .is_some_and(|creation| creation.request.local)
            {
                self.finish_folder(
                    result.and_then(|value| match value {
                        LocalValue::CreatedFolder(path) => Ok(path),
                        _ => anyhow::bail!("Unexpected folder worker result"),
                    }),
                    false,
                );
            } else if directory {
                self.finish_active(result.map(|_| ()), None);
            } else if generation == self.local_generation {
                match result {
                    Ok(LocalValue::Listing(listing)) => {
                        self.local.replace(listing);
                        self.local_ready = true;
                        self.note("Choose files: F5 reviews. Complete path paste uploads to the remote folder.");
                    }
                    Ok(LocalValue::Plan(jobs)) => self.finish_plan(true, generation, Ok(jobs)),
                    Ok(LocalValue::Directory) => {}
                    Ok(LocalValue::CreatedFolder(_)) => {
                        self.note("Unclaimed local folder result; refresh before retrying.")
                    }
                    Err(error)
                        if self.plan_intent.as_ref().is_some_and(|intent| {
                            intent.local && intent.generation == generation
                        }) =>
                    {
                        self.finish_plan(true, generation, Err(error))
                    }
                    Err(error) => self.note(format!("Local: {error:#}")),
                }
            }
        }
        if self
            .remote_job
            .as_ref()
            .is_some_and(|job| job.task.is_finished())
        {
            let job = self.remote_job.take().unwrap();
            let generation = job.generation;
            let (browser, result) = match job.task.await {
                Ok(value) => value,
                Err(error) => (None, Err(anyhow::anyhow!("Remote worker stopped: {error}"))),
            };
            self.browser = browser;
            let directory = self
                .active
                .as_ref()
                .is_some_and(|job| job.directory && job.direction == Direction::Upload);
            if self
                .creating
                .as_ref()
                .is_some_and(|creation| !creation.request.local)
            {
                self.finish_folder(
                    result.and_then(|value| match value {
                        RemoteValue::CreatedFolder(path) => Ok(path),
                        _ => anyhow::bail!("Unexpected folder worker result"),
                    }),
                    false,
                );
            } else if directory {
                self.finish_active(result.map(|_| ()), None);
            } else if generation == self.remote_generation {
                match result {
                    Ok(RemoteValue::Listing(listing)) => { self.remote.replace(listing); self.remote_ready = true; self.note("Choose files: F5 reviews. Complete path paste uploads to the remote folder."); },
                    Ok(RemoteValue::Plan(jobs)) => self.finish_plan(false, generation, Ok(jobs)),
                    Ok(RemoteValue::Directory) => {},
                    Ok(RemoteValue::CreatedFolder(_))=>self.note("Unclaimed remote folder result; refresh before retrying."),
                    Err(error) if self.plan_intent.as_ref().is_some_and(|intent| !intent.local && intent.generation == generation) => self.finish_plan(false, generation, Err(error)),
                    Err(error) => self.note(format!("SSH: {error:#}. Resolve connection/trust in a normal SSH session, then F8 to retry.")),
                }
            }
        }
        if let Some(transfer) = &self.transfer {
            self.progress = Some(transfer.progress.borrow().clone());
        }
        if self
            .transfer
            .as_ref()
            .is_some_and(|job| job.task.is_finished())
        {
            let job = self.transfer.take().unwrap();
            let fallback = job.progress.borrow().clone();
            match job.task.await {
                Ok(report) => {
                    self.transport = report.transport;
                    self.finish_active(report.result, Some(report.outcome));
                }
                Err(error) => self.finish_active(
                    Err(anyhow::anyhow!("Transfer worker stopped: {error}")),
                    Some(fallback),
                ),
            }
        }
        if self.modal.is_none()
            && let Some(text) = self.pending_draft.take()
        {
            self.modal = Some(Modal::Edit {
                kind: EditKind::Paste,
                text,
            });
        }
        if self.modal.is_none()
            && let Some(jobs) = self.pending_review.take()
        {
            self.modal = Some(Modal::Review { jobs, cursor: 0 });
        }
        if self.active.is_none()
            && self.failed.is_none()
            && !self.queue.is_empty()
            && self.permits_local()
        {
            let job = self.queue.front().unwrap().clone();
            if job.directory {
                let available = if job.direction == Direction::Upload {
                    self.remote_job.is_none()
                } else {
                    self.local_job.is_none()
                };
                if !available {
                    return;
                }
                let result = if job.direction == Direction::Upload {
                    self.start_remote(RemoteAction::Directory(job.destination.clone()))
                } else {
                    self.start_local(LocalAction::Directory(PathBuf::from(&job.destination)))
                };
                if let Err(error) = result {
                    self.note(error.to_string());
                    return;
                }
            } else {
                self.transfer = Some(TransferJob::start(
                    self.options.route.clone(),
                    self.registry.clone(),
                    self.transport.take(),
                    job.clone(),
                ));
            }
            self.queue.pop_front();
            self.active = Some(job);
        }
    }

    fn details(&mut self) {
        let current = self.current();
        let selected = current
            .current()
            .map(|entry| {
                format!(
                    "Selected: {}\n{}",
                    paths::display(&if self.pane == 0 {
                        PathBuf::from(&current.path)
                            .join(&entry.name)
                            .display()
                            .to_string()
                    } else {
                        format!("{}/{}", current.path.trim_end_matches('/'), entry.name)
                    }),
                    entry.rejected.as_deref().unwrap_or("")
                )
            })
            .unwrap_or_default();
        self.modal = Some(Modal::Details {
            body: format!(
                "Server: {}\nRoute: {}\nMachine ID: {}\nRoute ID: {}\n{}\n\n{}\n\n{}",
                paths::display(&self.options.label),
                paths::display(&self.options.route.host),
                paths::display(&self.options.machine_id),
                paths::display(&self.options.route_id),
                selected,
                self.history
                    .iter()
                    .map(|line| paths::display(line))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                self.retained.join("\n\n")
            ),
            scroll: 0,
        });
    }

    fn key(&mut self, key: KeyEvent) -> Result<bool> {
        self.pointer.clear();
        // Restore editable text after validation fails. Do not clone large
        // transfer reviews or history for every navigation key.
        let restore = match &self.modal {
            Some(Modal::CreateFolder(request)) => Some(Modal::CreateFolder(request.clone())),
            Some(Modal::Edit { kind, text }) => Some(Modal::Edit {
                kind: *kind,
                text: text.clone(),
            }),
            _ => None,
        };
        let result = self.key_inner(key);
        if result.is_err() && self.modal.is_none() {
            self.modal = restore;
        }
        result
    }
    fn key_inner(&mut self, key: KeyEvent) -> Result<bool> {
        if key.kind == KeyEventKind::Release {
            return Ok(false);
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let start = key.code == KeyCode::F(9) || (control && key.code == KeyCode::Char('s'));
        if control && key.code == KeyCode::Char('c') {
            self.modal = None;
            self.editor_return = None;
            self.cancel();
            return Ok(false);
        }
        if key.code == KeyCode::F(10) || (control && key.code == KeyCode::Char('q')) {
            if key.kind != KeyEventKind::Press {
                return Ok(false);
            }
            if matches!(self.modal, Some(Modal::Quit)) {
                return Ok(true);
            }
            if self.active.is_some() || !self.queue.is_empty() || self.creating.is_some() {
                self.modal = Some(Modal::Quit);
                return Ok(false);
            }
            return Ok(true);
        }
        if start && matches!(self.modal, Some(Modal::Review { .. })) {
            ensure!(
                self.permits_local(),
                "Local filesystem work is unresolved; transfer remains paused"
            );
            ensure!(self.active.is_none(), "A transfer is still finishing");
        }
        if let Some(modal) = self.modal.take() {
            match modal {
                Modal::CreateFolder(mut request) => {
                    match key.code {
                        KeyCode::Esc => return Ok(false),
                        KeyCode::F(9) if key.kind == KeyEventKind::Press => {
                            self.create_folder(request)?;
                            return Ok(false);
                        }
                        KeyCode::Backspace => {
                            request.name.pop();
                        }
                        KeyCode::Char('a') if control => request.name.clear(),
                        KeyCode::Char(character)
                            if !control
                                && !key.modifiers.contains(KeyModifiers::ALT)
                                && !character.is_control() =>
                        {
                            ensure!(
                                request.name.len() + character.len_utf8() <= 1024,
                                "Folder name is too long"
                            );
                            request.name.push(character);
                        }
                        _ => {}
                    }
                    self.modal = Some(Modal::CreateFolder(request));
                }
                Modal::Review {
                    mut jobs,
                    mut cursor,
                } => {
                    match key.code {
                        KeyCode::Esc => {
                            self.note("Transfer review cancelled");
                            return Ok(false);
                        }
                        KeyCode::Up => cursor = cursor.saturating_sub(1),
                        KeyCode::Down => cursor = (cursor + 1).min(jobs.len().saturating_sub(1)),
                        KeyCode::Delete => {
                            plan::skip(&mut jobs, cursor);
                            cursor = cursor.min(jobs.len().saturating_sub(1));
                        }
                        KeyCode::F(2) if !jobs.is_empty() => {
                            let text = jobs[cursor]
                                .destination
                                .rsplit(['/', '\\'])
                                .next()
                                .unwrap_or("")
                                .into();
                            self.editor_return = Some(Modal::Review { jobs, cursor });
                            self.modal = Some(Modal::Edit {
                                kind: EditKind::Rename,
                                text,
                            });
                            return Ok(false);
                        }
                        _ if start && key.kind == KeyEventKind::Press => {
                            ensure!(
                                self.permits_local(),
                                "Local filesystem work is unresolved; transfer remains paused"
                            );
                            ensure!(self.active.is_none(), "A transfer is still finishing");
                            self.failed = None;
                            self.queue = jobs.into();
                            self.note("Transfer queue started. Esc stops this queue.");
                            return Ok(false);
                        }
                        _ => {}
                    }
                    self.modal = Some(Modal::Review { jobs, cursor });
                }
                Modal::Edit { kind, mut text } => {
                    if key.code == KeyCode::Esc {
                        self.modal = self.editor_return.take();
                        return Ok(false);
                    }
                    let apply = key.code == KeyCode::F(5)
                        || (key.code == KeyCode::Enter && !matches!(kind, EditKind::Paste));
                    if apply {
                        match kind {
                            EditKind::LocalPath => {
                                self.start_local(LocalAction::List(PathBuf::from(
                                    text.trim_matches('"'),
                                )))?;
                            }
                            EditKind::RemotePath => {
                                self.start_remote(RemoteAction::List(text))?;
                            }
                            EditKind::LocalFilter => {
                                self.local.filter = text;
                                self.local.refilter();
                            }
                            EditKind::RemoteFilter => {
                                self.remote.filter = text;
                                self.remote.refilter();
                            }
                            EditKind::Paste => {
                                self.check_queue_available()?;
                                self.plan_upload(
                                    plan::pasted_paths(&text)?,
                                    self.remote.path.clone(),
                                    PlanMode::Review,
                                    None,
                                )?;
                            }
                            EditKind::Rename => {
                                if let Some(Modal::Review { mut jobs, cursor }) =
                                    self.editor_return.take()
                                {
                                    let result = plan::rename(&mut jobs, cursor, &text);
                                    self.modal = Some(Modal::Review { jobs, cursor });
                                    result?;
                                }
                            }
                        }
                        return Ok(false);
                    }
                    match key.code {
                        KeyCode::Backspace => {
                            text.pop();
                        }
                        KeyCode::Enter if matches!(kind, EditKind::Paste) => {
                            ensure!(text.len() < 64 * 1024, "Input is too long");
                            text.push('\n');
                        }
                        KeyCode::Char('a') if control => text.clear(),
                        KeyCode::Char(character)
                            if !control && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            ensure!(
                                text.len() + character.len_utf8()
                                    <= if matches!(kind, EditKind::Paste) {
                                        64 * 1024
                                    } else {
                                        4096
                                    },
                                "Input is too long"
                            );
                            text.push(character);
                        }
                        _ => {}
                    }
                    self.modal = Some(Modal::Edit { kind, text });
                }
                Modal::Quit => {
                    if key.code == KeyCode::F(10) || (control && key.code == KeyCode::Char('q')) {
                        return Ok(true);
                    }
                    if key.code != KeyCode::Esc {
                        self.modal = Some(Modal::Quit);
                    }
                }
                Modal::Help { mut scroll } => {
                    match key.code {
                        KeyCode::Up => scroll = scroll.saturating_sub(1),
                        KeyCode::Down => {
                            scroll = scroll.saturating_add(1).min(self.help_scroll_limit)
                        }
                        KeyCode::PageUp => scroll = scroll.saturating_sub(10),
                        KeyCode::PageDown => {
                            scroll = scroll.saturating_add(10).min(self.help_scroll_limit)
                        }
                        KeyCode::Home => scroll = 0,
                        _ => {}
                    }
                    if key.code != KeyCode::Esc && key.code != KeyCode::F(1) {
                        self.modal = Some(Modal::Help { scroll });
                    }
                }
                Modal::Details { body, mut scroll } => {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(4) => return Ok(false),
                        KeyCode::Up => scroll = scroll.saturating_sub(1),
                        KeyCode::Down => scroll = scroll.saturating_add(1),
                        KeyCode::PageDown => scroll = scroll.saturating_add(12),
                        KeyCode::PageUp => scroll = scroll.saturating_sub(12),
                        _ => {}
                    }
                    self.modal = Some(Modal::Details { body, scroll });
                }
            }
            return Ok(false);
        }
        if key.code == KeyCode::F(10) || (control && key.code == KeyCode::Char('q')) {
            if self.active.is_some() || !self.queue.is_empty() {
                self.modal = Some(Modal::Quit);
                return Ok(false);
            }
            return Ok(true);
        }
        if key.code == KeyCode::Esc || (control && key.code == KeyCode::Char('c')) {
            self.cancel();
            return Ok(false);
        }
        match key.code {
            KeyCode::Insert if key.kind == KeyEventKind::Press => {
                self.check_folder_available(self.pane == 0)?;
                self.note("Choose a folder name, then press F9 to create it.");
                self.modal = Some(Modal::CreateFolder(crate::folders::Request {
                    local: self.pane == 0,
                    parent: self.current().path.clone(),
                    name: String::new(),
                }));
            }
            KeyCode::Char('a' | 'A') if control => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.current_mut().marked.clear();
                    self.note("Selection cleared.");
                } else {
                    self.current_mut().select_all()?;
                    self.note(format!(
                        "Selected {} supported entries in this filtered list.",
                        self.current().marked.len()
                    ));
                }
            }
            KeyCode::Char('o') if control => {
                if key.kind == KeyEventKind::Press {
                    self.current_mut().cycle_sort();
                    self.note(format!(
                        "Sort: {} (folders first). Ctrl+O changes sort.",
                        self.current().sort.label()
                    ));
                }
            }
            KeyCode::Tab => self.pane = 1 - self.pane,
            KeyCode::Left => self.pane = 0,
            KeyCode::Right => self.pane = 1,
            KeyCode::Char('.')
                if !control
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.current().filter.is_empty() =>
            {
                if key.kind == KeyEventKind::Press {
                    let show = !self.local.show_hidden;
                    self.local.set_show_hidden(show);
                    self.remote.set_show_hidden(show);
                    self.note(if show {
                        "Hidden files shown (. to hide)"
                    } else {
                        "Hidden files hidden (. to show)"
                    });
                }
            }
            KeyCode::Up => self.current_mut().move_by(-1),
            KeyCode::Down => self.current_mut().move_by(1),
            KeyCode::PageUp => self.current_mut().move_by(-10),
            KeyCode::PageDown => self.current_mut().move_by(10),
            KeyCode::Enter => self.open_current()?,
            KeyCode::Backspace if !self.current().filter.is_empty() => {
                self.current_mut().filter.pop();
                self.current_mut().refilter();
            }
            KeyCode::Backspace => {
                let parent = if self.pane == 0 {
                    PathBuf::from(&self.local.path)
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|| self.local.path.clone())
                } else {
                    paths::remote_parent(&self.remote.path)
                };
                self.navigate(parent)?;
            }
            KeyCode::Char(' ') if self.current().filter.is_empty() => {
                self.current_mut().toggle()?
            }
            KeyCode::F(1) => self.modal = Some(Modal::Help { scroll: 0 }),
            KeyCode::F(2) if self.failed.is_some() => {
                let mut jobs = vec![self.failed.take().unwrap()];
                jobs.extend(self.queue.drain(..));
                self.editor_return = Some(Modal::Review { jobs, cursor: 0 });
                self.modal = Some(Modal::Edit {
                    kind: EditKind::Rename,
                    text: String::new(),
                });
            }
            KeyCode::F(2) => {
                self.modal = Some(Modal::Edit {
                    kind: if self.pane == 0 {
                        EditKind::LocalPath
                    } else {
                        EditKind::RemotePath
                    },
                    text: self.current().path.clone(),
                })
            }
            KeyCode::F(3) => {
                self.modal = Some(Modal::Edit {
                    kind: if self.pane == 0 {
                        EditKind::LocalFilter
                    } else {
                        EditKind::RemoteFilter
                    },
                    text: self.current().filter.clone(),
                })
            }
            KeyCode::F(4) => self.details(),
            KeyCode::F(5) => self.prepare()?,
            KeyCode::F(6) => {
                self.check_queue_available()?;
                self.modal = Some(Modal::Edit {
                    kind: EditKind::Paste,
                    text: String::new(),
                });
            }
            KeyCode::F(7) => {
                let pane = self.current();
                let path = match pane.current() {
                    Some(entry) => {
                        ensure!(
                            entry.rejected.is_none(),
                            "Unsupported entry; see F4 details"
                        );
                        if self.pane == 0 {
                            PathBuf::from(&pane.path)
                                .join(&entry.name)
                                .to_str()
                                .context("Path is not UTF-8")?
                                .to_owned()
                        } else {
                            paths::remote_join(&pane.path, &entry.name)?
                        }
                    }
                    None => pane.path.clone(),
                };
                crate::clipboard::copy(&path)?;
                self.note("Copy request sent to the terminal. Clipboard support or permission may be required; F4 shows the path.");
            }
            KeyCode::F(8) => self.navigate(self.current().path.clone())?,
            KeyCode::Delete if self.failed.is_some() => {
                let mut jobs = vec![self.failed.take().unwrap()];
                jobs.extend(self.queue.drain(..));
                plan::skip(&mut jobs, 0);
                self.queue = jobs.into();
                self.note(
                    "Skipped stopped item and any child items; continuing the remaining queue.",
                );
            }
            KeyCode::Char(character) if !control && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.modal = Some(Modal::Edit {
                    kind: EditKind::Paste,
                    text: character.to_string(),
                });
                self.note("Unframed text is a path draft, not a detected drop. F5 reviews; F9 starts. F3 edits the filter.");
            }
            _ => {}
        }
        Ok(false)
    }

    fn paste(&mut self, value: String) -> Result<()> {
        self.pointer.clear();
        ensure!(value.len() <= 64 * 1024, "Paste is too large");
        match &mut self.modal {
            Some(Modal::CreateFolder(request)) => {
                ensure!(
                    request.name.len() + value.len() <= 1024
                        && !value.chars().any(char::is_control),
                    "Paste one folder name, without newlines or control characters"
                );
                request.name.push_str(&value);
            }
            Some(Modal::Edit { kind, text }) => {
                let limit = if matches!(kind, EditKind::Paste) {
                    64 * 1024
                } else {
                    4096
                };
                ensure!(
                    text.len() + value.len() <= limit,
                    "Pasted input is too long"
                );
                text.push_str(&value);
            }
            None => {
                match plan::atomic_paths(&value).and_then(|sources| {
                    self.plan_upload(
                        sources,
                        self.remote.path.clone(),
                        PlanMode::StartOnReady,
                        Some(value.clone()),
                    )
                }) {
                    Ok(()) => {}
                    Err(error) => {
                        self.modal = Some(Modal::Edit {
                            kind: EditKind::Paste,
                            text: value,
                        });
                        self.note(format!(
                            "Paths kept as a draft: {error:#}. F5 reviews; F9 starts."
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn shutdown(&mut self) {
        self.shutting_down = true;
        self.queue.clear();
        self.failed = None;
        self.registry.close_all(); // Independent of any blocked async worker.
        self.cancel();
        if let Some(job) = &self.local_job {
            job.cancel();
        }
        if let Some(job) = &self.remote_job {
            job.cancel();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while (self.local_job.is_some() || self.remote_job.is_some() || self.transfer.is_some())
            && Instant::now() < deadline
        {
            self.collect().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if let Some(job) = &self.transfer {
            let outcome = job.progress.borrow().clone();
            self.finish_active(
                Err(anyhow::anyhow!("Worker has not completed local cleanup")),
                Some(outcome),
            );
        }
        if let Some(job) = &self.transfer {
            job.task.abort();
        }
        if self.active.as_ref().is_some_and(|job| job.directory) {
            self.finish_active(
                Err(anyhow::anyhow!(
                    "Directory worker has not completed; folder creation may have occurred"
                )),
                None,
            );
        }
        if self.creating.is_some() {
            self.finish_folder(
                Err(anyhow::anyhow!(
                    "Folder worker has not completed before shutdown"
                )),
                true,
            );
        }
        if let Some(job) = &self.remote_job {
            job.task.abort();
        }
        // A blocking local worker remains retained until app/runtime shutdown;
        // do not launch another or pretend aborting its JoinHandle stops it.
        let browser = self.browser.take();
        let transport = self.transport.take();
        let _ = tokio::join!(
            async {
                if let Some(browser) = browser {
                    let _ = browser.shutdown().await;
                }
            },
            async {
                if let Some(transport) = transport {
                    let _ = transport.shutdown().await;
                }
            }
        );
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.registry.close_all();
        if let Some(job) = &self.local_job {
            job.cancel();
        }
        if let Some(job) = &self.remote_job {
            job.cancel();
            job.task.abort();
        }
        if let Some(job) = &self.transfer {
            job.cancel();
            job.task.abort();
        }
    }
}

// Ratatui's Terminal::drop logs a failed cursor restore with eprintln!,
// which can panic again after both PTY output descriptors close. The outer
// guard already restores terminal state fallibly; make this inner destructor
// quiet while preserving ordinary rendering errors.
struct DropSafeWriter<W> {
    inner: W,
    dropping: Rc<Cell<bool>>,
}
impl<W: Write> Write for DropSafeWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.dropping.get() {
            Ok(bytes.len())
        } else {
            self.inner.write(bytes)
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.dropping.get() {
            Ok(())
        } else {
            self.inner.flush()
        }
    }
}
struct TerminalView {
    dropping: Rc<Cell<bool>>,
    inner: ratatui::Terminal<ratatui::backend::CrosstermBackend<DropSafeWriter<io::Stdout>>>,
}
impl TerminalView {
    fn new() -> io::Result<Self> {
        let dropping = Rc::new(Cell::new(false));
        Ok(Self {
            dropping: dropping.clone(),
            inner: ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(
                DropSafeWriter {
                    inner: io::stdout(),
                    dropping,
                },
            ))?,
        })
    }
}
impl Drop for TerminalView {
    fn drop(&mut self) {
        self.dropping.set(true);
    }
}
fn terminal_closed(error: &anyhow::Error) -> bool {
    let Some(error) = error.downcast_ref::<io::Error>() else {
        return false;
    };
    if matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::NotConnected
    ) {
        return true;
    }
    #[cfg(unix)]
    if matches!(error.raw_os_error(), Some(libc::EIO | libc::ENXIO)) {
        return true;
    }
    false
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Mouse capture saves the already-raw Windows input mode; restore it
        // before raw mode restores the original shell mode.
        let _ = execute!(io::stdout(), event::DisableMouseCapture);
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            event::DisableBracketedPaste,
            cursor::Show,
            terminal::LeaveAlternateScreen
        );
    }
}

pub async fn run(options: Options) -> Result<()> {
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "Open Files in an interactive terminal"
    );
    #[cfg(unix)]
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    terminal::enable_raw_mode()?;
    let guard = TerminalGuard;
    execute!(
        io::stdout(),
        terminal::EnterAlternateScreen,
        event::EnableBracketedPaste,
        event::EnableMouseCapture,
        cursor::Hide
    )?;
    let mut terminal = TerminalView::new()?;
    let mut app = App::new(options)?;
    app.start_local(LocalAction::List(PathBuf::from(&app.local.path)))?;
    app.start_remote(RemoteAction::List(app.remote.path.clone()))?;
    let loop_result: Result<()> = async {
    loop {
        app.collect().await;
        terminal.inner.draw(|frame| {
            app.remember_layout(frame.area());
            crate::ui_render::draw(frame, &app);
        })?;
        let mut quit = false;
        for _ in 0..32 {
            // The Unix use-dev-tty source checks its deadline before reading:
            // a zero timeout never consumes even ready/buffered input there.
            // Keep its EOF-safe backend with a small positive poll budget.
            let poll = if cfg!(unix) {
                Duration::from_millis(1)
            } else {
                Duration::ZERO
            };
            if !event::poll(poll)? {
                break;
            }
            let input = event::read()?;
            let redraw = matches!(input, Event::Mouse(_) | Event::Resize(_, _));
            let result = match input {
                Event::Key(key) => app.key(key),
                Event::Paste(value) => app.paste(value).map(|_| false),
                Event::Mouse(event) => app.mouse(event),
                Event::Resize(_, _) => { app.pointer.clear(); Ok(false) },
                _ => Ok(false),
            };
            match result {
                Ok(true) => {
                    quit = true;
                    break;
                }
                Ok(false) => {}
                Err(error) => app.note(format!("{error:#}")),
            }
            if redraw { break; }
        }
        if quit {
            break;
        }
        #[cfg(unix)]
        tokio::select! { _ = hangup.recv() => break, _ = terminate.recv() => break, _ = tokio::time::sleep(Duration::from_millis(15)) => {} }
        #[cfg(not(unix))]
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    Ok(())
    }.await;
    app.shutdown().await;
    drop(terminal);
    drop(guard);
    for retained in &app.retained {
        let _ = writeln!(io::stderr(), "{retained}\n");
    }
    match loop_result {
        Err(error) if terminal_closed(&error) => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_destructor_suppresses_only_teardown_output_errors() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        let mut writer = DropSafeWriter {
            inner: Closed,
            dropping: Rc::new(Cell::new(false)),
        };
        assert!(writer.write_all(b"frame").is_err());
        assert!(writer.flush().is_err());
        writer.dropping.set(true);
        writer.write_all(b"restore cursor").unwrap();
        writer.flush().unwrap();
        assert!(terminal_closed(&anyhow::Error::from(io::Error::from(
            io::ErrorKind::BrokenPipe
        ))));
        assert!(!terminal_closed(&anyhow::anyhow!(
            "Unrelated application failure"
        )));
    }
    fn app() -> App {
        App::new(Options {
            route: Route {
                host: "fixture.invalid".into(),
                hostname: None,
                config: None,
                user: None,
                port: None,
            },
            label: "Fixture".into(),
            machine_id: String::new(),
            route_id: String::new(),
            local: PathBuf::from("."),
            remote: ".".into(),
        })
        .unwrap()
    }
    #[test]
    fn pasted_key_text_cannot_quit_or_start_a_transfer() {
        let mut app = app();
        for character in "quit upload rename server.txt".chars() {
            assert!(
                !app.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                    .unwrap()
            );
        }
        assert!(app.active.is_none() && app.queue.is_empty() && app.transfer.is_none());
        app.modal = Some(Modal::Edit {
            kind: EditKind::Paste,
            text: String::new(),
        });
        for character in "C:\\name quit.txt".chars() {
            app.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(app.local_job.is_none() && app.transfer.is_none());
        assert!(matches!(app.modal, Some(Modal::Edit { .. })));
    }
    #[test]
    fn arrows_choose_panes_and_unframed_text_never_becomes_a_filter() {
        let mut app = app();
        for code in [KeyCode::Right, KeyCode::Right] {
            app.key(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
            assert_eq!(app.pane, 1);
        }
        app.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.pane, 0);
        for ch in "C:\\a path.txt".chars() {
            app.key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE))
                .unwrap();
        }
        assert!(
            matches!(&app.modal, Some(Modal::Edit {kind:EditKind::Paste,text}) if text == "C:\\a path.txt")
        );
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.pane, 0, "Editor arrows must not change the target pane");
        assert!(
            app.local.filter.is_empty() && app.remote.filter.is_empty() && app.local_job.is_none()
        );
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        app.key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE))
            .unwrap();
        app.paste("exact filter".into()).unwrap();
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.local.filter, "exact filter");
    }
    fn planned_job() -> Job {
        Job {
            direction: Direction::Upload,
            directory: false,
            source: "source".into(),
            destination: "/frozen/file".into(),
            bytes: 1,
        }
    }
    #[test]
    fn automatic_plan_requires_matching_generation_and_both_revisions_and_is_once_only() {
        for invalid in [
            "none",
            "generation",
            "source",
            "destination",
            "cancel",
            "queue-limit",
        ] {
            let mut app = app();
            app.local_ready = true;
            app.remote_ready = true;
            app.local_generation = 8;
            app.remember_plan(true, PlanMode::StartOnReady, None);
            if invalid == "source" {
                app.local.refilter();
            }
            if invalid == "destination" {
                app.remote.refilter();
            }
            if invalid == "cancel" {
                app.cancel();
            }
            let jobs = if invalid == "queue-limit" {
                vec![planned_job(); paths::QUEUE_LIMIT + 1]
            } else {
                vec![planned_job()]
            };
            app.finish_plan(true, if invalid == "generation" { 7 } else { 8 }, Ok(jobs));
            assert_eq!(app.queue.len(), usize::from(invalid == "none"), "{invalid}");
            if invalid == "none" {
                app.queue.clear();
                app.finish_plan(true, 8, Ok(vec![planned_job()]));
                assert!(app.queue.is_empty());
            }
        }
    }
    #[tokio::test]
    async fn invalid_atomic_paste_preserves_draft_and_cancelled_planner_cannot_start() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().canonicalize().unwrap().join("missing.txt");
        let mut app = app();
        app.local_ready = true;
        app.remote_ready = true;
        let text = format!("\"{}\"", source.display());
        app.paste(text.clone()).unwrap();
        assert!(app.plan_intent.is_some());
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.local_job.is_some() && Instant::now() < deadline {
            app.collect().await;
            tokio::task::yield_now().await;
        }
        assert!(app.local_job.is_none());
        assert!(
            matches!(&app.modal, Some(Modal::Edit {kind: EditKind::Paste,text: kept}) if kept == &text)
        );
        assert!(app.queue.is_empty() && app.transfer.is_none());
        app.modal = None;
        std::fs::write(&source, b"owned").unwrap();
        app.paste(text).unwrap();
        let cancel = app.local_job.as_ref().unwrap().cancel.clone();
        app.cancel();
        assert!(cancel.load(Ordering::Acquire));
        while app.local_job.is_some() && Instant::now() < deadline {
            app.collect().await;
            tokio::task::yield_now().await;
        }
        assert!(
            app.local_job.is_none()
                && app.plan_intent.is_none()
                && app.queue.is_empty()
                && app.transfer.is_none()
        );
    }
    #[test]
    fn review_enter_and_repeated_prepare_cannot_start_queue() {
        let mut app = app();
        app.modal = Some(Modal::Review {
            jobs: vec![Job {
                direction: Direction::Upload,
                directory: false,
                source: "source".into(),
                destination: "target".into(),
                bytes: 1,
            }],
            cursor: 0,
        });
        for code in [KeyCode::Enter, KeyCode::F(5)] {
            app.key(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
        }
        assert!(app.queue.is_empty());
        app.key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.queue.len(), 1);
    }

    #[tokio::test]
    async fn folder_form_keeps_paste_literal_and_needs_explicit_create_key() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        let mut app = app();
        app.local
            .replace(crate::browser::list_local(&parent, &AtomicBool::new(false)).unwrap());
        app.local_ready = true;
        // An unavailable other pane does not block creating a local folder.
        assert!(!app.remote_ready);
        app.key(KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE))
            .unwrap();
        app.paste("new folder".into()).unwrap();
        assert!(app.paste("\nF9".into()).is_err());
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(!parent.join("new folder").exists());
        assert!(app.local_job.is_none() && app.creating.is_none());
        let mut repeated = KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE);
        repeated.kind = KeyEventKind::Repeat;
        app.key(repeated).unwrap();
        assert!(app.local_job.is_none());
        app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(
            matches!(&app.modal, Some(Modal::CreateFolder(request)) if request.name.is_empty())
        );
        app.paste("../invalid".into()).unwrap();
        assert!(
            app.key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE))
                .is_err()
        );
        assert!(
            matches!(&app.modal, Some(Modal::CreateFolder(request)) if request.name == "../invalid")
        );
        app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL))
            .unwrap();
        app.paste("new folder".into()).unwrap();
        app.key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE))
            .unwrap();
        assert!(app.creating.is_some() && app.queue.is_empty() && app.active.is_none());
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.local_job.is_some() && Instant::now() < deadline {
            app.collect().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(app.local_job.is_none());
        assert!(parent.join("new folder").is_dir());
        assert!(
            app.local
                .entries
                .iter()
                .any(|entry| entry.name == "new folder")
        );
        assert!(app.retained.is_empty() && app.active.is_none() && app.queue.is_empty());
    }

    #[tokio::test]
    async fn shutdown_retains_uncertain_create_path_without_waiting_for_blocked_worker() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let mut app = app();
        let request = crate::folders::Request {
            local: true,
            parent: parent.to_str().unwrap().into(),
            name: "pending".into(),
        };
        let destination = request.destination().unwrap();
        app.creating = Some(Creation {
            request,
            // The worker has not signalled its mutation yet, but is still
            // running. Shutdown cannot prove that it will not create a path.
            started: Arc::new(AtomicBool::new(false)),
        });
        app.local_job = Some(LocalJob {
            generation: 1,
            started: Instant::now(),
            cancel: Arc::new(AtomicBool::new(false)),
            task: tokio::task::spawn_blocking(move || {
                receive.recv().unwrap();
                Ok(LocalValue::CreatedFolder(destination))
            }),
        });
        app.shutdown().await;
        assert!(
            app.retained
                .iter()
                .any(|line| line.contains("could not be confirmed") && line.contains("pending"))
        );
        assert!(app.creating.is_none());
        assert!(!app.local_job.as_ref().unwrap().task.is_finished());
        send.send(()).unwrap();
        app.local_job.take().unwrap().task.await.unwrap().unwrap();
    }
    #[test]
    fn paste_byte_limit_and_quit_repeat_keep_the_existing_ui_state() {
        let mut app = app();
        app.modal = Some(Modal::Edit {
            kind: EditKind::Paste,
            text: "x".repeat(64 * 1024),
        });
        assert!(
            app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .is_err()
        );
        assert!(matches!(&app.modal, Some(Modal::Edit { text, .. }) if text.len() == 64 * 1024));
        app.modal = Some(Modal::Edit {
            kind: EditKind::Paste,
            text: "x".repeat(64 * 1024 - 1),
        });
        assert!(
            app.key(KeyEvent::new(KeyCode::Char('開'), KeyModifiers::NONE))
                .is_err()
        );
        app.modal = Some(Modal::Quit);
        let mut key = KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE);
        key.kind = KeyEventKind::Repeat;
        assert!(!app.key(key).unwrap());
        assert!(matches!(app.modal, Some(Modal::Quit)));
        key.kind = KeyEventKind::Press;
        assert!(app.key(key).unwrap());
    }
    #[tokio::test]
    async fn one_planner_owns_review_and_does_not_replace_an_open_editor() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        std::fs::write(root.as_path().join("file"), b"data").unwrap();
        let mut app = app();
        app.local.replace(
            crate::browser::list_local(root.as_path(), &std::sync::atomic::AtomicBool::new(false))
                .unwrap(),
        );
        app.local_ready = true;
        app.remote_ready = true;
        app.remote.path = "/remote".into();
        app.prepare().unwrap();
        app.pane = 1;
        assert!(app.prepare().is_err());
        assert!(app.remote_job.is_none());
        app.modal = Some(Modal::Edit {
            kind: EditKind::RemotePath,
            text: "/another".into(),
        });
        while !app.local_job.as_ref().unwrap().task.is_finished() {
            tokio::task::yield_now().await;
        }
        app.collect().await;
        assert!(matches!(&app.modal, Some(Modal::Edit { text, .. }) if text == "/another"));
        assert!(app.pending_review.is_some());
        app.modal = None;
        app.collect().await;
        assert!(
            matches!(&app.modal, Some(Modal::Review { jobs, .. }) if jobs.len() == 1 && jobs[0].destination == "/remote/file")
        );
        assert!(app.queue.is_empty());
    }
    #[tokio::test]
    async fn shutdown_reports_unfinished_directory_mutation_without_joining_blocked_fs() {
        use std::sync::{Arc, atomic::AtomicBool};
        let (send, receive) = std::sync::mpsc::channel();
        let mut app = app();
        app.active = Some(Job {
            direction: Direction::Download,
            directory: true,
            source: "/remote/folder".into(),
            destination: "owned-pending-folder".into(),
            bytes: 0,
        });
        app.local_job = Some(LocalJob {
            generation: 1,
            started: Instant::now(),
            cancel: Arc::new(AtomicBool::new(false)),
            task: tokio::task::spawn_blocking(move || {
                receive.recv().unwrap();
                Ok(LocalValue::Directory)
            }),
        });
        app.shutdown().await;
        assert!(
            app.retained
                .iter()
                .any(|text| text.contains("folder may exist: owned-pending-folder"))
        );
        assert!(app.active.is_none());
        assert!(!app.local_job.as_ref().unwrap().task.is_finished());
        send.send(()).unwrap();
        app.local_job.take().unwrap().task.await.unwrap().unwrap();
    }
}
