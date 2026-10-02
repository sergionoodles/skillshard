use super::ingest::{self, HistoryFile};
use super::storage::Database;
use super::{Query, Source};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const NOW: i64 = 1_790_784_000_000;
const DAY: i64 = 86_400_000;
const MAIN: &str = include_str!("adapters/fixtures/codex-main.jsonl");

struct Fixture {
    directory: PathBuf,
    history: HistoryFile,
    database: Database,
}

impl Fixture {
    fn new(tag: &str, contents: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("skillshard-ingest-{tag}-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let history = HistoryFile {
            adapter: "codex".into(),
            path: directory.join("session.jsonl"),
        };
        fs::write(&history.path, contents).unwrap();
        let database = Database::open(&directory.join("usage.sqlite3")).unwrap();
        Self {
            directory,
            history,
            database,
        }
    }

    fn import(&mut self, cutoff: i64) {
        let cancelled = AtomicBool::new(false);
        for _ in 0..1000 {
            if ingest::import_batch(&mut self.database, &self.history, cutoff, &cancelled)
                .unwrap()
                .complete
            {
                return;
            }
        }
        panic!("fixture failed to finish bounded import");
    }

    fn counts(&self, cutoff: i64) -> u64 {
        self.database
            .snapshot(&Query::default(), cutoff, NOW + DAY)
            .unwrap()
            .counts
            .activations
    }

    fn source(&self) -> Source {
        self.database
            .source("codex", &self.history.path.to_string_lossy())
            .unwrap()
            .unwrap()
    }
}

fn synthetic(count: usize, timestamp: &str) -> String {
    let header = MAIN.lines().next().unwrap();
    let mut contents = format!("{header}\n");
    for index in 0..count {
        let call = serde_json::json!({"timestamp":timestamp,"type":"response_item","payload":{"type":"function_call","call_id":format!("read-{index}"),"name":"exec_command","arguments":"{\"cmd\":\"cat /fixture/example/SKILL.md\"}"}});
        let result = serde_json::json!({"timestamp":timestamp,"type":"response_item","payload":{"type":"function_call_output","call_id":format!("read-{index}"),"output":"Process exited with code 0\nFinal output:\nSynthetic instructions"}});
        contents.push_str(&format!("{call}\n{result}\n"));
    }
    contents
}

#[test]
fn restart_resumes_committed_batches_and_unchanged_files_skip_parsing() {
    let mut fixture = Fixture::new("restart", &synthetic(600, "2026-09-30T10:00:00Z"));
    let cutoff = NOW - 30 * DAY;
    let first = ingest::import_batch(
        &mut fixture.database,
        &fixture.history,
        cutoff,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(first.committed);
    assert!(!first.complete);
    assert!(fixture.source().offset < fixture.source().size);
    let database_path = fixture.directory.join("usage.sqlite3");
    drop(fixture.database);
    fixture.database = Database::open(&database_path).unwrap();
    fixture.import(cutoff);
    assert_eq!(fixture.counts(cutoff), 600);
    let unchanged = ingest::import_batch(
        &mut fixture.database,
        &fixture.history,
        cutoff,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(unchanged.complete);
    assert!(!unchanged.committed);
}

#[test]
fn incomplete_tail_waits_for_append_and_pending_result_survives_restart() {
    let contents = synthetic(1, "2026-09-30T10:00:00Z");
    let complete_end = contents.rfind('\n').unwrap();
    let mut fixture = Fixture::new("append", &contents[..complete_end]);
    let cutoff = NOW - 30 * DAY;
    fixture.import(cutoff);
    let before = fixture.source().offset;
    assert_eq!(fixture.counts(cutoff), 0);
    assert_eq!(
        fixture
            .database
            .snapshot(&Query::default(), cutoff, NOW + DAY)
            .unwrap()
            .counts
            .unconfirmed,
        1
    );
    let database_path = fixture.directory.join("usage.sqlite3");
    drop(fixture.database);
    fixture.database = Database::open(&database_path).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&fixture.history.path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    fixture.import(cutoff);
    assert!(fixture.source().offset > before);
    assert_eq!(fixture.counts(cutoff), 1);
}

#[test]
fn truncation_and_parser_replay_reconcile_observations() {
    let mut fixture = Fixture::new("rewrite", &synthetic(3, "2026-09-30T10:00:00Z"));
    let cutoff = NOW - 30 * DAY;
    fixture.import(cutoff);
    let generation = fixture.source().generation;
    fs::write(&fixture.history.path, synthetic(1, "2026-09-30T10:00:00Z")).unwrap();
    fixture.import(cutoff);
    assert_eq!(fixture.counts(cutoff), 1);
    assert!(fixture.source().generation > generation);
    let mut source = fixture.source();
    source.parser_version = 0;
    fixture
        .database
        .commit_batch(&source, &[], cutoff, false)
        .unwrap();
    fixture.import(cutoff);
    assert_eq!(fixture.counts(cutoff), 1);
    assert!(fixture.source().generation > source.generation);
}

#[test]
fn archived_exports_deduplicate_without_erasing_disappeared_sources() {
    let mut fixture = Fixture::new("archive", &synthetic(2, "2026-09-30T10:00:00Z"));
    let cutoff = NOW - 30 * DAY;
    fixture.import(cutoff);
    let archive = fixture.directory.join("archive.jsonl");
    fs::rename(&fixture.history.path, &archive).unwrap();
    fixture.history.path = archive;
    fixture.import(cutoff);
    assert_eq!(fixture.counts(cutoff), 2);
}

#[test]
fn increasing_retention_replays_previously_pruned_records_despite_offsets() {
    let mut fixture = Fixture::new("retention", &synthetic(2, "2026-09-10T10:00:00Z"));
    fixture.import(NOW - 30 * DAY);
    assert_eq!(fixture.counts(NOW - 30 * DAY), 2);
    fixture.database.prune(NOW - 7 * DAY).unwrap();
    assert_eq!(fixture.counts(NOW - 7 * DAY), 0);
    fixture.import(NOW - 30 * DAY);
    assert_eq!(fixture.counts(NOW - 30 * DAY), 2);
}

#[test]
fn cancelled_work_never_advances_cursors_and_malformed_records_are_diagnosed() {
    let mut fixture = Fixture::new(
        "cancel",
        &format!("{{ broken }}\n{}", synthetic(1, "2026-09-30T10:00:00Z")),
    );
    let cancelled = AtomicBool::new(true);
    let progress = ingest::import_batch(
        &mut fixture.database,
        &fixture.history,
        NOW - 30 * DAY,
        &cancelled,
    )
    .unwrap();
    assert!(!progress.committed);
    assert!(fixture
        .database
        .source("codex", &fixture.history.path.to_string_lossy())
        .unwrap()
        .is_none());
    cancelled.store(false, Ordering::Release);
    fixture.import(NOW - 30 * DAY);
    assert_eq!(fixture.source().diagnostics, 1);
    assert_eq!(fixture.counts(NOW - 30 * DAY), 1);
}

#[test]
fn claude_nul_padded_messages_import_without_losing_results() {
    let contents = include_str!("adapters/fixtures/claude-main.jsonl")
        .lines()
        .map(|line| format!("\0\0{line}\n"))
        .collect::<String>();
    let mut fixture = Fixture::new("claude-padding", &contents);
    fixture.history.adapter = "claude-code".into();
    let cutoff = NOW - 30 * DAY;
    fixture.import(cutoff);
    assert_eq!(fixture.counts(cutoff), 1);
    let source = fixture
        .database
        .source("claude-code", &fixture.history.path.to_string_lossy())
        .unwrap()
        .unwrap();
    assert_eq!(source.diagnostics, 0);
    assert_eq!(source.offset, contents.len() as u64);
}

#[test]
fn claude_metadata_only_history_is_recognized_without_inventing_usage() {
    let contents = "{\"type\":\"file-history-snapshot\",\"messageId\":\"synthetic-message\",\"snapshot\":{}}\n{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"timestamp\":\"2026-09-30T10:00:00Z\"}\n";
    let mut fixture = Fixture::new("claude-metadata", contents);
    fixture.history.adapter = "claude-code".into();
    let cutoff = NOW - 30 * DAY;
    fixture.import(cutoff);
    assert_eq!(fixture.counts(cutoff), 0);
    let source = fixture
        .database
        .source("claude-code", &fixture.history.path.to_string_lossy())
        .unwrap()
        .unwrap();
    assert_eq!(source.status, "supported");
    assert_eq!(source.diagnostics, 0);
}

#[test]
fn large_valid_tool_result_confirms_usage_without_persisting_output() {
    let mut records = synthetic(1, "2026-09-30T10:00:00Z")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    records[2]["payload"]["output"] = serde_json::json!(format!(
        "Process exited with code 0\nFinal output:\n{}",
        "x".repeat(1024 * 1024)
    ));
    let contents = records
        .iter()
        .map(|record| format!("{record}\n"))
        .collect::<String>();
    let mut fixture = Fixture::new("large-valid-result", &contents);
    let cutoff = NOW - 30 * DAY;
    fixture.import(cutoff);
    assert_eq!(fixture.counts(cutoff), 1);
    assert_eq!(fixture.source().diagnostics, 0);
    assert!(fixture.source().parser_state.len() < 4096);
}
