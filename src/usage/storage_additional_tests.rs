use super::tests::*;
use super::*;
use crate::model::Scope;
use crate::usage::{Outcome, Record};

#[test]
fn replay_only_replaces_old_generation_after_completion() {
    let mut database = database();
    database
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "original",
                "alpha",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    let mut rewritten = source("history");
    rewritten.generation = 2;
    database
        .commit_batch(
            &rewritten,
            &[activation(
                session("main", None),
                "replacement",
                "alpha",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            false,
        )
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 2);
    database
        .commit_batch(&rewritten, &[], NOW - DAY, true)
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 1);
}

#[test]
fn local_midnight_buckets_use_event_time() {
    use chrono::{Local, TimeZone};
    let midnight = Local
        .with_ymd_and_hms(2026, 10, 1, 0, 0, 0)
        .single()
        .unwrap()
        .timestamp_millis();
    let mut database = database();
    database
        .commit_batch(
            &source("history"),
            &[
                activation(
                    session("main", None),
                    "before",
                    "alpha",
                    Some(midnight - 1),
                    Outcome::Succeeded,
                ),
                activation(
                    session("main", None),
                    "after",
                    "alpha",
                    Some(midnight),
                    Outcome::Succeeded,
                ),
            ],
            midnight - DAY,
            true,
        )
        .unwrap();
    let snapshot = database
        .snapshot(&Query::default(), midnight - DAY, midnight)
        .unwrap();
    assert_eq!(
        snapshot.daily,
        vec![
            crate::usage::Bucket {
                label: "2026-09-30".into(),
                value: 1
            },
            crate::usage::Bucket {
                label: "2026-10-01".into(),
                value: 1
            }
        ]
    );
}

#[test]
fn symlink_aliases_and_inline_workers_stay_attributed() {
    let root = std::env::temp_dir().join(format!(
        "skillshard-alias-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let canonical = root.join("canonical/alpha");
    let linked = root.join("linked-alpha");
    std::fs::create_dir_all(&canonical).unwrap();
    std::fs::write(canonical.join("SKILL.md"), "test").unwrap();
    std::os::unix::fs::symlink(&canonical, &linked).unwrap();
    let mut installed = skill(Scope::Global, &linked.to_string_lossy(), "plugin:alpha");
    installed.canonical = Some(linked.clone());
    let mut database = database();
    database.register_skills(&[installed]).unwrap();
    let mut event = activation(
        session("main", None),
        "call",
        &canonical.join("SKILL.md").to_string_lossy(),
        Some(NOW),
        Outcome::Succeeded,
    );
    if let Record::Activation(activation) = &mut event.record {
        activation.worker_id = Some("child-worker".into());
    }
    database
        .commit_batch(
            &source("history"),
            &[
                event,
                activation(
                    session("main", None),
                    "name",
                    "plugin:alpha",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
                activation(
                    session("main", None),
                    "bare",
                    "alpha",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    let snapshot = snapshot(&database);
    assert_eq!(snapshot.rankings[0].counts.activations, 2);
    assert_eq!(snapshot.counts.subagent_activations, 1);
    assert_eq!(snapshot.unresolved, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_working_directory_resolves_relative_path_with_project_context() {
    let mut database = database();
    let installed = skill(
        Scope::Project("/workspace/project".into()),
        "/workspace/project/nested/.agents/skills/alpha",
        "alpha",
    );
    database.register_skills(&[installed]).unwrap();
    let mut event = activation(
        session("main", None),
        "call",
        ".agents/skills/alpha/SKILL.md",
        Some(NOW),
        Outcome::Succeeded,
    );
    if let Record::Activation(activation) = &mut event.record {
        activation.working_directory = Some("/workspace/project/nested".into());
    }
    database
        .commit_batch(&source("history"), &[event], NOW - DAY, true)
        .unwrap();
    assert_eq!(snapshot(&database).unresolved, 0);
    assert_eq!(snapshot(&database).rankings[0].counts.activations, 1);
    database
        .register_skills(&[skill(
            Scope::Project("/workspace/project".into()),
            "/workspace/project/nested/.agents/skills/alpha",
            "alpha",
        )])
        .unwrap();
    assert_eq!(snapshot(&database).unresolved, 0);
}

#[test]
fn unknown_result_orphans_are_bounded_and_dated_results_expire() {
    let mut database = database();
    let source = source("results");
    let results: Vec<_> = (0..1100)
        .map(|index| Observation {
            locator: index.to_string(),
            record: Record::ToolResult {
                agent: "codex".into(),
                session_id: "main".into(),
                worker_id: None,
                call_id: index.to_string(),
                outcome: Outcome::Succeeded,
                occurred_at: None,
            },
        })
        .collect();
    database
        .commit_batch(&source, &results, NOW - DAY, true)
        .unwrap();
    let count: i64 = database
        .connection
        .query_row("SELECT COUNT(*) FROM tool_results", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1024);
    let old = Observation {
        locator: "dated-old".into(),
        record: Record::ToolResult {
            agent: "codex".into(),
            session_id: "main".into(),
            worker_id: None,
            call_id: "dated-old".into(),
            outcome: Outcome::Succeeded,
            occurred_at: Some(NOW - DAY - 1),
        },
    };
    database
        .commit_batch(&source, &[old], NOW - DAY, false)
        .unwrap();
    let count: i64 = database
        .connection
        .query_row(
            "SELECT COUNT(*) FROM tool_results WHERE occurred_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn late_session_project_metadata_resolves_retained_event() {
    let mut database = database();
    database
        .register_skills(&[skill(
            Scope::Project("/workspace/project".into()),
            "/workspace/project/.agents/skills/alpha",
            "alpha",
        )])
        .unwrap();
    let mut unknown_project = session("main", None);
    unknown_project.project = None;
    database
        .commit_batch(
            &source("history"),
            &[activation(
                unknown_project,
                "call",
                ".agents/skills/alpha/SKILL.md",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    assert_eq!(snapshot(&database).unresolved, 1);
    database
        .commit_batch(
            &source("metadata"),
            &[Observation {
                locator: "session".into(),
                record: Record::Session(session("main", None)),
            }],
            NOW - DAY,
            true,
        )
        .unwrap();
    assert_eq!(snapshot(&database).unresolved, 0);
}

#[test]
fn sql_cancellation_interrupts_snapshot_and_rolls_back_cursor() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let mut database = database();
    let records: Vec<_> = (0..100)
        .map(|index| {
            activation(
                session("main", None),
                &index.to_string(),
                "alpha",
                Some(NOW),
                Outcome::Succeeded,
            )
        })
        .collect();
    let original = source("history");
    database
        .commit_batch(&original, &records, NOW - DAY, true)
        .unwrap();
    let cancellation = Arc::new(AtomicBool::new(true));
    database.set_cancellation(cancellation.clone()).unwrap();
    assert!(database
        .snapshot(&Query::default(), NOW - DAY, NOW)
        .is_err());
    let mut advanced = original;
    advanced.offset = 500;
    assert!(database
        .commit_batch(&advanced, &records, NOW - DAY, false)
        .is_err());
    cancellation.store(false, Ordering::Relaxed);
    assert_eq!(
        database.source("codex", "history").unwrap().unwrap().offset,
        0
    );
    assert_eq!(snapshot(&database).counts.activations, 100);
}

#[test]
fn multiple_results_in_one_source_record_preserve_distinct_outcomes() {
    let mut database = database();
    let request = vec![
        activation(
            session("main", None),
            "success",
            "alpha",
            Some(NOW),
            Outcome::Unknown,
        ),
        activation(
            session("main", None),
            "failure",
            "alpha",
            Some(NOW),
            Outcome::Unknown,
        ),
    ];
    database
        .commit_batch(&source("history"), &request, NOW - DAY, true)
        .unwrap();
    let results: Vec<_> = [
        ("success", Outcome::Succeeded),
        ("failure", Outcome::Failed),
    ]
    .into_iter()
    .map(|(call, outcome)| Observation {
        locator: "same-record".into(),
        record: Record::ToolResult {
            agent: "codex".into(),
            session_id: "main".into(),
            worker_id: None,
            call_id: call.into(),
            occurred_at: Some(NOW),
            outcome,
        },
    })
    .collect();
    database
        .commit_batch(&source("results"), &results, NOW - DAY, true)
        .unwrap();
    let snapshot = snapshot(&database);
    assert_eq!(snapshot.counts.activations, 1);
    assert_eq!(snapshot.counts.unconfirmed, 0);
}
