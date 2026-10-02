//! One owned worker with a process-lifetime writer lock and explicit cancellation.

use super::generations::Generations;
use super::ingest::HistoryRoot;
use super::storage::Database;
use super::{Query, ServiceState, Snapshot, Status, UsageResult};
use crate::model::{Scope, Skill};
use crate::preferences::TrackingDays;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

#[path = "runtime.rs"]
mod runtime;
use runtime::{acquire_lock, run_worker, Inputs};

#[cfg(test)]
pub(super) use runtime::scan_once_for_test;

const COMMAND_CAPACITY: usize = 16;

#[derive(Clone)]
pub struct Config {
    pub enabled: bool,
    pub days: TrackingDays,
    pub scopes: Vec<Scope>,
}

#[derive(Clone, Default)]
pub struct View {
    pub status: Status,
    pub snapshot: Snapshot,
    pub revision: u64,
    pub query: Query,
}

enum Command {
    Query(Query),
    Installs(Vec<Relocation>, Vec<Scope>),
    Wake,
}

pub enum Relocation {
    Move(Skill, Scope),
    Disable(Skill),
    Enable(Skill),
}

pub struct Service {
    directory: PathBuf,
    roots: Vec<HistoryRoot>,
    worker: Option<JoinHandle<()>>,
    commands: Option<mpsc::SyncSender<Command>>,
    cancelled: Arc<AtomicBool>,
    view: Arc<Mutex<View>>,
    config: Option<Config>,
    generation: u64,
    shutdown: Arc<AtomicBool>,
    clock: fn() -> i64,
}

impl Service {
    pub fn new(directory: PathBuf, roots: Vec<HistoryRoot>) -> Self {
        Self {
            directory,
            roots,
            worker: None,
            commands: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            view: Arc::new(Mutex::new(View::default())),
            config: None,
            generation: 0,
            shutdown: Arc::new(AtomicBool::new(false)),
            clock: now_utc,
        }
    }

    pub fn view(&self) -> Arc<Mutex<View>> {
        self.view.clone()
    }

    pub fn cancellation(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    pub fn shutdown_signal(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    pub fn with_clock(mut self, clock: fn() -> i64) -> Self {
        self.clock = clock;
        self
    }

    /// Called on a background executor. Repeated starts with unchanged settings
    /// leave the existing worker and committed checkpoint intact.
    pub fn configure(&mut self, config: Config) -> UsageResult<()> {
        let unchanged = self.config.as_ref().is_some_and(|previous| {
            previous.enabled == config.enabled
                && previous.days == config.days
                && previous.scopes == config.scopes
        });
        if unchanged
            && self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
        {
            return Ok(());
        }
        self.stop()?;
        self.config = Some(config.clone());
        if !config.enabled {
            return self.load_stopped_snapshot();
        }
        self.launch(config, false)
    }

    pub fn rebuild(&mut self, config: Config) -> UsageResult<()> {
        self.stop()?;
        self.config = Some(config.clone());
        self.launch(config, true)
    }

    pub fn stop(&mut self) -> UsageResult<()> {
        if self.worker.is_none() {
            return Ok(());
        }
        self.cancelled.store(true, Ordering::Release);
        update_view(&self.view, |view| {
            view.status.state = ServiceState::Stopping
        });
        if let Some(commands) = self.commands.take() {
            match commands.try_send(Command::Wake) {
                Ok(())
                | Err(mpsc::TrySendError::Full(_))
                | Err(mpsc::TrySendError::Disconnected(_)) => {}
            }
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "usage worker terminated unexpectedly".to_string())?;
        }
        update_view(&self.view, |view| {
            if view.status.state == ServiceState::Stopping {
                view.status.state = ServiceState::Stopped;
            }
        });
        Ok(())
    }

    pub fn query(&mut self, query: Query) -> UsageResult<()> {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            return self.enqueue(Command::Query(query));
        }
        let generations = Generations::load(&self.directory)?;
        if generations.pending.is_some() {
            update_view(&self.view, |view| {
                view.status.state = ServiceState::RebuildPaused;
                view.snapshot = Snapshot::default();
            });
            return Ok(());
        }
        if !generations.path().exists() {
            return Ok(());
        }
        self.cancelled.store(false, Ordering::Release);
        self.check_shutdown()?;
        let database = Database::open_read_only_with_cancellation(
            &generations.path(),
            self.cancelled.clone(),
        )?;
        let now = (self.clock)();
        let days = self
            .config
            .as_ref()
            .map_or(TrackingDays::default(), |config| config.days);
        let snapshot = database.snapshot(&query, days.cutoff(now), now)?;
        update_view(&self.view, |view| {
            view.snapshot = snapshot;
            view.query = query;
        });
        Ok(())
    }

    pub fn apply_install_changes(
        &self,
        changes: Vec<Relocation>,
        scopes: Vec<Scope>,
    ) -> UsageResult<()> {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            return self.enqueue(Command::Installs(changes, scopes));
        }
        let generations = Generations::load(&self.directory)?;
        if !generations.path().exists() {
            return Ok(());
        }
        self.cancelled.store(false, Ordering::Release);
        self.check_shutdown()?;
        let _writer_lock = acquire_lock(&self.directory)?;
        let mut database =
            Database::open_with_cancellation(&generations.path(), self.cancelled.clone())?;
        let scopes = self
            .config
            .as_ref()
            .map_or(scopes.as_slice(), |config| config.scopes.as_slice());
        runtime::apply_install_changes(&mut database, changes, scopes)
    }

    fn enqueue(&self, command: Command) -> UsageResult<()> {
        self.commands
            .as_ref()
            .ok_or("usage collector is stopped")?
            .try_send(command)
            .map_err(|error| format!("usage request could not be queued: {error}"))
    }

    fn launch(&mut self, config: Config, rebuild: bool) -> UsageResult<()> {
        self.generation += 1;
        self.cancelled.store(false, Ordering::Release);
        self.check_shutdown()?;
        let (sender, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let inputs = Inputs {
            directory: self.directory.clone(),
            roots: self.roots.clone(),
            config,
            rebuild,
            cancelled: self.cancelled.clone(),
            view: self.view.clone(),
            clock: self.clock,
        };
        update_view(&self.view, |view| {
            view.status = Status {
                state: if rebuild {
                    ServiceState::Rebuilding
                } else {
                    ServiceState::Starting
                },
                generation: self.generation,
                ..Status::default()
            };
            view.snapshot = Snapshot::default();
        });
        let worker = thread::Builder::new()
            .name("skillshard-usage".into())
            .spawn(move || {
                let result = run_worker(&inputs, receiver);
                if let Err(error) = result {
                    update_view(&inputs.view, |view| {
                        if inputs.cancelled.load(Ordering::Acquire) {
                            view.status.state =
                                if inputs.directory.join("usage-rebuild.json").exists() {
                                    ServiceState::RebuildPaused
                                } else {
                                    ServiceState::Stopped
                                };
                        } else {
                            view.status.state = ServiceState::Failed;
                            view.status.error = Some(error);
                            view.snapshot = Snapshot::default();
                        }
                    });
                }
            })
            .map_err(|error| error.to_string())?;
        self.worker = Some(worker);
        self.commands = Some(sender);
        Ok(())
    }

    pub fn expire_disabled(&mut self) -> UsageResult<()> {
        if self.config.as_ref().is_none_or(|config| config.enabled) {
            return Ok(());
        }
        self.load_stopped_snapshot()
    }

    fn load_stopped_snapshot(&mut self) -> UsageResult<()> {
        let mut generations = Generations::load(&self.directory)?;
        if generations.path().exists() || generations.pending.is_some() {
            let _writer_lock = acquire_lock(&self.directory)?;
            generations.finish_published_cleanup()?;
            if generations.pending.is_none() {
                self.cancelled.store(false, Ordering::Release);
                self.check_shutdown()?;
                let mut database =
                    Database::open_with_cancellation(&generations.path(), self.cancelled.clone())?;
                let days = self
                    .config
                    .as_ref()
                    .map_or(TrackingDays::default(), |config| config.days);
                database.prune(days.cutoff((self.clock)()))?;
            }
        }
        update_view(&self.view, |view| {
            view.status.state = if generations.pending.is_some() {
                ServiceState::RebuildPaused
            } else {
                ServiceState::Stopped
            };
            view.snapshot = Snapshot::default();
            view.status.error = None;
        });
        if generations.pending.is_none() {
            let query = self
                .view
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .query
                .clone();
            self.query(query)?;
        }
        Ok(())
    }

    fn check_shutdown(&self) -> UsageResult<()> {
        if self.shutdown.load(Ordering::Acquire) {
            self.cancelled.store(true, Ordering::Release);
            return Err("usage service has shut down".into());
        }
        Ok(())
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("could not stop local usage tracking: {error}");
        }
    }
}

fn update_view(view: &Arc<Mutex<View>>, update: impl FnOnce(&mut View)) {
    let mut view = view.lock().unwrap_or_else(|poison| poison.into_inner());
    update(&mut view);
    view.revision += 1;
}

fn now_utc() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
