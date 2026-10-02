use super::tests::*;
use super::*;
use crate::usage::{Outcome, Record, Session, WorkerRole};

fn entry(session: Session, locator: &str, entry_id: &str, inherits_parent: bool) -> Observation {
    Observation {
        locator: locator.into(),
        record: Record::SessionEntry {
            session,
            entry_id: entry_id.into(),
            inherits_parent,
            occurred_at: Some(NOW),
        },
    }
}

fn path(session: Session, path: &str) -> Observation {
    Observation {
        locator: "header".into(),
        record: Record::SessionPath {
            session,
            path: path.into(),
        },
    }
}

#[test]
fn child_before_parent_reconciles_paths_and_exact_inherited_entries_reversibly() {
    let mut database = database();
    let child = session("child-native", Some("/sessions/custom-parent.jsonl"));
    let mut original = source("child");
    let records = vec![
        path(child.clone(), "/sessions/arbitrary-child.jsonl"),
        entry(child.clone(), "copied", "copied-entry", true),
        activation(
            child.clone(),
            "copied",
            "alpha",
            Some(NOW),
            Outcome::Succeeded,
        ),
        entry(child.clone(), "new", "new-entry", true),
        activation(child, "new", "alpha", Some(NOW), Outcome::Succeeded),
    ];
    database
        .commit_batch(&original, &records, NOW - DAY, true)
        .unwrap();
    assert!(snapshot(&database).counts.provisional_conversations);
    let parent = session("parent-native", None);
    let mut parent_source = source("parent");
    let parent_records = vec![
        path(parent.clone(), "/sessions/custom-parent.jsonl"),
        entry(parent.clone(), "copied", "copied-entry", false),
        activation(parent, "copied", "alpha", Some(NOW), Outcome::Succeeded),
    ];
    database
        .commit_batch(&parent_source, &parent_records, NOW - DAY, true)
        .unwrap();
    let counts = snapshot(&database).counts;
    assert_eq!(
        (counts.activations, counts.sessions, counts.conversations),
        (2, 2, 1)
    );
    assert_eq!(
        (counts.main_activations, counts.subagent_activations),
        (1, 1)
    );
    assert!(!counts.provisional_conversations);
    database
        .commit_batch(&original, &records, NOW - DAY, true)
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 2);

    parent_source.generation += 1;
    database
        .commit_batch(&parent_source, &[], NOW - DAY, true)
        .unwrap();
    assert_eq!(
        snapshot(&database).counts.activations,
        2,
        "child evidence becomes independent when copied parent evidence is removed"
    );
    original.generation += 1;
    database
        .commit_batch(&original, &[], NOW - DAY, true)
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 0);
}

#[test]
fn fork_relationship_does_not_change_main_role_and_reused_paths_stay_provisional() {
    let mut database = database();
    let mut fork = session("fork", Some("/sessions/parent.jsonl"));
    fork.role = WorkerRole::Main;
    database
        .commit_batch(
            &source("fork"),
            &[activation(
                fork.clone(),
                "fork-new",
                "alpha",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    for native_id in ["parent-a", "parent-b"] {
        let parent = session(native_id, None);
        database
            .commit_batch(
                &source(native_id),
                &[path(parent, "/sessions/parent.jsonl")],
                NOW - DAY,
                true,
            )
            .unwrap();
    }
    let counts = snapshot(&database).counts;
    assert_eq!(counts.main_activations, 1);
    assert_eq!(counts.subagent_activations, 0);
    assert!(counts.provisional_conversations);
}

#[test]
fn cyclic_parent_metadata_does_not_suppress_observed_entries_as_inherited() {
    let mut database = database();
    let self_parent = session("self", Some("/sessions/self.jsonl"));
    database
        .commit_batch(
            &source("self"),
            &[
                path(self_parent.clone(), "/sessions/self.jsonl"),
                entry(self_parent.clone(), "call", "entry", true),
                activation(self_parent, "call", "alpha", Some(NOW), Outcome::Succeeded),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    let counts = snapshot(&database).counts;
    assert_eq!(counts.activations, 1);
    assert!(counts.provisional_conversations);
}

#[test]
fn copied_results_cannot_override_independent_child_evidence_and_mirrors_survive_replay() {
    let mut database = database();
    let child = session("child", Some("/sessions/parent.jsonl"));
    let failed_result = |session: Session| Observation {
        locator: "result".into(),
        record: Record::ToolResult {
            agent: session.agent,
            session_id: session.native_id,
            worker_id: None,
            call_id: "call".into(),
            outcome: Outcome::Failed,
            occurred_at: Some(NOW),
        },
    };
    let records = vec![
        path(child.clone(), "/sessions/child.jsonl"),
        entry(child.clone(), "result", "copied-result", true),
        failed_result(child.clone()),
        entry(child.clone(), "call", "independent-call", true),
        activation(child, "call", "alpha", Some(NOW), Outcome::Succeeded),
    ];
    let mut child_source = source("child");
    database
        .commit_batch(&child_source, &records, NOW - DAY, true)
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 0);
    let parent = session("parent", None);
    database
        .commit_batch(
            &source("parent"),
            &[
                path(parent.clone(), "/sessions/parent.jsonl"),
                entry(parent.clone(), "result", "copied-result", false),
                failed_result(parent),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 1);
    database
        .commit_batch(&source("child-mirror"), &records, NOW - DAY, true)
        .unwrap();
    child_source.generation += 1;
    database
        .commit_batch(&child_source, &[], NOW - DAY, true)
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 1);
    assert_eq!(snapshot(&database).counts.subagent_activations, 1);
}

#[test]
fn corrected_session_header_replaces_old_parent_metadata_without_erasing_result_context() {
    let mut database = database();
    let parent = session("parent", None);
    database
        .commit_batch(
            &source("parent"),
            &[activation(
                parent,
                "parent-call",
                "alpha",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    let child = session("child", Some("parent"));
    let mut history = source("child");
    database
        .commit_batch(
            &history,
            &[
                Observation {
                    locator: "header".into(),
                    record: Record::Session(child.clone()),
                },
                activation(child, "call", "alpha", Some(NOW), Outcome::Succeeded),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    assert_eq!(snapshot(&database).counts.conversations, 1);
    history.generation += 1;
    let corrected = session("child", None);
    database
        .commit_batch(
            &history,
            &[
                Observation {
                    locator: "header".into(),
                    record: Record::Session(corrected.clone()),
                },
                activation(corrected, "call", "alpha", Some(NOW), Outcome::Succeeded),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    assert_eq!(snapshot(&database).counts.conversations, 2);
    assert_eq!(snapshot(&database).counts.main_activations, 2);
}
