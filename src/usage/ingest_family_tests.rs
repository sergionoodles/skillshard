use super::ingest::{import_batch, HistoryFile};
use super::storage::Database;
use super::Query;
use std::fs;
use std::sync::atomic::AtomicBool;

const NOW: i64 = 1_790_784_000_000;
const DAY: i64 = 86_400_000;

fn import(database: &mut Database, file: &HistoryFile) {
    let cancelled = AtomicBool::new(false);
    for _ in 0..100 {
        if import_batch(database, file, NOW - 30 * DAY, &cancelled)
            .unwrap()
            .complete
        {
            return;
        }
    }
    panic!("synthetic import did not finish");
}

#[test]
fn pi_fork_import_excludes_copied_entries_after_parent_arrives_and_restart() {
    let directory =
        std::env::temp_dir().join(format!("skillshard-pi-import-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let main = HistoryFile {
        adapter: "pi".into(),
        path: directory.join("main.jsonl"),
    };
    let fork = HistoryFile {
        adapter: "pi".into(),
        path: directory.join("fork.jsonl"),
    };
    fs::write(&main.path, include_str!("adapters/fixtures/pi-main.jsonl")).unwrap();
    fs::write(
        &fork.path,
        include_str!("adapters/fixtures/pi-fork.jsonl")
            .replace("/tmp/synthetic-parent.jsonl", &main.path.to_string_lossy()),
    )
    .unwrap();
    let database_path = directory.join("usage.sqlite3");
    let mut database = Database::open(&database_path).unwrap();
    import(&mut database, &fork);
    import(&mut database, &main);
    let first = database
        .snapshot(&Query::default(), NOW - 30 * DAY, NOW)
        .unwrap()
        .counts;
    assert_eq!(
        (first.activations, first.sessions, first.conversations),
        (3, 1, 1)
    );
    assert_eq!(
        (
            first.main_activations,
            first.subagent_activations,
            first.unconfirmed
        ),
        (3, 0, 2)
    );
    assert!(!first.provisional_conversations);
    drop(database);
    let mut database = Database::open(&database_path).unwrap();
    import(&mut database, &fork);
    import(&mut database, &main);
    assert_eq!(
        database
            .snapshot(&Query::default(), NOW - 30 * DAY, NOW)
            .unwrap()
            .counts,
        first
    );
}

#[test]
fn omp_child_path_links_and_mirrored_exports_preserve_worker_counts() {
    let directory =
        std::env::temp_dir().join(format!("skillshard-omp-import-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let main = HistoryFile {
        adapter: "omp".into(),
        path: directory.join("main.jsonl"),
    };
    let child = HistoryFile {
        adapter: "omp".into(),
        path: directory.join("child.jsonl"),
    };
    let mirror = HistoryFile {
        adapter: "omp".into(),
        path: directory.join("mirror.jsonl"),
    };
    fs::write(&main.path, include_str!("adapters/fixtures/omp-main.jsonl")).unwrap();
    fs::write(
        &mirror.path,
        include_str!("adapters/fixtures/omp-main.jsonl"),
    )
    .unwrap();
    fs::write(
        &child.path,
        include_str!("adapters/fixtures/omp-child.jsonl").replace(
            "/tmp/omp-sessions/./main.jsonl",
            &main.path.to_string_lossy(),
        ),
    )
    .unwrap();
    let mut database = Database::open(std::path::Path::new(":memory:")).unwrap();
    import(&mut database, &child);
    import(&mut database, &main);
    import(&mut database, &mirror);
    let counts = database
        .snapshot(&Query::default(), NOW - 30 * DAY, NOW)
        .unwrap()
        .counts;
    assert_eq!(
        (counts.activations, counts.sessions, counts.conversations),
        (5, 2, 1)
    );
    assert_eq!(
        (counts.main_activations, counts.subagent_activations),
        (3, 2)
    );
    assert!(!counts.provisional_conversations);
}
