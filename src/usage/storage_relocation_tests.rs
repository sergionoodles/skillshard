use super::tests::*;
use super::*;
use crate::model::{AgentInstall, InstallKind, Scope};
use crate::usage::Outcome;

#[test]
fn disable_and_enable_preserve_selected_copy_and_historical_group() {
    let mut original = skill(Scope::Global, "/codex/alpha", "alpha");
    original.canonical = None;
    original.installs[0].kind = InstallKind::Copy;
    original.installs.push(AgentInstall {
        agent: crate::agents::by_key("claude-code").unwrap(),
        path: "/claude/alpha".into(),
        kind: InstallKind::Copy,
    });
    let mut database = database();
    database
        .register_skills(std::slice::from_ref(&original))
        .unwrap();
    let mut claude = session("main", None);
    claude.agent = "claude-code".into();
    database
        .commit_batch(
            &source("history"),
            &[
                activation(
                    session("main", None),
                    "source",
                    "/codex/alpha/SKILL.md",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
                activation(
                    claude,
                    "other",
                    "/claude/alpha/SKILL.md",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    let initial = snapshot(&database);
    let source_id = initial.skill_ids[&installed_identities(&original).unwrap()[0]];
    database.record_disable(&original).unwrap();
    let parked_path = original.scope.disabled_dir().join(&original.name);
    let mut parked = original.clone();
    parked.canonical = Some(parked_path.clone());
    parked.installs.clear();
    parked.disabled = true;
    database
        .register_skills(std::slice::from_ref(&parked))
        .unwrap();
    database
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "parked",
                &parked_path.join("SKILL.md").to_string_lossy(),
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            false,
        )
        .unwrap();
    let disabled_snapshot = snapshot(&database);
    assert_eq!(
        disabled_snapshot
            .rankings
            .iter()
            .find(|ranking| ranking.skill_id == source_id)
            .unwrap()
            .counts
            .activations,
        2
    );
    assert_eq!(
        installed_group_ids(&parked, &disabled_snapshot)
            .unwrap()
            .len(),
        2
    );
    database.record_enable(&parked).unwrap();
    let enabled_path = original.scope.canonical_dir().join(&original.name);
    let enabled = skill(Scope::Global, &enabled_path.to_string_lossy(), "alpha");
    database
        .register_skills(std::slice::from_ref(&enabled))
        .unwrap();
    database
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "enabled",
                &enabled_path.join("SKILL.md").to_string_lossy(),
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            false,
        )
        .unwrap();
    let enabled_snapshot = snapshot(&database);
    assert_eq!(
        enabled_snapshot
            .rankings
            .iter()
            .find(|ranking| ranking.skill_id == source_id)
            .unwrap()
            .counts
            .activations,
        3
    );
    let group = installed_group_ids(&enabled, &enabled_snapshot).unwrap();
    let total: u64 = enabled_snapshot
        .rankings
        .iter()
        .filter(|ranking| group.contains(&ranking.skill_id))
        .map(|ranking| ranking.counts.activations)
        .sum();
    assert_eq!(total, 4);
}

#[test]
fn cycle_descendants_keep_deterministic_provisional_conversation_counts() {
    let mut database = database();
    let records = [
        activation(
            session("a", Some("b")),
            "a",
            "alpha",
            Some(NOW),
            Outcome::Succeeded,
        ),
        activation(
            session("b", Some("a")),
            "b",
            "alpha",
            Some(NOW),
            Outcome::Succeeded,
        ),
        activation(
            session("descendant", Some("a")),
            "descendant",
            "alpha",
            Some(NOW),
            Outcome::Succeeded,
        ),
    ];
    for _ in 0..10 {
        database
            .commit_batch(&source("cycle"), &records, NOW - DAY, true)
            .unwrap();
        let counts = snapshot(&database).counts;
        assert_eq!(counts.conversations, 3);
        assert!(counts.provisional_conversations);
    }
}

#[test]
fn cancelled_open_preserves_existing_database_and_creates_no_new_database() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let path = std::env::temp_dir().join(format!(
        "skillshard-open-cancel-{}-{:?}.sqlite3",
        std::process::id(),
        std::thread::current().id()
    ));
    let mut database = Database::open(&path).unwrap();
    database
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "call",
                "alpha",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    database.connection.execute("WITH RECURSIVE numbers(value) AS (VALUES(1) UNION ALL SELECT value+1 FROM numbers WHERE value<1000) INSERT INTO activation_events(session_id,worker_id,native_id,occurred_at,evidence,reference,outcome,deduplication_key) SELECT (SELECT id FROM sessions WHERE agent='codex' AND native_id='main'),'','synthetic-'||value,?1,'SkillTool','alpha','succeeded','synthetic-'||value FROM numbers", [NOW]).unwrap();
    database.checkpoint().unwrap();
    drop(database);
    let before = std::fs::read(&path).unwrap();
    let cancellation = Arc::new(AtomicBool::new(true));
    assert!(Database::open_with_cancellation(&path, cancellation.clone()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let missing = path.with_extension("new.sqlite3");
    assert!(Database::open_with_cancellation(&missing, cancellation.clone()).is_err());
    assert!(!missing.exists());
    cancellation.store(false, Ordering::Relaxed);
    let database = Database::open_with_cancellation(&path, cancellation.clone()).unwrap();
    assert_eq!(snapshot(&database).counts.activations, 1001);
    cancellation.store(true, Ordering::Relaxed);
    assert!(database.validate().is_err());
    drop(database);
    std::fs::remove_file(path).unwrap();
}
