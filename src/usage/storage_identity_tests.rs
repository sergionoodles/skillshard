use super::tests::*;
use super::*;
use crate::model::Scope;
use crate::usage::{Metric, Outcome, RoleFilter};
use std::path::PathBuf;

#[test]
fn skill_identity_relative_paths_and_ambiguous_names() {
    let mut database = database();
    let global = skill(Scope::Global, "/skills/alpha", "alpha");
    let project = skill(
        Scope::Project(PathBuf::from("/workspace/project")),
        "/workspace/project/.agents/skills/alpha",
        "alpha",
    );
    database
        .register_skills(&[global.clone(), project.clone()])
        .unwrap();
    database
        .commit_batch(
            &source("history"),
            &[
                activation(
                    session("main", None),
                    "name",
                    "alpha",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
                activation(
                    session("main", None),
                    "path",
                    ".agents/skills/alpha/SKILL.md",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    let initial = snapshot(&database);
    assert_eq!(initial.unresolved, 1);
    assert_eq!(initial.rankings.len(), 1);
    assert_eq!(initial.rankings[0].counts.activations, 1);
    let mut parked = project.clone();
    parked.disabled = true;
    parked.canonical = Some(PathBuf::from(
        "/workspace/project/.agents/.skillshard/disabled/alpha",
    ));
    parked.installs.clear();
    database.register_skills(&[global, parked]).unwrap();
    assert_eq!(snapshot(&database).rankings.len(), 2);
    assert_eq!(snapshot(&database).counts.activations, 2);
}

#[test]
fn filters_and_bucket_distinct_counts() {
    let mut database = database();
    database
        .register_skills(&[skill(Scope::Global, "/skills/alpha", "alpha")])
        .unwrap();
    database
        .commit_batch(
            &source("history"),
            &[
                activation(
                    session("main", None),
                    "first",
                    "alpha",
                    Some(NOW - DAY),
                    Outcome::Succeeded,
                ),
                activation(
                    session("main", None),
                    "repeat",
                    "alpha",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
                activation(
                    session("child", Some("main")),
                    "child",
                    "alpha",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    for metric in [Metric::Activations, Metric::Sessions, Metric::Conversations] {
        let query = Query {
            metric,
            ..Query::default()
        };
        let snapshot = database.snapshot(&query, NOW - DAY, NOW).unwrap();
        let daily_total = snapshot
            .daily
            .iter()
            .map(|bucket| bucket.value)
            .sum::<u64>();
        if metric == Metric::Activations {
            assert_eq!(daily_total, snapshot.counts.value(metric));
        } else {
            assert!(daily_total > snapshot.counts.value(metric));
        }
    }
    let query = Query {
        role: RoleFilter::Subagents,
        agent: Some("codex".into()),
        project: Some("/workspace/project".into()),
        since: Some(NOW),
        ..Query::default()
    };
    assert_eq!(
        database
            .snapshot(&query, NOW - DAY, NOW)
            .unwrap()
            .counts
            .activations,
        1
    );
    let query = Query {
        agent: Some("claude-code".into()),
        ..Query::default()
    };
    assert_eq!(
        database
            .snapshot(&query, NOW - DAY, NOW)
            .unwrap()
            .counts
            .activations,
        0
    );
}

#[test]
fn move_preserves_identity_and_historical_path() {
    let mut database = database();
    let original = skill(Scope::Global, "/skills/alpha", "alpha");
    database
        .register_skills(std::slice::from_ref(&original))
        .unwrap();
    database
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "old",
                "/skills/alpha/SKILL.md",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    let old_id = snapshot(&database).rankings[0].skill_id;
    let destination = Scope::Project("/workspace/project".into());
    database.record_move(&original, &destination).unwrap();
    let moved = skill(
        destination,
        "/workspace/project/.agents/skills/alpha",
        "alpha",
    );
    database.register_skills(&[moved]).unwrap();
    database
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "new",
                "/workspace/project/.agents/skills/alpha/SKILL.md",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            false,
        )
        .unwrap();
    assert_eq!(snapshot(&database).rankings[0].skill_id, old_id);
    assert_eq!(snapshot(&database).rankings[0].counts.activations, 2);
}

#[test]
fn newer_schema_and_failed_migration_are_visible_without_partial_tables() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    assert!(schema::initialize(&mut connection).is_err());
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE sessions(id INTEGER)")
        .unwrap();
    assert!(schema::initialize(&mut connection).is_err());
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sources'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
}

#[test]
fn independently_copied_installs_with_same_name_have_separate_identity() {
    use crate::model::{AgentInstall, InstallKind};
    let mut installed = skill(Scope::Global, "/codex/alpha", "alpha");
    installed.canonical = None;
    installed.installs = vec![
        AgentInstall {
            agent: crate::agents::by_key("codex").unwrap(),
            path: "/codex/alpha".into(),
            kind: InstallKind::Copy,
        },
        AgentInstall {
            agent: crate::agents::by_key("claude-code").unwrap(),
            path: "/claude/alpha".into(),
            kind: InstallKind::Copy,
        },
    ];
    let identities = installed_identities(&installed).unwrap();
    assert_eq!(identities.len(), 2);
    let mut database = database();
    database.register_skills(&[installed]).unwrap();
    let mut claude = session("main", None);
    claude.agent = "claude-code".into();
    database
        .commit_batch(
            &source("history"),
            &[
                activation(
                    session("main", None),
                    "codex",
                    "/codex/alpha/SKILL.md",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
                activation(
                    claude,
                    "claude",
                    "/claude/alpha/SKILL.md",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    let snapshot = snapshot(&database);
    assert_eq!(snapshot.rankings.len(), 2);
    assert_ne!(snapshot.rankings[0].skill_id, snapshot.rankings[1].skill_id);
    assert!(snapshot
        .rankings
        .iter()
        .all(|ranking| ranking.counts.activations == 1));
    assert!(identities
        .iter()
        .all(|identity| snapshot.skill_ids.contains_key(identity)));
}

#[test]
fn corrupt_database_is_visible_and_read_only_open_never_creates_files() {
    let path = std::env::temp_dir().join(format!(
        "skillshard-corrupt-{}-{:?}.sqlite3",
        std::process::id(),
        std::thread::current().id()
    ));
    assert!(Database::open_read_only(&path).is_err());
    assert!(!path.exists());
    std::fs::write(&path, b"corrupt usage database").unwrap();
    assert!(Database::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupt usage database");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn writer_contention_fails_without_advancing_cursor() {
    let path = std::env::temp_dir().join(format!(
        "skillshard-contention-{}-{:?}.sqlite3",
        std::process::id(),
        std::thread::current().id()
    ));
    let owner = Database::open(&path).unwrap();
    let mut contender = Database::open(&path).unwrap();
    contender
        .connection
        .busy_timeout(Duration::from_millis(20))
        .unwrap();
    owner.connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(contender
        .commit_batch(&source("history"), &[], NOW - DAY, true)
        .is_err());
    owner.connection.execute_batch("ROLLBACK").unwrap();
    assert!(contender.source("codex", "history").unwrap().is_none());
    drop(contender);
    drop(owner);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn explicit_move_preserves_selected_independent_copy_identity() {
    let mut original = skill(Scope::Global, "/codex/alpha", "alpha");
    original.canonical = None;
    original.installs[0].kind = crate::model::InstallKind::Copy;
    let mut database = database();
    database
        .register_skills(std::slice::from_ref(&original))
        .unwrap();
    database
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "before",
                "/codex/alpha/SKILL.md",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            true,
        )
        .unwrap();
    let old_id = snapshot(&database).rankings[0].skill_id;
    let destination = Scope::Project("/workspace/project".into());
    database.record_move(&original, &destination).unwrap();
    database
        .register_skills(&[skill(
            destination,
            "/workspace/project/.agents/skills/alpha",
            "alpha",
        )])
        .unwrap();
    database
        .commit_batch(
            &source("history"),
            &[activation(
                session("main", None),
                "after",
                "/workspace/project/.agents/skills/alpha/SKILL.md",
                Some(NOW),
                Outcome::Succeeded,
            )],
            NOW - DAY,
            false,
        )
        .unwrap();
    let snapshot = snapshot(&database);
    assert_eq!(snapshot.rankings.len(), 1);
    assert_eq!(snapshot.rankings[0].skill_id, old_id);
    assert_eq!(snapshot.rankings[0].counts.activations, 2);
}

#[test]
fn selected_skills_narrow_counts_and_days_but_rankings_list_every_skill() {
    let mut database = database();
    database
        .register_skills(&[
            skill(Scope::Global, "/skills/alpha", "alpha"),
            skill(Scope::Global, "/skills/beta", "beta"),
        ])
        .unwrap();
    database
        .commit_batch(
            &source("history"),
            &[
                activation(
                    session("main", None),
                    "a1",
                    "alpha",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
                activation(
                    session("main", None),
                    "a2",
                    "alpha",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
                activation(
                    session("main", None),
                    "b1",
                    "beta",
                    Some(NOW),
                    Outcome::Succeeded,
                ),
            ],
            NOW - DAY,
            true,
        )
        .unwrap();
    let everything = database
        .snapshot(&Query::default(), NOW - DAY, NOW)
        .unwrap();
    assert!(everything.daily_by_skill.is_empty());
    let beta = everything
        .rankings
        .iter()
        .find(|ranking| ranking.name == "beta")
        .unwrap()
        .skill_id;

    let query = Query {
        skill_ids: vec![beta],
        ..Query::default()
    };
    let selected = database.snapshot(&query, NOW - DAY, NOW).unwrap();

    assert_eq!(selected.counts.activations, 1);
    assert_eq!(
        selected
            .daily
            .iter()
            .map(|bucket| bucket.value)
            .sum::<u64>(),
        1
    );
    assert_eq!(selected.rankings.len(), 2);
    assert_eq!(selected.daily_by_skill.len(), 1);
    assert_eq!(selected.daily_by_skill[0].skill_id, beta);
    assert_eq!(selected.daily_by_skill[0].value, 1);
}

#[test]
fn reporting_range_starts_at_local_midnight_of_its_first_day() {
    use chrono::{Local, TimeZone};
    let at = |day: u32, hour: u32, minute: u32| {
        Local
            .with_ymd_and_hms(2026, 9, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    };
    let now = at(30, 12, 0);
    let mut database = database();
    database
        .register_skills(&[skill(Scope::Global, "/skills/alpha", "alpha")])
        .unwrap();
    database
        .commit_batch(
            &source("history"),
            &[
                activation(
                    session("main", None),
                    "first-day",
                    "alpha",
                    Some(at(24, 0, 30)),
                    Outcome::Succeeded,
                ),
                activation(
                    session("main", None),
                    "day-before",
                    "alpha",
                    Some(at(23, 23, 30)),
                    Outcome::Succeeded,
                ),
            ],
            now - 30 * DAY,
            true,
        )
        .unwrap();
    let query = Query {
        range_days: Some(crate::preferences::TrackingDays::Seven),
        ..Query::default()
    };

    let snapshot = database.snapshot(&query, now - 30 * DAY, now).unwrap();

    assert_eq!(snapshot.counts.activations, 1);
    assert_eq!(snapshot.daily.len(), 1);
    assert_eq!(snapshot.daily[0].label, "2026-09-24");
}
