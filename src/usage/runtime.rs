//! Serialized scheduling, imports and generation publication.

use super::{update_view, Command, Config, Relocation, View, COMMAND_CAPACITY};
use crate::model::Scope;
use crate::usage::generations::Generations;
use crate::usage::ingest::{self, HistoryFile, HistoryRoot};
use crate::usage::storage::{Database, IdentityTransfer};
use crate::usage::{Coverage, Query, ServiceState, UsageResult};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(30);
const COMMAND_INTERVAL: Duration = Duration::from_millis(100);

#[path = "coverage.rs"]
mod coverage;
use coverage::ScanCoverage;

#[path = "writer_lock.rs"]
mod writer_lock;
pub(super) use writer_lock::acquire_lock;

pub(super) struct Inputs {
    pub(super) directory: PathBuf,
    pub(super) roots: Vec<HistoryRoot>,
    pub(super) config: Config,
    pub(super) rebuild: bool,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) view: Arc<Mutex<View>>,
    pub(super) clock: fn() -> i64,
}

pub(super) fn run_worker(inputs: &Inputs, commands: mpsc::Receiver<Command>) -> UsageResult<()> {
    let _writer_lock = acquire_lock(&inputs.directory)?;
    let (generations, database, attribution_warning) = prepare_import(inputs)?;
    let mut runtime = Runtime {
        files: VecDeque::new(),
        next_discovery: Instant::now(),
        query: Query::default(),
        generations,
        scan_cutoff: inputs.config.days.cutoff((inputs.clock)()),
        had_source_errors: false,
        coverage: ScanCoverage::default(),
        attribution_warning,
    };
    run_imports(database, &mut runtime, inputs, commands)
}

fn prepare_import(inputs: &Inputs) -> UsageResult<(Generations, Database, Option<String>)> {
    let mut generations = Generations::load(&inputs.directory)?;
    generations.finish_published_cleanup()?;
    if inputs.rebuild || generations.pending.is_some() {
        generations.begin_rebuild(inputs.config.days)?;
    }
    let mut database = match Database::open_with_cancellation(
        &generations.import_path(),
        inputs.cancelled.clone(),
    ) {
        Ok(database) => database,
        Err(_)
            if inputs.rebuild
                && generations.pending.is_some()
                && !inputs.cancelled.load(Ordering::Acquire) =>
        {
            generations.replace_pending(inputs.config.days)?;
            Database::open_with_cancellation(&generations.import_path(), inputs.cancelled.clone())?
        }
        Err(error) => return Err(error),
    };
    #[cfg(test)]
    crate::usage::crash_tests::crash_at("fresh_initialized");
    if generations.pending.is_none() {
        generations.ensure_manifest()?;
    }
    let attribution_warning = if generations.pending.is_some() && generations.path().exists() {
        match database.copy_identity_metadata_from(&generations.path(), inputs.cancelled.clone())? {
            IdentityTransfer::Copied => None,
            IdentityTransfer::Unavailable(error) => Some(format!(
                "Historical skill aliases could not be recovered: {error}"
            )),
        }
    } else {
        None
    };
    register_scopes(&mut database, &inputs.config.scopes)?;
    Ok((generations, database, attribution_warning))
}

fn run_imports(
    mut database: Database,
    runtime: &mut Runtime,
    inputs: &Inputs,
    commands: mpsc::Receiver<Command>,
) -> UsageResult<()> {
    while !inputs.cancelled.load(Ordering::Acquire) {
        drain_commands(&mut database, runtime, inputs, &commands)?;
        if inputs.cancelled.load(Ordering::Acquire) {
            break;
        }
        if runtime.files.is_empty() && Instant::now() >= runtime.next_discovery {
            begin_scan(&mut database, runtime, inputs)?;
        }
        if let Some(file) = runtime.files.pop_front() {
            import_next(&mut database, runtime, inputs, file)?;
            continue;
        }
        if runtime.generations.pending.is_some() {
            if runtime.had_source_errors {
                return Err("Rebuild incomplete: a history source could not be imported. Retry after addressing the reported coverage errors.".into());
            }
            database.validate()?;
            database.checkpoint()?;
            #[cfg(test)]
            crate::usage::crash_tests::crash_at("validated");
            drop(database);
            runtime.generations.publish()?;
            update_view(&inputs.view, |view| {
                view.status.state = if inputs.config.enabled {
                    ServiceState::Running
                } else {
                    ServiceState::Stopped
                };
            });
            database = Database::open_with_cancellation(
                &runtime.generations.path(),
                inputs.cancelled.clone(),
            )?;
            publish_snapshot(&database, &runtime.query, inputs)?;
            if !inputs.config.enabled {
                break;
            }
        }
        match commands.recv_timeout(COMMAND_INTERVAL) {
            Ok(command) => apply_command(&mut database, runtime, inputs, command)?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(database);
    update_view(&inputs.view, |view| {
        view.status.state = if runtime.generations.pending.is_some() {
            ServiceState::RebuildPaused
        } else {
            ServiceState::Stopped
        }
    });
    Ok(())
}

struct Runtime {
    files: VecDeque<HistoryFile>,
    next_discovery: Instant,
    query: Query,
    generations: Generations,
    scan_cutoff: i64,
    had_source_errors: bool,
    coverage: ScanCoverage,
    attribution_warning: Option<String>,
}

fn register_scopes(database: &mut Database, scopes: &[Scope]) -> UsageResult<()> {
    let skills = scopes
        .iter()
        .flat_map(crate::scan::scan)
        .collect::<Vec<_>>();
    database.register_skills(&skills)
}

fn add_missing_history_coverage(
    database: &Database,
    files: &VecDeque<HistoryFile>,
    coverage: &mut Vec<Coverage>,
) -> UsageResult<()> {
    let paths = files
        .iter()
        .map(|file| (file.adapter.clone(), file.path.clone()))
        .collect::<HashSet<_>>();
    let identities = files
        .iter()
        .filter_map(|file| {
            fs::metadata(&file.path)
                .ok()
                .map(|metadata| (file.adapter.clone(), ingest::file_identity(&metadata)))
        })
        .collect::<HashSet<_>>();
    let mut missing = BTreeMap::<String, usize>::new();
    for (agent, path, identity) in database.source_locations()? {
        if !paths.contains(&(agent.clone(), path))
            && !identities.contains(&(agent.clone(), identity))
        {
            *missing.entry(agent).or_default() += 1;
        }
    }
    for (agent, count) in missing {
        let status =
            format!("Partial coverage: {count} previously imported history sources unavailable");
        if let Some(item) = coverage.iter_mut().find(|item| item.agent == agent) {
            item.status = status;
            item.diagnostics = item.diagnostics.saturating_add(count);
        } else {
            coverage.push(Coverage {
                agent,
                status,
                diagnostics: count,
                ..Coverage::default()
            });
        }
    }
    Ok(())
}

fn begin_scan(database: &mut Database, runtime: &mut Runtime, inputs: &Inputs) -> UsageResult<()> {
    runtime.scan_cutoff = inputs.config.days.cutoff((inputs.clock)());
    database.prune(runtime.scan_cutoff)?;
    let discovery = ingest::discover(&inputs.roots, &inputs.cancelled);
    runtime.files = discovery.files;
    let mut coverage = discovery.coverage;
    add_missing_history_coverage(database, &runtime.files, &mut coverage)?;
    if let Some(warning) = &runtime.attribution_warning {
        coverage.push(Coverage {
            agent: "skill-attribution".into(),
            status: format!("Partial coverage: {warning}"),
            diagnostics: 1,
            ..Coverage::default()
        });
    }
    runtime.coverage = ScanCoverage::new(coverage.clone());
    for file in &runtime.files {
        let source = database.source(&file.adapter, &file.path.to_string_lossy())?;
        runtime.coverage.update(file, source.as_ref(), None);
    }
    runtime.next_discovery = Instant::now() + POLL_INTERVAL;
    runtime.had_source_errors = coverage.iter().any(|item| {
        item.status.starts_with("History unavailable") || item.status.contains("discovery limit")
    });
    update_view(&inputs.view, |view| {
        view.status.state = if runtime.generations.pending.is_some() {
            ServiceState::Rebuilding
        } else {
            ServiceState::Running
        };
        view.status.coverage = runtime.coverage.snapshot();
    });
    publish_snapshot(database, &runtime.query, inputs)
}

fn import_next(
    database: &mut Database,
    runtime: &mut Runtime,
    inputs: &Inputs,
    file: HistoryFile,
) -> UsageResult<()> {
    let result = if file.adapter == "opencode" {
        crate::usage::opencode_ingest::import_batch(
            database,
            &file.path,
            runtime.scan_cutoff,
            inputs.cancelled.clone(),
        )
    } else {
        ingest::import_batch(database, &file, runtime.scan_cutoff, &inputs.cancelled)
    };
    match result {
        Ok(progress) => {
            if !progress.complete && !inputs.cancelled.load(Ordering::Acquire) {
                runtime.files.push_back(file.clone());
            }
            if progress.committed {
                #[cfg(test)]
                crate::usage::crash_tests::crash_at("batch_committed");
                update_view(&inputs.view, |view| {
                    view.status.imported_records += progress.records;
                });
            }
            let source = database.source(&file.adapter, &file.path.to_string_lossy())?;
            runtime.coverage.update(&file, source.as_ref(), None);
        }
        Err(error) => {
            runtime.had_source_errors = true;
            let source = database.source(&file.adapter, &file.path.to_string_lossy())?;
            runtime
                .coverage
                .update(&file, source.as_ref(), Some(error.clone()));
            update_view(&inputs.view, |view| {
                view.status.error = Some(error.clone());
            });
        }
    }
    update_view(&inputs.view, |view| {
        view.status.coverage = runtime.coverage.snapshot();
    });
    if runtime.files.is_empty() && !inputs.cancelled.load(Ordering::Acquire) {
        publish_snapshot(database, &runtime.query, inputs)?;
    }
    Ok(())
}

#[cfg(test)]
pub(in crate::usage) fn scan_once_for_test(
    directory: PathBuf,
    roots: Vec<HistoryRoot>,
    view: Arc<Mutex<View>>,
    clock: fn() -> i64,
) -> UsageResult<()> {
    let _writer_lock = acquire_lock(&directory)?;
    let inputs = Inputs {
        directory,
        roots,
        config: Config {
            enabled: true,
            days: crate::preferences::TrackingDays::Thirty,
            scopes: Vec::new(),
        },
        rebuild: false,
        cancelled: Arc::new(AtomicBool::new(false)),
        view,
        clock,
    };
    let generations = Generations::load(&inputs.directory)?;
    let mut database = Database::open(&generations.path())?;
    let mut runtime = Runtime {
        files: VecDeque::new(),
        next_discovery: Instant::now(),
        query: Query::default(),
        generations,
        scan_cutoff: inputs.config.days.cutoff(clock()),
        had_source_errors: false,
        coverage: ScanCoverage::default(),
        attribution_warning: None,
    };
    begin_scan(&mut database, &mut runtime, &inputs)?;
    while let Some(file) = runtime.files.pop_front() {
        import_next(&mut database, &mut runtime, &inputs, file)?;
    }
    Ok(())
}

fn drain_commands(
    database: &mut Database,
    runtime: &mut Runtime,
    inputs: &Inputs,
    commands: &mpsc::Receiver<Command>,
) -> UsageResult<()> {
    for _ in 0..COMMAND_CAPACITY {
        match commands.try_recv() {
            Ok(command) => apply_command(database, runtime, inputs, command)?,
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
        }
    }
    Ok(())
}

fn apply_command(
    database: &mut Database,
    runtime: &mut Runtime,
    inputs: &Inputs,
    command: Command,
) -> UsageResult<()> {
    match command {
        Command::Query(query) => {
            runtime.query = query;
            publish_snapshot(database, &runtime.query, inputs)
        }
        Command::Installs(changes, scopes) => apply_install_changes(database, changes, &scopes),
        Command::Wake => Ok(()),
    }
}

pub(super) fn apply_install_changes(
    database: &mut Database,
    changes: Vec<Relocation>,
    scopes: &[Scope],
) -> UsageResult<()> {
    for change in changes {
        match change {
            Relocation::Move(skill, destination) => database.record_move(&skill, &destination)?,
            Relocation::Disable(skill) => database.record_disable(&skill)?,
            Relocation::Enable(skill) => database.record_enable(&skill)?,
        }
    }
    register_scopes(database, scopes)
}

fn publish_snapshot(database: &Database, query: &Query, inputs: &Inputs) -> UsageResult<()> {
    if inputs.cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let now = (inputs.clock)();
    let snapshot = database.snapshot(query, inputs.config.days.cutoff(now), now)?;
    if !inputs.cancelled.load(Ordering::Acquire) {
        update_view(&inputs.view, |view| {
            view.snapshot = snapshot;
            view.query = query.clone();
        });
    }
    Ok(())
}
