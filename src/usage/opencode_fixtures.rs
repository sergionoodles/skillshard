use super::super::*;
use crate::agents::by_key;
use crate::model::{AgentInstall, InstallKind, Scope, Skill, UpdateState};
use crate::tokens::TokenCost;
use crate::usage::Query;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::sync::atomic::AtomicU64;

static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

pub(super) struct OwnedDirectory(std::path::PathBuf);

impl OwnedDirectory {
    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub(super) const NOW: i64 = 100_000;

pub(super) struct Fixture {
    pub(super) _directory: OwnedDirectory,
    pub(super) path: std::path::PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let directory = OwnedDirectory(std::env::temp_dir().join(format!(
            "skillshard-opencode-{}-{}",
            std::process::id(),
            FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir_all(directory.path()).unwrap();
        let path = directory.path().join("opencode.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE session_v2(id TEXT PRIMARY KEY,parent_id TEXT,directory TEXT,path TEXT,agent TEXT,model TEXT,fork_session_id TEXT,fork_boundary TEXT); CREATE TABLE session_message(id TEXT PRIMARY KEY,session_id TEXT,type TEXT,seq INTEGER,time_created INTEGER,time_updated INTEGER,data TEXT); CREATE INDEX message_created ON session_message(time_created); CREATE INDEX message_sequence ON session_message(session_id,seq);").unwrap();
        Self {
            _directory: directory,
            path,
        }
    }

    pub(super) fn session(&self, id: &str, parent: Option<&str>, fork: Option<(&str, Value)>) {
        let (fork_id, boundary) = fork.map_or((None, None), |(id, boundary)| {
            (Some(id), Some(boundary.to_string()))
        });
        Connection::open(&self.path).unwrap().execute("INSERT INTO session_v2 VALUES(?1,?2,'/project',NULL,'build','{\"id\":\"test-model\"}',?3,?4)",params![id,parent,fork_id,boundary]).unwrap();
    }

    pub(super) fn message(
        &self,
        id: &str,
        session: &str,
        sequence: i64,
        updated: i64,
        kind: &str,
        data: Value,
    ) {
        Connection::open(&self.path).unwrap().execute("INSERT INTO session_message VALUES(?1,?2,?3,?4,1000,?5,?6) ON CONFLICT(id) DO UPDATE SET data=excluded.data,time_updated=excluded.time_updated",params![id,session,kind,sequence,updated,data.to_string()]).unwrap();
    }

    pub(super) fn delete(&self, id: &str) {
        Connection::open(&self.path)
            .unwrap()
            .execute("DELETE FROM session_message WHERE id=?1", [id])
            .unwrap();
    }
}

pub(super) fn tool(status: &str) -> Value {
    json!({"agent":"build","model":{"id":"test-model"},"content":[{"type":"tool","id":"native-call","name":"skill","time":{"created":1000,"ran":1000},"state":{"status":status,"input":{"id":"alpha"},"metadata":{"directory":"/skills/alpha"},"content":[{"type":"text","text":"private output never persisted"}]}}]})
}

pub(super) fn database() -> Database {
    let mut database = Database::open(Path::new(":memory:")).unwrap();
    database
        .register_skills(&[Skill {
            name: "alpha".into(),
            title: None,
            description: String::new(),
            scope: Scope::Global,
            canonical: Some("/skills/alpha".into()),
            installs: vec![AgentInstall {
                agent: by_key("opencode").unwrap(),
                path: "/skills/alpha".into(),
                kind: InstallKind::Canonical,
            }],
            lock: None,
            disabled: false,
            update: UpdateState::Unknown,
            cost: TokenCost::default(),
        }])
        .unwrap();
    database
}

pub(super) fn import_all(database: &mut Database, fixture: &Fixture, cutoff: i64) {
    for _ in 0..32 {
        let progress = import_batch(
            database,
            &fixture.path,
            cutoff,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        if progress.complete {
            return;
        }
    }
    panic!("bounded import failed to finish");
}

pub(super) fn counts(database: &Database) -> crate::usage::Counts {
    database.snapshot(&Query::default(), 0, NOW).unwrap().counts
}
