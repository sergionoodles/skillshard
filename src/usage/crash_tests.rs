use super::generations::Generations;
use super::ingest::HistoryRoot;
use super::service::{Config, Service};
use super::storage::Database;
use super::ServiceState;
use crate::preferences::TrackingDays;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const CRASH_EXIT_CODE: i32 = 89;
const MAIN: &str = include_str!("adapters/fixtures/codex-main.jsonl");
const CHILD: &str = include_str!("adapters/fixtures/codex-child.jsonl");

pub(super) fn crash_at(point: &str) {
    if std::env::var("SKILLSHARD_USAGE_CRASH_POINT").as_deref() == Ok(point) {
        std::process::exit(CRASH_EXIT_CODE);
    }
}

fn now() -> i64 {
    1_790_784_000_000
}

fn options() -> Config {
    Config {
        enabled: true,
        days: TrackingDays::Thirty,
        scopes: Vec::new(),
    }
}

fn service(directory: &Path) -> Service {
    Service::new(
        directory.join("data"),
        vec![HistoryRoot {
            adapter: "codex".into(),
            path: directory.join("history"),
        }],
    )
    .with_clock(now)
}

#[test]
#[ignore = "subprocess helper used by forced_termination_recovers_every_publication_boundary"]
fn crash_worker() {
    let directory =
        std::path::PathBuf::from(std::env::var_os("SKILLSHARD_USAGE_CRASH_DIRECTORY").unwrap());
    let mut service = service(&directory);
    service.rebuild(options()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("requested crash boundary was not reached");
}

#[test]
fn forced_termination_recovers_every_publication_boundary() {
    for point in [
        "marker_synced",
        "intent",
        "fresh_initialized",
        "batch_committed",
        "validated",
        "manifest_synced",
        "published",
        "old_removed",
        "marker_removed",
        "retired_intent",
        "retired_removed",
    ] {
        let directory =
            std::env::temp_dir().join(format!("skillshard-crash-{point}-{}", std::process::id()));
        fs::create_dir_all(directory.join("data")).unwrap();
        fs::create_dir_all(directory.join("history")).unwrap();
        fs::write(directory.join("history/main.jsonl"), MAIN).unwrap();
        fs::write(directory.join("history/child.jsonl"), CHILD).unwrap();
        Database::open(&directory.join("data/usage.sqlite3")).unwrap();
        if point.starts_with("retired_") {
            let mut generations = Generations::load(&directory.join("data")).unwrap();
            generations.begin_rebuild(TrackingDays::Thirty).unwrap();
            fs::write(generations.import_path(), b"corrupt pending generation").unwrap();
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "usage::crash_tests::crash_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("SKILLSHARD_USAGE_CRASH_POINT", point)
            .env("SKILLSHARD_USAGE_CRASH_DIRECTORY", &directory)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(CRASH_EXIT_CODE),
            "{point}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut recovered = service(&directory);
        recovered.configure(options()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let view = recovered.view().lock().unwrap().clone();
            if view.status.state == ServiceState::Running
                && view.snapshot.counts.subagent_activations > 0
            {
                assert_eq!(view.snapshot.counts.conversations, 1, "{point}");
                assert_eq!(view.snapshot.counts.activations, 3, "{point}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{point}: {:?} {:?}",
                view.status.state,
                view.status.error
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        recovered.stop().unwrap();
        let generations = Generations::load(&directory.join("data")).unwrap();
        assert!(generations.pending.is_none(), "{point}");
        assert_eq!(
            fs::read_to_string(directory.join("history/main.jsonl")).unwrap(),
            MAIN
        );
        assert_eq!(
            fs::read_to_string(directory.join("history/child.jsonl")).unwrap(),
            CHILD
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
