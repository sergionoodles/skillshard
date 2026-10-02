use super::UsageResult;
use rusqlite::Connection;

const SCHEMA_VERSION: i64 = 1;

pub(super) fn initialize(connection: &mut Connection) -> UsageResult<()> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "Usage database schema {version} is newer than supported {SCHEMA_VERSION}"
        ));
    }
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(SCHEMA)
        .map_err(|error| error.to_string())?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

const SCHEMA: &str = "
CREATE TABLE sources (
 id INTEGER PRIMARY KEY, adapter TEXT NOT NULL, path TEXT NOT NULL,
 generation INTEGER NOT NULL, size INTEGER NOT NULL, modified TEXT NOT NULL,
 identity TEXT NOT NULL, offset INTEGER NOT NULL, fingerprint TEXT NOT NULL,
 parser_version INTEGER NOT NULL, cutoff INTEGER NOT NULL, parser_state TEXT NOT NULL,
 diagnostics INTEGER NOT NULL, unknown_time INTEGER NOT NULL, status TEXT NOT NULL DEFAULT '', UNIQUE(adapter,path)
);
CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE);
CREATE TABLE history_locations (
 adapter TEXT NOT NULL, path TEXT NOT NULL, identity TEXT NOT NULL,
 PRIMARY KEY(adapter,path)
);
CREATE TABLE sessions (
 id INTEGER PRIMARY KEY, agent TEXT NOT NULL, native_id TEXT NOT NULL,
 parent_native_id TEXT, parent_id INTEGER REFERENCES sessions(id),
 root_id INTEGER REFERENCES sessions(id), provisional INTEGER NOT NULL DEFAULT 0,
 ancestry_cycle INTEGER NOT NULL DEFAULT 0,
 role TEXT NOT NULL, worker_name TEXT, model TEXT, project_id INTEGER REFERENCES projects(id),
 UNIQUE(agent,native_id)
);
CREATE INDEX session_parent ON sessions(parent_id);
CREATE INDEX session_root ON sessions(root_id);
CREATE INDEX session_project ON sessions(project_id,agent);
CREATE TABLE session_metadata (
 source_id INTEGER NOT NULL REFERENCES sources(id), generation INTEGER NOT NULL,
 session_id INTEGER NOT NULL REFERENCES sessions(id), parent_native_id TEXT, role TEXT NOT NULL,
 PRIMARY KEY(source_id,generation,session_id)
);
CREATE INDEX session_metadata_lookup ON session_metadata(session_id);
CREATE TABLE session_paths (
 source_id INTEGER NOT NULL REFERENCES sources(id), generation INTEGER NOT NULL,
 agent TEXT NOT NULL, path TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id),
 PRIMARY KEY(source_id,generation,path,session_id)
);
CREATE INDEX session_path_lookup ON session_paths(agent,path,session_id);
CREATE TABLE session_entries (
 source_id INTEGER NOT NULL REFERENCES sources(id), generation INTEGER NOT NULL,
 locator TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id),
 entry_id TEXT NOT NULL, inherits_parent INTEGER NOT NULL, occurred_at INTEGER NOT NULL,
 PRIMARY KEY(source_id,generation,locator)
);
CREATE INDEX session_entry_lookup ON session_entries(session_id,entry_id);
CREATE INDEX session_entry_native ON session_entries(entry_id);
CREATE INDEX session_entry_time ON session_entries(occurred_at);
CREATE TABLE skills (
 id INTEGER PRIMARY KEY, identity TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
 installed INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE skill_identities (identity TEXT PRIMARY KEY, skill_id INTEGER NOT NULL REFERENCES skills(id));
CREATE TABLE skill_aliases (
 skill_id INTEGER NOT NULL REFERENCES skills(id), reference TEXT NOT NULL,
 kind TEXT NOT NULL, agent TEXT NOT NULL, project TEXT NOT NULL,
 active INTEGER NOT NULL DEFAULT 1,
 PRIMARY KEY(skill_id,reference,kind,agent,project)
);
CREATE INDEX alias_lookup ON skill_aliases(reference,kind,agent,project,active);
CREATE TABLE activation_events (
 id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id),
 worker_id TEXT NOT NULL, native_id TEXT NOT NULL, occurred_at INTEGER NOT NULL,
 evidence TEXT NOT NULL, reference TEXT NOT NULL, cwd TEXT, skill_id INTEGER REFERENCES skills(id),
 outcome TEXT NOT NULL, deduplication_key TEXT NOT NULL UNIQUE, inherited INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX event_skill_time ON activation_events(skill_id,occurred_at);
CREATE INDEX event_time_skill ON activation_events(occurred_at,skill_id);
CREATE INDEX event_session_skill ON activation_events(session_id,skill_id);
CREATE INDEX event_call ON activation_events(session_id,worker_id,native_id);
CREATE TABLE event_observations (
 event_id INTEGER NOT NULL REFERENCES activation_events(id) ON DELETE CASCADE,
 source_id INTEGER NOT NULL REFERENCES sources(id), generation INTEGER NOT NULL,
 locator TEXT NOT NULL, outcome TEXT NOT NULL, inherited INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(source_id,generation,locator,event_id)
);
CREATE INDEX observation_event ON event_observations(event_id);
CREATE TABLE tool_results (
 source_id INTEGER NOT NULL REFERENCES sources(id), generation INTEGER NOT NULL,
 locator TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id),
 worker_id TEXT NOT NULL, native_id TEXT NOT NULL, outcome TEXT NOT NULL, occurred_at INTEGER, inherited INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(source_id,generation,locator,session_id,worker_id,native_id)
);
CREATE INDEX result_call ON tool_results(session_id,worker_id,native_id);
";
