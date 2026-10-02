use super::tests::*;
use super::*;
use crate::model::{InstallKind, Scope};
use crate::usage::Outcome;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn temporary_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "skillshard-identity-{label}-{}-{:?}.sqlite3",
        std::process::id(),
        std::thread::current().id()
    ))
}

#[test]
fn fresh_generation_replays_moved_disabled_copy_paths_without_reusing_history() {
    let path = temporary_path("transfer");
    let mut previous = Database::open(&path).unwrap();
    let mut original = skill(Scope::Global, "/codex/alpha", "alpha");
    original.canonical = None;
    original.installs[0].kind = InstallKind::Copy;
    previous
        .register_skills(std::slice::from_ref(&original))
        .unwrap();
    let old_record = activation(
        session("main", None),
        "old",
        "/codex/alpha/SKILL.md",
        Some(NOW),
        Outcome::Succeeded,
    );
    previous
        .commit_batch(
            &source("original"),
            std::slice::from_ref(&old_record),
            NOW - DAY,
            true,
        )
        .unwrap();
    let destination = Scope::Project("/workspace/project".into());
    previous.record_move(&original, &destination).unwrap();
    let moved = skill(
        destination.clone(),
        "/workspace/project/.agents/skills/alpha",
        "alpha",
    );
    previous
        .register_skills(std::slice::from_ref(&moved))
        .unwrap();
    previous.record_disable(&moved).unwrap();
    let parked_path = destination.disabled_dir().join("alpha");
    let mut parked = moved.clone();
    parked.canonical = Some(parked_path.clone());
    parked.installs.clear();
    parked.disabled = true;
    previous
        .register_skills(std::slice::from_ref(&parked))
        .unwrap();
    previous.checkpoint().unwrap();
    drop(previous);
    let before = std::fs::read(&path).unwrap();
    let mut fresh = database();
    let cancellation = Arc::new(AtomicBool::new(false));
    assert_eq!(
        fresh
            .copy_identity_metadata_from(&path, cancellation.clone())
            .unwrap(),
        IdentityTransfer::Copied
    );
    assert_eq!(snapshot(&fresh).counts.activations, 0);
    assert!(fresh.source("codex", "original").unwrap().is_none());
    fresh
        .register_skills(std::slice::from_ref(&parked))
        .unwrap();
    let moved_record = activation(
        session("main", None),
        "moved",
        "/workspace/project/.agents/skills/alpha/SKILL.md",
        Some(NOW),
        Outcome::Succeeded,
    );
    let parked_record = activation(
        session("main", None),
        "parked",
        &parked_path.join("SKILL.md").to_string_lossy(),
        Some(NOW),
        Outcome::Succeeded,
    );
    fresh
        .commit_batch(
            &source("replay"),
            &[old_record, moved_record, parked_record],
            NOW - DAY,
            true,
        )
        .unwrap();
    let rebuilt = snapshot(&fresh);
    assert_eq!(rebuilt.unresolved, 0);
    assert_eq!(rebuilt.rankings.len(), 1);
    assert_eq!(rebuilt.rankings[0].counts.activations, 3);
    assert_eq!(
        fresh
            .copy_identity_metadata_from(&path, cancellation)
            .unwrap(),
        IdentityTransfer::Copied
    );
    assert_eq!(snapshot(&fresh).rankings.len(), 1);
    assert_eq!(snapshot(&fresh).counts.activations, 3);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn unavailable_metadata_does_not_block_fresh_database_or_leave_partial_copies() {
    let path = temporary_path("unavailable");
    std::fs::write(&path, b"corrupt old database").unwrap();
    let mut fresh = database();
    let cancellation = Arc::new(AtomicBool::new(false));
    assert!(matches!(
        fresh
            .copy_identity_metadata_from(&path, cancellation.clone())
            .unwrap(),
        IdentityTransfer::Unavailable(_)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupt old database");
    std::fs::remove_file(&path).unwrap();
    let partial = Connection::open(&path).unwrap();
    partial.execute_batch("PRAGMA user_version=1; CREATE TABLE skills(id INTEGER PRIMARY KEY,identity TEXT,name TEXT,installed INTEGER); INSERT INTO skills VALUES(1,'identity','alpha',1);").unwrap();
    drop(partial);
    assert!(matches!(
        fresh
            .copy_identity_metadata_from(&path, cancellation)
            .unwrap(),
        IdentityTransfer::Unavailable(_)
    ));
    let count: i64 = fresh
        .connection
        .query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    fresh
        .register_skills(&[skill(Scope::Global, "/skills/alpha", "alpha")])
        .unwrap();
    fresh.validate().unwrap();
    std::fs::remove_file(path).unwrap();
}

#[test]
fn metadata_transfer_and_read_only_open_honor_cancellation() {
    let path = temporary_path("cancellation");
    let mut previous = Database::open(&path).unwrap();
    previous
        .register_skills(&[skill(Scope::Global, "/skills/alpha", "alpha")])
        .unwrap();
    drop(previous);
    let cancellation = Arc::new(AtomicBool::new(true));
    let mut fresh = database();
    assert!(fresh
        .copy_identity_metadata_from(&path, cancellation.clone())
        .is_err());
    assert!(Database::open_read_only_with_cancellation(&path, cancellation.clone()).is_err());
    cancellation.store(false, Ordering::Relaxed);
    let reader = Database::open_read_only_with_cancellation(&path, cancellation.clone()).unwrap();
    cancellation.store(true, Ordering::Relaxed);
    assert!(reader.connection.query_row("WITH RECURSIVE numbers(value) AS (VALUES(1) UNION ALL SELECT value+1 FROM numbers WHERE value<10000) SELECT SUM(value) FROM numbers", [], |row| row.get::<_,i64>(0)).is_err());
    drop(reader);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn path_reuse_keeps_old_attribution_and_new_ambiguous_reference_unresolved() {
    let mut fresh = database();
    let original = skill(Scope::Global, "/skills/reused", "original");
    fresh.register_skills(&[original]).unwrap();
    fresh
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "old",
                "/skills/reused/SKILL.md",
                Some(NOW - DAY),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    let old_id = snapshot(&fresh).rankings[0].skill_id;
    fresh
        .register_skills(&[skill(Scope::Global, "/skills/reused", "replacement")])
        .unwrap();
    fresh
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "new",
                "/skills/reused/SKILL.md",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            false,
        )
        .unwrap();
    let snapshot = snapshot(&fresh);
    assert_eq!(snapshot.unresolved, 1);
    assert_eq!(snapshot.rankings[0].skill_id, old_id);
    assert_eq!(snapshot.rankings[0].counts.activations, 1);
}
