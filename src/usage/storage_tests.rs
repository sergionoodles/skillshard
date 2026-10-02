use super::*;
use crate::agents::by_key;
use crate::model::{AgentInstall, InstallKind, Scope, UpdateState};
use crate::tokens::TokenCost;
use crate::usage::{Activation, Evidence, Outcome, Record, Session, WorkerRole};
use std::path::PathBuf;

pub(super) const NOW: i64 = 1_800_000_000_000;
pub(super) const DAY: i64 = 86_400_000;

pub(super) fn database() -> Database {
    Database::open(Path::new(":memory:")).unwrap()
}

pub(super) fn source(path: &str) -> Source {
    Source {
        adapter: "codex".into(),
        path: path.into(),
        generation: 1,
        cutoff: NOW - DAY,
        ..Source::default()
    }
}

pub(super) fn session(native_id: &str, parent: Option<&str>) -> Session {
    Session {
        agent: "codex".into(),
        native_id: native_id.into(),
        parent_native_id: parent.map(str::to_string),
        role: if parent.is_some() {
            WorkerRole::Subagent
        } else {
            WorkerRole::Main
        },
        project: Some(PathBuf::from("/workspace/project")),
        ..Session::default()
    }
}

pub(super) fn activation(
    session: Session,
    call: &str,
    reference: &str,
    timestamp: Option<i64>,
    outcome: Outcome,
) -> Observation {
    Observation {
        locator: call.into(),
        record: Record::Activation(Activation {
            session,
            worker_id: None,
            native_id: call.into(),
            occurred_at: timestamp,
            working_directory: None,
            evidence: Evidence::SkillTool,
            reference: reference.into(),
            outcome,
        }),
    }
}

pub(super) fn snapshot(database: &Database) -> Snapshot {
    database
        .snapshot(&Query::default(), NOW - DAY, NOW)
        .unwrap()
}

pub(super) fn skill(scope: Scope, path: &str, name: &str) -> Skill {
    Skill {
        name: name.into(),
        title: None,
        description: String::new(),
        scope,
        canonical: Some(PathBuf::from(path)),
        installs: vec![AgentInstall {
            agent: by_key("codex").unwrap(),
            path: PathBuf::from(path),
            kind: InstallKind::Canonical,
        }],
        lock: None,
        disabled: false,
        update: UpdateState::Unknown,
        cost: TokenCost::default(),
    }
}

#[test]
fn repeat_import_mirrors_and_independent_siblings() {
    let mut database = database();
    let records = vec![
        activation(
            session("main", None),
            "call",
            "alpha",
            Some(NOW),
            Outcome::Succeeded,
        ),
        activation(
            session("child-a", Some("main")),
            "same-call",
            "alpha",
            Some(NOW),
            Outcome::Succeeded,
        ),
        activation(
            session("child-b", Some("main")),
            "same-call",
            "alpha",
            Some(NOW),
            Outcome::Succeeded,
        ),
    ];
    database
        .commit_batch(&source("original"), &records, NOW - DAY, true)
        .unwrap();
    database
        .commit_batch(&source("original"), &records, NOW - DAY, true)
        .unwrap();
    database
        .commit_batch(&source("mirror"), &records, NOW - DAY, true)
        .unwrap();
    let counts = snapshot(&database).counts;
    assert_eq!(
        (
            counts.activations,
            counts.sessions,
            counts.conversations,
            counts.subagent_activations
        ),
        (3, 3, 1, 2)
    );
    let mut rewritten = source("original");
    rewritten.generation = 2;
    database
        .commit_batch(&rewritten, &[], NOW - DAY, true)
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 3);
    let mut mirror = source("mirror");
    mirror.generation = 2;
    database
        .commit_batch(&mirror, &[], NOW - DAY, true)
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 0);
}

#[test]
fn child_before_parent_missing_and_cycle_are_provisional() {
    let mut database = database();
    database
        .commit_batch(
            &source("child"),
            &[activation(
                session("child", Some("parent")),
                "call",
                "alpha",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    assert!(snapshot(&database).counts.provisional_conversations);
    database
        .commit_batch(
            &source("parent"),
            &[activation(
                session("parent", None),
                "parent-call",
                "alpha",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    assert_eq!(snapshot(&database).counts.conversations, 1);
    assert!(!snapshot(&database).counts.provisional_conversations);
    let link = Observation {
        locator: "cycle".into(),
        record: Record::Link {
            agent: "codex".into(),
            parent_id: "child".into(),
            child_id: "parent".into(),
        },
    };
    database
        .commit_batch(&source("cycle"), &[link], NOW - DAY, true)
        .unwrap();
    assert!(snapshot(&database).counts.provisional_conversations);
}

#[test]
fn results_join_before_requests_and_survive_restart() {
    let root = std::env::temp_dir().join(format!(
        "skillshard-storage-{}-{:?}.sqlite3",
        std::process::id(),
        std::thread::current().id()
    ));
    let mut database = Database::open(&root).unwrap();
    let result = Observation {
        locator: "result".into(),
        record: Record::ToolResult {
            agent: "codex".into(),
            session_id: "main".into(),
            worker_id: None,
            call_id: "call".into(),
            outcome: Outcome::Succeeded,
            occurred_at: Some(NOW),
        },
    };
    database
        .commit_batch(&source("result"), &[result], NOW - DAY, false)
        .unwrap();
    drop(database);
    let mut database = Database::open(&root).unwrap();
    database
        .commit_batch(
            &source("request"),
            &[activation(
                session("main", None),
                "call",
                "alpha",
                Some(NOW),
                Outcome::Unknown,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 1);
    let failed = Observation {
        locator: "result".into(),
        record: Record::ToolResult {
            agent: "codex".into(),
            session_id: "main".into(),
            worker_id: None,
            call_id: "call".into(),
            outcome: Outcome::Failed,
            occurred_at: Some(NOW),
        },
    };
    database
        .commit_batch(&source("result"), &[failed], NOW - DAY, false)
        .unwrap();
    assert_eq!(snapshot(&database).counts.activations, 0);
    drop(database);
    std::fs::remove_file(root).unwrap();
}

#[test]
fn checkpoints_commit_atomically_and_retention_advances_coverage() {
    let mut database = database();
    let original = source("history");
    database
        .commit_batch(
            &original,
            &[activation(
                session("main", None),
                "old",
                "alpha",
                Some(NOW - DAY),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    let mut invalid = original.clone();
    invalid.offset = u64::MAX;
    assert!(database
        .commit_batch(&invalid, &[], NOW - DAY, false)
        .is_err());
    assert_eq!(
        database.source("codex", "history").unwrap().unwrap().offset,
        0
    );
    database.prune(NOW).unwrap();
    assert_eq!(
        database.source("codex", "history").unwrap().unwrap().cutoff,
        NOW
    );
    assert_eq!(snapshot(&database).counts.activations, 0);
}

#[test]
fn unknown_times_and_old_events_do_not_enter_counters() {
    let mut database = database();
    let mut source = source("history");
    source.unknown_time = 1;
    database
        .commit_batch(
            &source,
            &[
                activation(
                    session("main", None),
                    "undated",
                    "alpha",
                    None,
                    Outcome::Succeeded,
                ),
                activation(
                    session("main", None),
                    "old",
                    "alpha",
                    Some(NOW - DAY - 1),
                    Outcome::Succeeded,
                ),
                activation(
                    session("main", None),
                    "boundary",
                    "alpha",
                    Some(NOW - DAY),
                    Outcome::Unknown,
                ),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    let snapshot = snapshot(&database);
    assert_eq!(
        (
            snapshot.counts.activations,
            snapshot.counts.unconfirmed,
            snapshot.unknown_time
        ),
        (0, 1, 1)
    );
}

#[test]
fn source_format_status_and_diagnostics_survive_restart() {
    let path = std::env::temp_dir().join(format!(
        "skillshard-source-status-{}-{:?}.sqlite3",
        std::process::id(),
        std::thread::current().id()
    ));
    let mut database = Database::open(&path).unwrap();
    let mut unsupported = source("history");
    unsupported.status = "unsupported_format".into();
    unsupported.diagnostics = 3;
    database
        .commit_batch(&unsupported, &[], NOW - DAY, true)
        .unwrap();
    drop(database);
    let mut database = Database::open(&path).unwrap();
    let restored = database.source("codex", "history").unwrap().unwrap();
    assert_eq!(restored.status, "unsupported_format");
    assert_eq!(restored.diagnostics, 3);
    database
        .commit_batch(&restored, &[], NOW - DAY, false)
        .unwrap();
    assert_eq!(
        database.source("codex", "history").unwrap().unwrap().status,
        "unsupported_format"
    );
    drop(database);
    std::fs::remove_file(path).unwrap();
}
