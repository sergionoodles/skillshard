//! Transfer attribution evidence between owned generations without copying history.
use super::{check_cancellation, Database};
use crate::usage::UsageResult;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{atomic::AtomicBool, Arc};

#[derive(Debug, PartialEq, Eq)]
pub enum IdentityTransfer {
    Copied,
    Unavailable(String),
}

enum TransferError {
    Source(String),
    Destination(String),
}

impl TransferError {
    fn source(error: rusqlite::Error) -> Self {
        Self::Source(error.to_string())
    }
    fn destination(error: rusqlite::Error) -> Self {
        Self::Destination(error.to_string())
    }
}

/// Call before registering current installs and importing transcripts. Repeated
/// copies into a resumed generation are idempotent; no event/cursor is reused.
pub(super) fn copy_metadata(
    destination: &mut Database,
    path: &Path,
    cancellation: Arc<AtomicBool>,
) -> UsageResult<IdentityTransfer> {
    check_cancellation(Some(&cancellation))?;
    let previous = match Database::open_read_only_with_cancellation(path, cancellation.clone()) {
        Ok(previous) => previous,
        Err(error) => {
            check_cancellation(Some(&cancellation))?;
            return Ok(IdentityTransfer::Unavailable(error));
        }
    };
    destination.set_cancellation(cancellation.clone())?;
    let source = match previous.connection.unchecked_transaction() {
        Ok(source) => source,
        Err(error) => {
            check_cancellation(Some(&cancellation))?;
            return Ok(IdentityTransfer::Unavailable(error.to_string()));
        }
    };
    let transaction = destination
        .connection
        .transaction()
        .map_err(|error| error.to_string())?;
    match copy_tables(&source, &transaction) {
        Ok(()) => {
            check_cancellation(Some(&cancellation))?;
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(IdentityTransfer::Copied)
        }
        Err(TransferError::Source(error)) => {
            check_cancellation(Some(&cancellation))?;
            Ok(IdentityTransfer::Unavailable(error))
        }
        Err(TransferError::Destination(error)) => Err(error),
    }
}

fn copy_tables(source: &Connection, destination: &Connection) -> Result<(), TransferError> {
    let skills = copy_skills(source, destination)?;
    copy_identities(source, destination, &skills)?;
    copy_aliases(source, destination, &skills)?;
    copy_history_locations(source, destination)
}

fn copy_history_locations(
    source: &Connection,
    destination: &Connection,
) -> Result<(), TransferError> {
    let mut statement = source
        .prepare("SELECT adapter,path,identity FROM history_locations")
        .map_err(TransferError::source)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(TransferError::source)?;
    for row in rows {
        let (adapter, path, identity) = row.map_err(TransferError::source)?;
        destination.execute("INSERT INTO history_locations(adapter,path,identity) VALUES(?1,?2,?3) ON CONFLICT(adapter,path) DO NOTHING",params![adapter,path,identity]).map_err(TransferError::destination)?;
    }
    Ok(())
}

fn copy_skills(
    source: &Connection,
    destination: &Connection,
) -> Result<HashMap<i64, i64>, TransferError> {
    let mut statement = source
        .prepare("SELECT id,identity,name,installed FROM skills ORDER BY id")
        .map_err(TransferError::source)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })
        .map_err(TransferError::source)?;
    let mut skills = HashMap::new();
    for row in rows {
        let (old_id, identity, name, installed) = row.map_err(TransferError::source)?;
        destination.execute("INSERT INTO skills(identity,name,installed) VALUES(?1,?2,?3) ON CONFLICT(identity) DO NOTHING",params![identity,name,installed]).map_err(TransferError::destination)?;
        let new_id = destination
            .query_row(
                "SELECT id FROM skills WHERE identity=?1",
                [&identity],
                |row| row.get(0),
            )
            .map_err(TransferError::destination)?;
        skills.insert(old_id, new_id);
    }
    Ok(skills)
}

fn mapped_skill(skills: &HashMap<i64, i64>, previous_id: i64) -> Result<i64, TransferError> {
    skills.get(&previous_id).copied().ok_or_else(|| {
        TransferError::Source(
            "Previous skill metadata contains an invalid identity reference".into(),
        )
    })
}

fn copy_identities(
    source: &Connection,
    destination: &Connection,
    skills: &HashMap<i64, i64>,
) -> Result<(), TransferError> {
    let mut statement = source
        .prepare("SELECT identity,skill_id FROM skill_identities")
        .map_err(TransferError::source)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(TransferError::source)?;
    for row in rows {
        let (identity, previous_id) = row.map_err(TransferError::source)?;
        let skill_id = mapped_skill(skills, previous_id)?;
        destination.execute("INSERT INTO skill_identities(identity,skill_id) VALUES(?1,?2) ON CONFLICT(identity) DO UPDATE SET skill_id=excluded.skill_id",params![identity,skill_id]).map_err(TransferError::destination)?;
    }
    Ok(())
}

fn copy_aliases(
    source: &Connection,
    destination: &Connection,
    skills: &HashMap<i64, i64>,
) -> Result<(), TransferError> {
    let mut statement = source
        .prepare("SELECT skill_id,reference,kind,agent,project,active FROM skill_aliases")
        .map_err(TransferError::source)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, bool>(5)?,
            ))
        })
        .map_err(TransferError::source)?;
    for row in rows {
        let (previous_id, reference, kind, agent, project, active) =
            row.map_err(TransferError::source)?;
        let skill_id = mapped_skill(skills, previous_id)?;
        destination.execute("INSERT INTO skill_aliases(skill_id,reference,kind,agent,project,active) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(skill_id,reference,kind,agent,project) DO NOTHING",params![skill_id,reference,kind,agent,project,active]).map_err(TransferError::destination)?;
    }
    Ok(())
}
