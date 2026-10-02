use super::resolve;
use crate::usage::{Activation, Observation, Record, Session, UsageResult};
use rusqlite::{params, Connection};

pub(super) fn write(
    connection: &Connection,
    source_id: i64,
    generation: i64,
    observation: &Observation,
    cutoff: i64,
) -> UsageResult<()> {
    match &observation.record {
        Record::Session(session) => {
            super::session_metadata::write(connection, source_id, generation, session)?;
        }
        Record::SessionPath { session, path } => {
            let session_id = write_session(connection, session)?;
            let path = resolve::normalize_path(path).to_string_lossy().into_owned();
            connection.execute(
                "INSERT OR IGNORE INTO session_paths(source_id,generation,agent,path,session_id) VALUES(?1,?2,?3,?4,?5)",
                params![source_id,generation,session.agent,path,session_id],
            ).map_err(|error| error.to_string())?;
        }
        Record::SessionEntry {
            session,
            entry_id,
            inherits_parent,
            occurred_at,
        } => {
            let session_id = write_session(connection, session)?;
            if let Some(timestamp) = occurred_at.filter(|timestamp| *timestamp >= cutoff) {
                connection.execute(
                    "INSERT INTO session_entries(source_id,generation,locator,session_id,entry_id,inherits_parent,occurred_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(source_id,generation,locator) DO UPDATE SET entry_id=excluded.entry_id,inherits_parent=excluded.inherits_parent,occurred_at=excluded.occurred_at",
                    params![source_id,generation,observation.locator,session_id,entry_id,inherits_parent,timestamp],
                ).map_err(|error| error.to_string())?;
                connection
                    .execute(
                        "INSERT OR IGNORE INTO touched_entries(entry_id) VALUES(?1)",
                        [entry_id],
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        Record::Activation(activation) => {
            write_session(connection, &activation.session)?;
            if activation
                .occurred_at
                .is_some_and(|timestamp| timestamp >= cutoff)
            {
                write_activation(connection, source_id, generation, observation, activation)?;
            }
        }
        Record::ToolResult {
            agent,
            session_id,
            worker_id,
            call_id,
            outcome,
            occurred_at,
        } => {
            let session = Session {
                agent: agent.clone(),
                native_id: session_id.clone(),
                ..Session::default()
            };
            let session_id = write_session(connection, &session)?;
            connection.execute("INSERT INTO tool_results(source_id,generation,locator,session_id,worker_id,native_id,outcome,occurred_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(source_id,generation,locator,session_id,worker_id,native_id) DO UPDATE SET outcome=excluded.outcome,occurred_at=excluded.occurred_at", params![source_id,generation,observation.locator,session_id,worker_id.as_deref().unwrap_or(""),call_id,outcome.key(),occurred_at]).map_err(|error| error.to_string())?;
            connection.execute("INSERT OR IGNORE INTO touched_events SELECT id FROM activation_events WHERE session_id=?1 AND worker_id=?2 AND native_id=?3", params![session_id,worker_id.as_deref().unwrap_or(""),call_id]).map_err(|error| error.to_string())?;
        }
        Record::Link {
            agent,
            parent_id,
            child_id,
        } => {
            write_session(
                connection,
                &Session {
                    agent: agent.clone(),
                    native_id: parent_id.clone(),
                    ..Session::default()
                },
            )?;
            super::session_metadata::write(
                connection,
                source_id,
                generation,
                &Session {
                    agent: agent.clone(),
                    native_id: child_id.clone(),
                    parent_native_id: Some(parent_id.clone()),
                    role: crate::usage::WorkerRole::Subagent,
                    ..Session::default()
                },
            )?;
        }
    }
    Ok(())
}

pub(super) fn write_session(connection: &Connection, session: &Session) -> UsageResult<i64> {
    let project = session
        .project
        .as_ref()
        .map(|path| resolve::normalize_path(path).to_string_lossy().into_owned());
    if let Some(project) = &project {
        connection
            .execute("INSERT OR IGNORE INTO projects(path) VALUES(?1)", [project])
            .map_err(|error| error.to_string())?;
    }
    connection.execute(
        "INSERT INTO sessions(agent,native_id,parent_native_id,role,worker_name,model,project_id) VALUES(?1,?2,?3,?4,?5,?6,(SELECT id FROM projects WHERE path=?7)) ON CONFLICT(agent,native_id) DO UPDATE SET parent_native_id=COALESCE(excluded.parent_native_id,sessions.parent_native_id),role=CASE WHEN excluded.role='unknown' THEN sessions.role ELSE excluded.role END,worker_name=COALESCE(excluded.worker_name,sessions.worker_name),model=COALESCE(excluded.model,sessions.model),project_id=COALESCE(excluded.project_id,sessions.project_id)",
        params![session.agent,session.native_id,session.parent_native_id,session.role.key(),session.worker_name,session.model,project],
    ).map_err(|error| error.to_string())?;
    let id = connection
        .query_row(
            "SELECT id FROM sessions WHERE agent=?1 AND native_id=?2",
            params![session.agent, session.native_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT OR IGNORE INTO touched_sessions(id) VALUES(?1)",
            [id],
        )
        .map_err(|error| error.to_string())?;
    Ok(id)
}

fn write_activation(
    connection: &Connection,
    source_id: i64,
    generation: i64,
    observation: &Observation,
    activation: &Activation,
) -> UsageResult<()> {
    let session_id = write_session(connection, &activation.session)?;
    let key = serde_json::to_string(&(
        &activation.session.agent,
        &activation.session.native_id,
        &activation.worker_id,
        &activation.native_id,
        &activation.reference,
    ))
    .map_err(|error| error.to_string())?;
    let project: Option<String> = connection
        .query_row(
            "SELECT p.path FROM sessions s LEFT JOIN projects p ON p.id=s.project_id WHERE s.id=?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let skill_id = resolve::resolve_reference(
        connection,
        &activation.reference,
        &activation.session.agent,
        project.as_deref().map(std::path::Path::new),
        activation.working_directory.as_deref(),
    )?;
    connection.execute("INSERT INTO activation_events(session_id,worker_id,native_id,occurred_at,evidence,reference,skill_id,outcome,deduplication_key,cwd) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(deduplication_key) DO UPDATE SET skill_id=COALESCE(excluded.skill_id,activation_events.skill_id)", params![session_id,activation.worker_id.as_deref().unwrap_or(""),activation.native_id,activation.occurred_at,format!("{:?}",activation.evidence),activation.reference,skill_id,activation.outcome.key(),key,activation.working_directory.as_ref().map(|path| resolve::normalize_path(path).to_string_lossy().into_owned())]).map_err(|error| error.to_string())?;
    connection.execute("INSERT INTO event_observations(event_id,source_id,generation,locator,outcome) VALUES((SELECT id FROM activation_events WHERE deduplication_key=?1),?2,?3,?4,?5) ON CONFLICT(source_id,generation,locator,event_id) DO UPDATE SET outcome=excluded.outcome", params![key,source_id,generation,observation.locator,activation.outcome.key()]).map_err(|error| error.to_string())?;
    connection.execute("INSERT OR IGNORE INTO touched_events SELECT id FROM activation_events WHERE deduplication_key=?1", [key]).map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn reconcile_outcomes(connection: &Connection, full: bool) -> UsageResult<()> {
    let selection = if full {
        String::new()
    } else {
        " WHERE id IN (SELECT id FROM touched_events)".into()
    };
    let sql = format!("UPDATE activation_events AS e SET outcome=CASE WHEN EXISTS(SELECT 1 FROM tool_results r WHERE r.session_id=e.session_id AND r.worker_id=e.worker_id AND r.native_id=e.native_id AND r.outcome='failed' AND r.inherited=0) THEN 'failed' WHEN EXISTS(SELECT 1 FROM tool_results r WHERE r.session_id=e.session_id AND r.worker_id=e.worker_id AND r.native_id=e.native_id AND r.outcome='succeeded' AND r.inherited=0) THEN 'succeeded' WHEN EXISTS(SELECT 1 FROM event_observations o WHERE o.event_id=e.id AND o.outcome='succeeded' AND o.inherited=0) THEN 'succeeded' WHEN EXISTS(SELECT 1 FROM event_observations o WHERE o.event_id=e.id AND o.outcome='failed' AND o.inherited=0) THEN 'failed' ELSE 'unknown' END{selection}");
    connection
        .execute(&sql, [])
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn prune_results(connection: &Connection, cutoff: i64) -> UsageResult<()> {
    connection.execute("DELETE FROM tool_results WHERE occurred_at<?1 AND NOT EXISTS (SELECT 1 FROM activation_events e WHERE e.session_id=tool_results.session_id AND e.worker_id=tool_results.worker_id AND e.native_id=tool_results.native_id)", [cutoff]).map_err(|error| error.to_string())?;
    const ORPHAN_RESULT_LIMIT: i64 = 1024;
    connection.execute("DELETE FROM tool_results WHERE rowid IN (SELECT r.rowid FROM tool_results r WHERE r.occurred_at IS NULL AND NOT EXISTS (SELECT 1 FROM activation_events e WHERE e.session_id=r.session_id AND e.worker_id=r.worker_id AND e.native_id=r.native_id) ORDER BY r.rowid DESC LIMIT -1 OFFSET ?1)", [ORPHAN_RESULT_LIMIT]).map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn replace_locators(
    connection: &Connection,
    source_id: i64,
    generation: i64,
    locators: &[String],
) -> UsageResult<()> {
    for locator in locators {
        connection.execute("INSERT OR IGNORE INTO touched_events SELECT event_id FROM event_observations WHERE source_id=?1 AND generation=?2 AND locator=?3",params![source_id,generation,locator]).map_err(|error| error.to_string())?;
        connection.execute("INSERT OR IGNORE INTO touched_events SELECT e.id FROM activation_events e JOIN tool_results r ON r.session_id=e.session_id AND r.worker_id=e.worker_id AND r.native_id=e.native_id WHERE r.source_id=?1 AND r.generation=?2 AND r.locator=?3",params![source_id,generation,locator]).map_err(|error| error.to_string())?;
        connection.execute("DELETE FROM event_observations WHERE source_id=?1 AND generation=?2 AND locator=?3",params![source_id,generation,locator]).map_err(|error| error.to_string())?;
        connection
            .execute(
                "DELETE FROM tool_results WHERE source_id=?1 AND generation=?2 AND locator=?3",
                params![source_id, generation, locator],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
