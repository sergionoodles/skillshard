use super::generations::Generations;
use super::ingest::HistoryRoot;
use super::service::{Config, Service, View};
use super::{Query, ServiceState};
use crate::preferences::TrackingDays;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const MAIN: &str = include_str!("adapters/fixtures/codex-main.jsonl");
const CHILD: &str = include_str!("adapters/fixtures/codex-child.jsonl");

fn now() -> i64 {
    1_790_784_000_000
}

fn directory(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "skillshard-service-{tag}-{}-{number}",
        std::process::id()
    ));
    fs::create_dir_all(path.join("history")).unwrap();
    path
}

fn service(directory: &std::path::Path) -> Service {
    Service::new(
        directory.join("data"),
        vec![HistoryRoot {
            adapter: "codex".into(),
            path: directory.join("history"),
        }],
    )
    .with_clock(now)
}

fn config(enabled: bool, days: TrackingDays) -> Config {
    Config {
        enabled,
        days,
        scopes: Vec::new(),
    }
}

fn wait_for(service: &Service, predicate: impl Fn(&View) -> bool) -> View {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let view = service.view().lock().unwrap().clone();
        if predicate(&view) {
            return view;
        }
        assert!(
            Instant::now() < deadline,
            "usage service did not reach expected state: {:?}, {:?}",
            view.status.state,
            view.status.error
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn repeated_start_stop_restart_keeps_child_usage_and_releases_writer_lock() {
    let directory = directory("lifetime");
    fs::write(directory.join("history/main.jsonl"), MAIN).unwrap();
    fs::write(directory.join("history/child.jsonl"), CHILD).unwrap();
    let mut service = service(&directory);
    let options = config(true, TrackingDays::Thirty);
    service.configure(options.clone()).unwrap();
    let imported = wait_for(&service, |view| {
        view.snapshot.counts.subagent_activations > 0
    });
    assert_eq!(imported.snapshot.counts.conversations, 1);
    service.configure(options.clone()).unwrap();
    assert_eq!(
        service.view().lock().unwrap().status.generation,
        imported.status.generation
    );
    service.stop().unwrap();
    service.stop().unwrap();
    let stopped = service.view().lock().unwrap().clone();
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(service.view().lock().unwrap().revision, stopped.revision);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join("data/usage-writer.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    drop(lock);
    service.configure(options).unwrap();
    let restarted = wait_for(&service, |view| {
        view.snapshot.counts.subagent_activations > 0
    });
    assert_eq!(restarted.snapshot.counts, imported.snapshot.counts);
    service.stop().unwrap();
    assert_eq!(
        fs::read_to_string(directory.join("history/main.jsonl")).unwrap(),
        MAIN
    );
    assert_eq!(
        fs::read_to_string(directory.join("history/child.jsonl")).unwrap(),
        CHILD
    );
}

#[test]
fn stopped_service_accepts_install_metadata_changes_until_shutdown() {
    let directory = directory("stopped-installs");
    fs::write(directory.join("history/main.jsonl"), MAIN).unwrap();
    let mut service = service(&directory);
    service
        .configure(config(true, TrackingDays::Thirty))
        .unwrap();
    wait_for(&service, |view| view.snapshot.counts.activations > 0);
    service.stop().unwrap();

    service
        .apply_install_changes(Vec::new(), Vec::new())
        .unwrap();
    service.shutdown_signal().store(true, Ordering::Release);
    assert!(service
        .apply_install_changes(Vec::new(), Vec::new())
        .is_err());
}

#[test]
fn one_shot_rebuild_recovers_corrupt_active_database_without_enabling_tracking() {
    let directory = directory("corrupt");
    fs::create_dir_all(directory.join("data")).unwrap();
    fs::write(directory.join("data/usage.sqlite3"), b"corrupt SQLite").unwrap();
    fs::write(directory.join("history/main.jsonl"), MAIN).unwrap();
    let mut service = service(&directory);
    service
        .configure(config(true, TrackingDays::Thirty))
        .unwrap();
    wait_for(&service, |view| view.status.state == ServiceState::Failed);
    service
        .rebuild(config(false, TrackingDays::Thirty))
        .unwrap();
    let rebuilt = wait_for(&service, |view| {
        view.status.state == ServiceState::Stopped && view.snapshot.counts.activations > 0
    });
    assert!(rebuilt.status.error.is_none());
    let generations = Generations::load(&directory.join("data")).unwrap();
    assert!(generations.path().exists());
    assert!(!directory.join("data/usage.sqlite3").exists());
    assert!(generations.pending.is_none());
    service.query(Query::default()).unwrap();
    service.stop().unwrap();
    assert_eq!(
        fs::read_to_string(directory.join("history/main.jsonl")).unwrap(),
        MAIN
    );
}

#[test]
fn interrupted_marker_stays_paused_when_disabled_and_explicit_resume_publishes() {
    let directory = directory("paused");
    fs::write(directory.join("history/main.jsonl"), MAIN).unwrap();
    let mut generations = Generations::load(&directory.join("data")).unwrap();
    generations.begin_rebuild(TrackingDays::Thirty).unwrap();
    let fresh = generations.pending.as_ref().unwrap().fresh.clone();
    let mut service = service(&directory);
    service
        .configure(config(false, TrackingDays::Thirty))
        .unwrap();
    assert_eq!(
        service.view().lock().unwrap().status.state,
        ServiceState::RebuildPaused
    );
    assert!(!directory.join("data").join(&fresh).exists());
    service.query(Query::default()).unwrap();
    assert_eq!(
        service.view().lock().unwrap().status.state,
        ServiceState::RebuildPaused
    );
    service
        .rebuild(config(false, TrackingDays::Thirty))
        .unwrap();
    wait_for(&service, |view| {
        view.status.state == ServiceState::Stopped && view.snapshot.counts.activations > 0
    });
    assert_eq!(
        Generations::load(&directory.join("data")).unwrap().active,
        fresh
    );
    service.stop().unwrap();
}

#[test]
fn second_collector_fails_visibly_and_shutdown_cannot_restart_it() {
    let directory = directory("contention");
    fs::write(directory.join("history/main.jsonl"), MAIN).unwrap();
    let mut first = service(&directory);
    first.configure(config(true, TrackingDays::Thirty)).unwrap();
    wait_for(&first, |view| view.snapshot.counts.activations > 0);
    let mut second = service(&directory);
    second
        .configure(config(true, TrackingDays::Thirty))
        .unwrap();
    let failed = wait_for(&second, |view| view.status.state == ServiceState::Failed);
    assert!(failed
        .status
        .error
        .unwrap()
        .contains("another app instance"));
    second.stop().unwrap();
    first.shutdown_signal().store(true, Ordering::Release);
    first.stop().unwrap();
    assert!(first.configure(config(true, TrackingDays::Thirty)).is_err());
}

#[test]
fn service_window_changes_prune_and_backfill_the_same_history() {
    let directory = directory("windows");
    let older = MAIN.replace("2026-09-30", "2026-09-10");
    fs::write(directory.join("history/main.jsonl"), older).unwrap();
    let mut service = service(&directory);
    service
        .configure(config(true, TrackingDays::Thirty))
        .unwrap();
    let original = wait_for(&service, |view| view.snapshot.counts.activations > 0);
    service
        .configure(config(true, TrackingDays::Seven))
        .unwrap();
    wait_for(&service, |view| {
        view.status.state == ServiceState::Running && view.snapshot.counts.activations == 0
    });
    assert_eq!(
        service.view().lock().unwrap().snapshot.counts.activations,
        0
    );
    service
        .configure(config(true, TrackingDays::Thirty))
        .unwrap();
    let backfilled = wait_for(&service, |view| view.snapshot.counts.activations > 0);
    assert_eq!(
        backfilled.snapshot.counts.activations,
        original.snapshot.counts.activations
    );
    service.stop().unwrap();
}

#[test]
fn disabled_tracking_still_prunes_local_retention_without_importing_sources() {
    let directory = directory("disabled-retention");
    let older = MAIN.replace("2026-09-30", "2026-09-10");
    fs::write(directory.join("history/main.jsonl"), &older).unwrap();
    let mut service = service(&directory);
    service
        .configure(config(true, TrackingDays::Thirty))
        .unwrap();
    wait_for(&service, |view| view.snapshot.counts.activations > 0);
    service
        .configure(config(false, TrackingDays::Seven))
        .unwrap();
    let generations = Generations::load(&directory.join("data")).unwrap();
    let database = super::storage::Database::open_read_only(&generations.path()).unwrap();
    assert_eq!(
        database
            .snapshot(&Query::default(), TrackingDays::Thirty.cutoff(now()), now())
            .unwrap()
            .counts
            .activations,
        0
    );
    assert_eq!(
        fs::read_to_string(directory.join("history/main.jsonl")).unwrap(),
        older
    );
    assert_eq!(
        service.view().lock().unwrap().status.state,
        ServiceState::Stopped
    );
}

#[test]
fn explicit_rebuild_retry_recovers_a_corrupt_pending_generation() {
    let directory = directory("corrupt-pending");
    fs::write(directory.join("history/main.jsonl"), MAIN).unwrap();
    let mut generations = Generations::load(&directory.join("data")).unwrap();
    generations.begin_rebuild(TrackingDays::Thirty).unwrap();
    let corrupt = generations.import_path();
    fs::write(&corrupt, b"corrupt pending SQLite").unwrap();
    let mut service = service(&directory);
    service
        .rebuild(config(false, TrackingDays::Thirty))
        .unwrap();
    wait_for(&service, |view| {
        view.status.state == ServiceState::Stopped && view.snapshot.counts.activations > 0
    });
    let recovered = Generations::load(&directory.join("data")).unwrap();
    assert_ne!(recovered.path(), corrupt);
    assert!(!corrupt.exists());
    assert!(recovered.pending.is_none());
    service.stop().unwrap();
}

fn scan_diagnostics(directory: &std::path::Path) -> super::Coverage {
    let view = std::sync::Arc::new(std::sync::Mutex::new(View::default()));
    super::service::scan_once_for_test(
        directory.join("data"),
        vec![HistoryRoot {
            adapter: "codex".into(),
            path: directory.join("history"),
        }],
        view.clone(),
        now,
    )
    .unwrap();
    let current = view.lock().unwrap();
    current
        .status
        .coverage
        .iter()
        .find(|coverage| coverage.agent == "codex")
        .expect("Codex coverage")
        .clone()
}

#[test]
fn rescanning_a_replaced_malformed_source_does_not_double_count_diagnostics() {
    let directory = directory("diagnostic-replay");
    let source = directory.join("history/main.jsonl");
    fs::write(&source, format!("{MAIN}malformed record\n")).unwrap();
    assert_eq!(scan_diagnostics(&directory).diagnostics, 1);
    assert_eq!(scan_diagnostics(&directory).diagnostics, 1);
    fs::write(&source, format!("{MAIN}another malformed record\n")).unwrap();
    assert_eq!(scan_diagnostics(&directory).diagnostics, 1);
}

#[test]
fn replacing_an_unsupported_source_clears_stale_coverage_and_diagnostics() {
    let directory = directory("diagnostic-correction");
    let source = directory.join("history/main.jsonl");
    fs::write(&source, "{\"type\":\"unsupported\"}\n").unwrap();
    assert_eq!(scan_diagnostics(&directory).status, "Unsupported format");
    fs::write(&source, MAIN).unwrap();
    let corrected = scan_diagnostics(&directory);
    assert_eq!(corrected.diagnostics, 0);
    assert_eq!(corrected.status, "Supported; observed evidence only");
}

#[test]
fn unchanged_sources_keep_partial_attribution_status() {
    let directory = directory("diagnostic-attribution");
    let child = CHILD
        .lines()
        .map(|line| {
            let mut record: serde_json::Value = serde_json::from_str(line).unwrap();
            if record["type"] == "session_meta" {
                record["payload"]
                    .as_object_mut()
                    .unwrap()
                    .remove("subagent_history_start_ordinal");
                record["payload"]
                    .as_object_mut()
                    .unwrap()
                    .remove("forked_from_ordinal_exclusive");
            }
            format!("{record}\n")
        })
        .collect::<String>();
    fs::write(directory.join("history/child.jsonl"), child).unwrap();
    let first = scan_diagnostics(&directory);
    let unchanged = scan_diagnostics(&directory);
    assert_eq!(unchanged.diagnostics, first.diagnostics);
    assert!(
        unchanged.status.contains("attribution"),
        "{}",
        unchanged.status
    );
}

#[test]
fn rebuild_reports_missing_original_history_without_reusing_cached_events() {
    let directory = directory("missing-rebuild-source");
    let history = directory.join("history/main.jsonl");
    fs::write(&history, MAIN).unwrap();
    let mut service = service(&directory);
    service
        .configure(config(true, TrackingDays::Thirty))
        .unwrap();
    wait_for(&service, |view| view.snapshot.counts.activations > 0);
    service.stop().unwrap();
    fs::rename(history, directory.join("saved-source.jsonl")).unwrap();
    service
        .rebuild(config(false, TrackingDays::Thirty))
        .unwrap();
    let rebuilt = wait_for(&service, |view| view.status.state == ServiceState::Stopped);
    assert_eq!(rebuilt.snapshot.counts.activations, 0);
    assert!(rebuilt.status.coverage.iter().any(|coverage| coverage
        .status
        .contains("previously imported history sources unavailable")));
    service.stop().unwrap();
    assert_eq!(
        fs::read_to_string(directory.join("saved-source.jsonl")).unwrap(),
        MAIN
    );
}

#[test]
fn rebuild_preserves_explicit_historical_move_aliases() {
    use super::service::Relocation;
    use crate::model::Scope;

    let directory = directory("rebuild-alias");
    let original = Scope::Project(directory.join("original"));
    let destination = Scope::Project(directory.join("destination"));
    let original_path = original.canonical_dir().join("example");
    fs::create_dir_all(&original_path).unwrap();
    fs::write(
        original_path.join("SKILL.md"),
        "---\nname: example\ndescription: synthetic skill\n---\n",
    )
    .unwrap();
    let main = MAIN.replace(
        "/tmp/skillshard-synthetic-project",
        &directory.join("original").to_string_lossy(),
    );
    fs::write(directory.join("history/main.jsonl"), main).unwrap();
    let mut service = service(&directory);
    let mut options = config(true, TrackingDays::Thirty);
    options.scopes = vec![original.clone(), destination.clone()];
    service.configure(options.clone()).unwrap();
    wait_for(&service, |view| !view.snapshot.rankings.is_empty());
    service.stop().unwrap();
    let old_skill = crate::scan::scan(&original).remove(0);
    fs::create_dir_all(destination.canonical_dir()).unwrap();
    fs::rename(original_path, destination.canonical_dir().join("example")).unwrap();
    service
        .apply_install_changes(
            vec![Relocation::Move(old_skill, destination)],
            options.scopes.clone(),
        )
        .unwrap();
    options.enabled = false;
    service.rebuild(options).unwrap();
    let rebuilt = wait_for(&service, |view| {
        view.status.state == ServiceState::Stopped && !view.snapshot.rankings.is_empty()
    });
    assert_eq!(rebuilt.snapshot.unresolved, 0);
    assert_eq!(rebuilt.snapshot.rankings.len(), 1);
    assert_eq!(rebuilt.snapshot.rankings[0].counts.activations, 2);
    service.stop().unwrap();
}
