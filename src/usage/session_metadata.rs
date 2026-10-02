//! Source generations preserve ancestry evidence while allowing corrected headers.

use super::records;
use crate::usage::{Session, UsageResult, WorkerRole};
use rusqlite::{params, Connection};

pub(super) fn write(
    connection: &Connection,
    source_id: i64,
    generation: i64,
    session: &Session,
) -> UsageResult<()> {
    let session_id = records::write_session(connection, session)?;
    if session.role == WorkerRole::Unknown && session.parent_native_id.is_none() {
        return Ok(());
    }
    connection.execute("INSERT INTO session_metadata(source_id,generation,session_id,parent_native_id,role) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(source_id,generation,session_id) DO UPDATE SET parent_native_id=excluded.parent_native_id,role=excluded.role",params![source_id,generation,session_id,session.parent_native_id,session.role.key()]).map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn reconcile(connection: &Connection) -> UsageResult<()> {
    connection.execute("UPDATE sessions SET parent_native_id=(SELECT parent_native_id FROM session_metadata metadata WHERE metadata.session_id=sessions.id AND parent_native_id IS NOT NULL ORDER BY generation DESC,source_id LIMIT 1),role=CASE WHEN EXISTS(SELECT 1 FROM session_metadata metadata WHERE metadata.session_id=sessions.id AND role='subagent') THEN 'subagent' WHEN EXISTS(SELECT 1 FROM session_metadata metadata WHERE metadata.session_id=sessions.id AND role='main') THEN 'main' ELSE role END WHERE EXISTS(SELECT 1 FROM session_metadata metadata WHERE metadata.session_id=sessions.id)",[]).map_err(|error| error.to_string())?;
    Ok(())
}
