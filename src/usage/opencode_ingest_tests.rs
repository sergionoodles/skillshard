use super::*;
use crate::usage::Query;
use rusqlite::Connection;
use serde_json::json;

#[path = "opencode_fixtures.rs"]
mod fixtures;
use fixtures::*;

#[test]
fn main_and_child_counts_outcomes_and_read_only_source() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    fixture.session("child", Some("main"), None);
    fixture.message("main-call", "main", 1, 2000, "assistant", tool("completed"));
    fixture.message(
        "child-call",
        "child",
        1,
        2000,
        "assistant",
        tool("completed"),
    );
    fixture.message("failed-call", "main", 2, 2001, "assistant", tool("error"));
    let original = fs::read(&fixture.path).unwrap();
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    let count = counts(&database);
    assert_eq!(
        (
            count.activations,
            count.main_activations,
            count.subagent_activations,
            count.sessions,
            count.conversations
        ),
        (2, 1, 1, 2, 1)
    );
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database), count);
    assert_eq!(fs::read(&fixture.path).unwrap(), original);
    let checkpoint = database
        .source("opencode", &fixture.path.to_string_lossy())
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.status, "supported");
    assert!(!checkpoint.parser_state.contains("private output"));
}

#[test]
fn mutable_results_removed_evidence_and_deleted_native_rows_reconcile() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    fixture.message("message", "main", 1, 2000, "assistant", tool("running"));
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    assert_eq!(
        (counts(&database).activations, counts(&database).unconfirmed),
        (0, 1)
    );
    fixture.message("message", "main", 1, 3000, "assistant", tool("completed"));
    import_all(&mut database, &fixture, 0);
    assert_eq!(
        (counts(&database).activations, counts(&database).unconfirmed),
        (1, 0)
    );
    fixture.message(
        "message",
        "main",
        1,
        4000,
        "assistant",
        json!({"content":[]}),
    );
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 0);
    fixture.message("message", "main", 1, 5000, "assistant", tool("completed"));
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 1);
    fixture.delete("message");
    fixture.message(
        "replacement",
        "main",
        2,
        6000,
        "user",
        json!({"text":"mere mention alpha"}),
    );
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 0);
}

#[test]
fn checkpoint_resumes_bounded_batch_and_does_not_merge_same_time_siblings() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    for sequence in 0..300 {
        fixture.message(
            &format!("message-{sequence:04}"),
            "main",
            sequence,
            2000,
            "assistant",
            tool("completed"),
        );
    }
    let path = fixture._directory.path().join("usage.sqlite3");
    let mut database = Database::open(&path).unwrap();
    let first = import_batch(
        &mut database,
        &fixture.path,
        0,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert_eq!(first.records, 256);
    assert!(!first.complete);
    assert_eq!(counts(&database).activations, 256);
    drop(database);
    let mut database = Database::open(&path).unwrap();
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 300);
}

#[test]
fn fork_projection_excludes_exact_inherited_prefix_but_keeps_later_calls() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    fixture.session(
        "fork",
        None,
        Some(("main", json!({"type":"after","messageID":"boundary"}))),
    );
    fixture.message("boundary", "main", 10, 2000, "assistant", tool("completed"));
    fixture.message(
        "copied-with-new-id",
        "fork",
        10,
        2000,
        "assistant",
        tool("completed"),
    );
    fixture.message("new-call", "fork", 11, 2001, "assistant", tool("completed"));
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    assert_eq!(
        (
            counts(&database).activations,
            counts(&database).sessions,
            counts(&database).conversations
        ),
        (2, 2, 2)
    );
}

#[test]
fn unsupported_schema_and_cancellation_are_visible_without_reading_private_history() {
    let fixture = Fixture::new();
    Connection::open(&fixture.path)
        .unwrap()
        .execute("DROP TABLE session_message", [])
        .unwrap();
    let mut database = database();
    let stopped = import_batch(
        &mut database,
        &fixture.path,
        0,
        Arc::new(AtomicBool::new(true)),
    )
    .unwrap();
    assert!(!stopped.committed);
    assert!(database
        .source("opencode", &fixture.path.to_string_lossy())
        .unwrap()
        .is_none());
    import_all(&mut database, &fixture, 0);
    let source = database
        .source("opencode", &fixture.path.to_string_lossy())
        .unwrap()
        .unwrap();
    assert_eq!(source.status, "unsupported_format");
    assert_eq!(source.diagnostics, 1);
    import_all(&mut database, &fixture, 0);
    assert_eq!(
        database
            .source("opencode", &fixture.path.to_string_lossy())
            .unwrap()
            .unwrap()
            .status,
        "unsupported_format"
    );
}

#[test]
fn malformed_and_oversized_rows_are_bounded_diagnostics_without_false_usage() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    fixture.message(
        "unknown",
        "main",
        1,
        2000,
        "future-shape",
        json!({"text":"alpha"}),
    );
    fixture.message(
        "huge",
        "main",
        2,
        2001,
        "assistant",
        json!({"content":[],"unused":"x".repeat(MAX_RECORD_BYTES)}),
    );
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 0);
    let source = database
        .source("opencode", &fixture.path.to_string_lossy())
        .unwrap()
        .unwrap();
    assert_eq!(source.status, "unsupported_format");
    assert_eq!(source.diagnostics, 2);
    fixture.message("mention", "main", 3, 2002, "user", json!({"text":"alpha"}));
    import_all(&mut database, &fixture, 0);
    assert_eq!(
        database
            .source("opencode", &fixture.path.to_string_lossy())
            .unwrap()
            .unwrap()
            .diagnostics,
        2
    );
}

#[test]
fn old_assistant_row_with_newly_added_tool_is_found_beyond_overlap() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    fixture.message("old", "main", 1, 2000, "assistant", json!({"content":[]}));
    fixture.message(
        "recent",
        "main",
        2,
        90_000,
        "user",
        json!({"text":"mention"}),
    );
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 0);
    fixture.message("old", "main", 1, 95_000, "assistant", tool("completed"));
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 1);
}

#[test]
fn historical_location_resolves_relative_read_before_later_session_move() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    Connection::open(&fixture.path)
        .unwrap()
        .execute("UPDATE session_v2 SET directory='/after'", [])
        .unwrap();
    let read = json!({"content":[{"type":"tool","id":"read-call","name":"read","time":{"created":1000},"state":{"status":"completed","input":{"path":"alpha/SKILL.md"},"content":[{"type":"text","text":"delivered instructions"}]}}]});
    fixture.message("old-read", "main", 1, 2000, "assistant", read);
    fixture.message(
        "move",
        "main",
        2,
        2001,
        "location-switched",
        json!({"location":{"directory":"/after"},"previous":{"location":{"directory":"/skills"}}}),
    );
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    let snapshot = database.snapshot(&Query::default(), 0, NOW).unwrap();
    assert_eq!(snapshot.counts.activations, 1);
    assert_eq!(snapshot.unresolved, 0);
}

#[test]
fn size_budget_limits_allocated_rows_and_unknown_times_do_not_repeat_on_overlap() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    for sequence in 0..40 {
        fixture.message(
            &format!("large-{sequence:04}"),
            "main",
            sequence,
            2000,
            "user",
            json!({"text":"x".repeat(300_000)}),
        );
    }
    let mut no_time = tool("completed");
    no_time["content"][0]
        .as_object_mut()
        .unwrap()
        .remove("time");
    fixture.message("unknown-time", "main", 41, 2001, "assistant", no_time);
    let mut database = database();
    let progress = import_batch(
        &mut database,
        &fixture.path,
        0,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(progress.records > 0 && progress.records < 40);
    assert!(!progress.complete);
    import_all(&mut database, &fixture, 0);
    assert_eq!(
        database
            .snapshot(&Query::default(), 0, NOW)
            .unwrap()
            .unknown_time,
        1
    );
    fixture.message(
        "mention",
        "main",
        22,
        2002,
        "user",
        json!({"text":"mention"}),
    );
    import_all(&mut database, &fixture, 0);
    assert_eq!(
        database
            .snapshot(&Query::default(), 0, NOW)
            .unwrap()
            .unknown_time,
        1
    );
}

#[test]
fn deletion_from_one_database_keeps_observation_from_another_mirror() {
    let original = Fixture::new();
    original.session("main", None, None);
    original.message("message", "main", 1, 2000, "assistant", tool("completed"));
    let mirror = Fixture::new();
    fs::copy(&original.path, &mirror.path).unwrap();
    let mut database = database();
    import_all(&mut database, &original, 0);
    import_all(&mut database, &mirror, 0);
    assert_eq!(counts(&database).activations, 1);
    original.delete("message");
    import_all(&mut database, &original, 0);
    assert_eq!(counts(&database).activations, 1);
    mirror.delete("message");
    import_all(&mut database, &mirror, 0);
    assert_eq!(counts(&database).activations, 0);
}

#[test]
fn expanding_retention_replays_bounded_old_records() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    fixture.message("old", "main", 1, 2000, "assistant", tool("completed"));
    let mut recent = tool("completed");
    recent["content"][0]["time"] = json!({"created":9000});
    fixture.message("recent", "main", 2, 10_000, "assistant", recent);
    let mut database = database();
    import_all(&mut database, &fixture, 5000);
    assert_eq!(counts(&database).activations, 1);
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 2);
}

#[path = "opencode_fork_tests.rs"]
mod fork_tests;

#[path = "opencode_location_tests.rs"]
mod location_tests;

#[path = "opencode_size_tests.rs"]
mod size_tests;
