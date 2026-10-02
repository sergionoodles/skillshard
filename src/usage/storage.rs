//! Transactional normalized observations and committed source cursors.
use super::{Observation, Query, Snapshot, Source, UsageResult};
use crate::model::Skill;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

#[path = "identity_transfer.rs"]
mod identity_transfer;
#[path = "lineage.rs"]
mod lineage;
#[path = "queries.rs"]
mod queries;
#[path = "records.rs"]
mod records;
pub use identity_transfer::IdentityTransfer;
#[path = "relocations.rs"]
mod relocations;
#[path = "resolve.rs"]
mod resolve;
#[path = "schema.rs"]
mod schema;
#[path = "session_metadata.rs"]
mod session_metadata;

pub fn installed_identities(skill: &Skill) -> UsageResult<Vec<String>> {
    resolve::installed_identities(skill)
}

pub fn installed_group_ids(skill: &Skill, snapshot: &Snapshot) -> UsageResult<Vec<i64>> {
    let base = resolve::skill_identity(skill, &skill.scope)?;
    let mut ids = Vec::new();
    for (identity, id) in &snapshot.skill_ids {
        let value = serde_json::from_str::<serde_json::Value>(identity)
            .map_err(|error| format!("Invalid stored skill identity: {error}"))?;
        let is_copy = value
            .as_array()
            .and_then(|array| array.first())
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value == base);
        if identity == &base || is_copy {
            ids.push(*id);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Database {
    pub(super) connection: Connection,
}

impl Database {
    pub fn set_cancellation(&self, cancellation: Arc<AtomicBool>) -> UsageResult<()> {
        const PROGRESS_STEPS: i32 = 1000;
        self.connection
            .progress_handler(
                PROGRESS_STEPS,
                Some(move || cancellation.load(Ordering::Relaxed)),
            )
            .map_err(|error| error.to_string())
    }

    pub fn open(path: &Path) -> UsageResult<Self> {
        Self::open_inner(path, None)
    }

    pub fn open_with_cancellation(path: &Path, cancellation: Arc<AtomicBool>) -> UsageResult<Self> {
        Self::open_inner(path, Some(cancellation))
    }

    fn open_inner(path: &Path, cancellation: Option<Arc<AtomicBool>>) -> UsageResult<Self> {
        check_cancellation(cancellation.as_deref())?;
        let mut database = Self {
            connection: Connection::open(path).map_err(|error| error.to_string())?,
        };
        if let Some(cancellation) = &cancellation {
            database.set_cancellation(cancellation.clone())?;
        }
        database
            .connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        database
            .connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| error.to_string())?;
        database
            .connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|error| error.to_string())?;
        check_cancellation(cancellation.as_deref())?;
        schema::initialize(&mut database.connection)?;
        database.validate()?;
        Ok(database)
    }

    pub fn open_read_only(path: &Path) -> UsageResult<Self> {
        Self::open_read_only_inner(path, None)
    }

    pub fn open_read_only_with_cancellation(
        path: &Path,
        cancellation: Arc<AtomicBool>,
    ) -> UsageResult<Self> {
        Self::open_read_only_inner(path, Some(cancellation))
    }

    fn open_read_only_inner(
        path: &Path,
        cancellation: Option<Arc<AtomicBool>>,
    ) -> UsageResult<Self> {
        check_cancellation(cancellation.as_deref())?;
        let database = Self {
            connection: Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|error| error.to_string())?,
        };
        if let Some(cancellation) = &cancellation {
            database.set_cancellation(cancellation.clone())?;
        }
        database
            .connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        check_cancellation(cancellation.as_deref())?;
        let version: i64 = database
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if version != 1 {
            return Err(format!("Unsupported usage database schema {version}"));
        }
        Ok(database)
    }

    pub fn copy_identity_metadata_from(
        &mut self,
        previous: &Path,
        cancellation: Arc<AtomicBool>,
    ) -> UsageResult<IdentityTransfer> {
        identity_transfer::copy_metadata(self, previous, cancellation)
    }

    pub fn source_locations(&self) -> UsageResult<Vec<(String, std::path::PathBuf, String)>> {
        let mut statement = self
            .connection
            .prepare("SELECT adapter,path,identity FROM history_locations ORDER BY adapter,path")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    std::path::PathBuf::from(row.get::<_, String>(1)?),
                    row.get(2)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn observed_locators(
        &self,
        source: &Source,
        after: &str,
        limit: usize,
    ) -> UsageResult<Vec<String>> {
        let mut statement = self.connection.prepare(
            "SELECT locator FROM event_observations WHERE source_id=(SELECT id FROM sources WHERE adapter=?1 AND path=?2) AND generation=?3 AND locator>?4 UNION SELECT locator FROM tool_results WHERE source_id=(SELECT id FROM sources WHERE adapter=?1 AND path=?2) AND generation=?3 AND locator>?4 ORDER BY locator LIMIT ?5",
        ).map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(
                rusqlite::params![
                    source.adapter,
                    source.path,
                    source.generation,
                    after,
                    limit as i64
                ],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn record_move(
        &mut self,
        skill: &Skill,
        destination: &crate::model::Scope,
    ) -> UsageResult<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        relocations::record_move(&transaction, skill, destination)?;
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn record_disable(&mut self, skill: &Skill) -> UsageResult<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        relocations::record_toggle(&transaction, skill, true)?;
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn record_enable(&mut self, skill: &Skill) -> UsageResult<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        relocations::record_toggle(&transaction, skill, false)?;
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn validate(&self) -> UsageResult<()> {
        let result: String = self
            .connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if result != "ok" {
            return Err(format!("Usage database integrity check: {result}"));
        }
        let foreign_keys: Option<String> = self
            .connection
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()
            .map_err(|error| error.to_string())?;
        if foreign_keys.is_some() {
            return Err("Usage database contains invalid foreign keys".into());
        }
        Ok(())
    }

    pub fn checkpoint(&self) -> UsageResult<()> {
        let busy: i64 = self
            .connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if busy != 0 {
            return Err("Usage database checkpoint is busy".into());
        }
        Ok(())
    }

    pub fn source(&self, adapter: &str, path: &str) -> UsageResult<Option<Source>> {
        self.connection.query_row(
            "SELECT adapter,path,generation,size,modified,identity,offset,fingerprint,parser_version,cutoff,parser_state,diagnostics,unknown_time,status FROM sources WHERE adapter=?1 AND path=?2",
            params![adapter, path],
            |row| Ok(Source {
                adapter: row.get(0)?, path: row.get(1)?, generation: row.get(2)?, size: queries::unsigned(row,3)?,
                modified: row.get(4)?, identity: row.get(5)?, offset: queries::unsigned(row,6)?, fingerprint: row.get(7)?,
                parser_version: row.get(8)?, cutoff: row.get(9)?, parser_state: row.get(10)?,
                diagnostics: queries::unsigned(row,11)?, unknown_time: queries::unsigned(row,12)?, status: row.get(13)?,
            }),
        ).optional().map_err(|error| error.to_string())
    }

    pub fn commit_batch(
        &mut self,
        source: &Source,
        observations: &[Observation],
        cutoff: i64,
        completed_replay: bool,
    ) -> UsageResult<()> {
        self.commit_reconciled_batch(source, observations, &[], cutoff, completed_replay)
    }

    pub fn commit_reconciled_batch(
        &mut self,
        source: &Source,
        observations: &[Observation],
        replaced_locators: &[String],
        cutoff: i64,
        completed_replay: bool,
    ) -> UsageResult<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction.execute_batch("CREATE TEMP TABLE IF NOT EXISTS touched_events(id INTEGER PRIMARY KEY); DELETE FROM touched_events; CREATE TEMP TABLE IF NOT EXISTS touched_sessions(id INTEGER PRIMARY KEY); DELETE FROM touched_sessions; CREATE TEMP TABLE IF NOT EXISTS touched_entries(entry_id TEXT PRIMARY KEY); DELETE FROM touched_entries;").map_err(|error| error.to_string())?;
        let source_id = write_source(&transaction, source)?;
        records::replace_locators(
            &transaction,
            source_id,
            source.generation,
            replaced_locators,
        )?;
        let has_old_generation: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM event_observations WHERE source_id=?1 AND generation<>?2) OR EXISTS(SELECT 1 FROM tool_results WHERE source_id=?1 AND generation<>?2) OR EXISTS(SELECT 1 FROM session_entries WHERE source_id=?1 AND generation<>?2)",params![source_id,source.generation],|row| row.get(0)).map_err(|error| error.to_string())?;
        for observation in observations {
            records::write(
                &transaction,
                source_id,
                source.generation,
                observation,
                cutoff,
            )?;
        }
        if completed_replay {
            transaction
                .execute(
                    "DELETE FROM session_metadata WHERE source_id=?1 AND generation<>?2",
                    params![source_id, source.generation],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "DELETE FROM session_entries WHERE source_id=?1 AND generation<>?2",
                    params![source_id, source.generation],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "DELETE FROM session_paths WHERE source_id=?1 AND generation<>?2",
                    params![source_id, source.generation],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "DELETE FROM event_observations WHERE source_id=?1 AND generation<>?2",
                    params![source_id, source.generation],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "DELETE FROM tool_results WHERE source_id=?1 AND generation<>?2",
                    params![source_id, source.generation],
                )
                .map_err(|error| error.to_string())?;
            if has_old_generation {
                transaction.execute("DELETE FROM activation_events WHERE NOT EXISTS (SELECT 1 FROM event_observations o WHERE o.event_id=activation_events.id)", []).map_err(|error| error.to_string())?;
            }
        }
        transaction.execute("DELETE FROM activation_events WHERE id IN (SELECT id FROM touched_events) AND NOT EXISTS (SELECT 1 FROM event_observations o WHERE o.event_id=activation_events.id)", []).map_err(|error| error.to_string())?;
        session_metadata::reconcile(&transaction)?;
        let has_changed_parents = resolve::reconcile_sessions(&transaction)?;
        lineage::reconcile_inherited_entries(
            &transaction,
            has_changed_parents || (completed_replay && has_old_generation),
        )?;
        records::reconcile_outcomes(&transaction, completed_replay && has_old_generation)?;
        resolve::reconcile_unresolved(&transaction)?;
        records::prune_results(&transaction, cutoff)?;
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn register_skills(&mut self, skills: &[Skill]) -> UsageResult<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        resolve::register_skills(&transaction, skills)?;
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn snapshot(&self, query: &Query, cutoff: i64, now: i64) -> UsageResult<Snapshot> {
        queries::snapshot(&self.connection, query, cutoff, now)
    }

    pub fn prune(&mut self, cutoff: i64) -> UsageResult<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute("UPDATE sources SET cutoff=MAX(cutoff,?1)", [cutoff])
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "DELETE FROM activation_events WHERE occurred_at<?1",
                [cutoff],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute("DELETE FROM session_entries WHERE occurred_at<?1", [cutoff])
            .map_err(|error| error.to_string())?;
        records::prune_results(&transaction, cutoff)?;
        transaction.commit().map_err(|error| error.to_string())
    }
}

fn check_cancellation(cancellation: Option<&AtomicBool>) -> UsageResult<()> {
    if cancellation.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
        return Err("Usage database open cancelled".into());
    }
    Ok(())
}

fn write_source(connection: &Connection, source: &Source) -> UsageResult<i64> {
    connection.execute("INSERT INTO history_locations(adapter,path,identity) VALUES(?1,?2,?3) ON CONFLICT(adapter,path) DO UPDATE SET identity=excluded.identity", params![source.adapter,source.path,source.identity]).map_err(|error| error.to_string())?;
    connection.execute(
        "INSERT INTO sources(adapter,path,generation,size,modified,identity,offset,fingerprint,parser_version,cutoff,parser_state,diagnostics,unknown_time,status) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(adapter,path) DO UPDATE SET generation=excluded.generation,size=excluded.size,modified=excluded.modified,identity=excluded.identity,offset=excluded.offset,fingerprint=excluded.fingerprint,parser_version=excluded.parser_version,cutoff=excluded.cutoff,parser_state=excluded.parser_state,diagnostics=excluded.diagnostics,unknown_time=excluded.unknown_time,status=excluded.status",
        params![source.adapter,source.path,source.generation,i64::try_from(source.size).map_err(|error| error.to_string())?,source.modified,source.identity,i64::try_from(source.offset).map_err(|error| error.to_string())?,source.fingerprint,source.parser_version,source.cutoff,source.parser_state,i64::try_from(source.diagnostics).map_err(|error| error.to_string())?,i64::try_from(source.unknown_time).map_err(|error| error.to_string())?,source.status],
    ).map_err(|error| error.to_string())?;
    connection
        .query_row(
            "SELECT id FROM sources WHERE adapter=?1 AND path=?2",
            params![source.adapter, source.path],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "storage_additional_tests.rs"]
mod additional_tests;
#[cfg(test)]
#[path = "storage_identity_tests.rs"]
mod identity_tests;

#[cfg(test)]
#[path = "storage_benchmarks.rs"]
mod benchmarks;

#[cfg(test)]
#[path = "storage_relocation_tests.rs"]
mod relocation_tests;

#[cfg(test)]
#[path = "storage_transfer_tests.rs"]
mod transfer_tests;

#[cfg(test)]
#[path = "storage_lineage_tests.rs"]
mod lineage_tests;
