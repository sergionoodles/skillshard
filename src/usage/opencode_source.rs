use super::{Checkpoint, MAX_BATCH_BYTES, MAX_RECORD_BYTES};
use crate::usage::{Record, Session, UsageResult, WorkerRole};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

const MAX_ANCESTORS: usize = 64;
const MAX_METADATA_BYTES: usize = 4096;
const MAX_BATCH_RECORDS: i64 = 256;

pub(super) struct Reader {
    connection: Connection,
}

pub(super) struct MessageRow {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub sequence: i64,
    pub created: i64,
    pub updated: i64,
    pub data: Option<String>,
}

struct SessionRow {
    session: Session,
    directory: Option<PathBuf>,
    fork_session: Option<String>,
    fork_boundary: Option<String>,
}

pub(super) struct Context {
    pub session: Session,
    pub directory: Option<PathBuf>,
    pub records: Vec<Record>,
    pub inherited: bool,
    pub partial: bool,
}

impl Reader {
    pub fn open(path: &Path, cancelled: Arc<AtomicBool>) -> UsageResult<Self> {
        if cancelled.load(Ordering::Acquire) {
            return Err("OpenCode import cancelled".into());
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|error| error.to_string())?;
        connection
            .progress_handler(1000, Some(move || cancelled.load(Ordering::Acquire)))
            .map_err(|error| error.to_string())?;
        connection
            .execute_batch("BEGIN DEFERRED")
            .map_err(|error| error.to_string())?;
        validate_schema(&connection)?;
        Ok(Self { connection })
    }

    pub fn watermark(&self) -> UsageResult<i64> {
        self.connection
            .query_row(
                "SELECT COALESCE(MAX(time_updated),0) FROM session_message",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    pub fn messages(&self, checkpoint: &Checkpoint) -> UsageResult<Vec<MessageRow>> {
        // Select only indexed metadata before loading JSON, so equal-time sorting
        // cannot hold unbounded transcript content in SQLite's temporary sorter.
        let mut statement = self.connection.prepare(
            "SELECT id,session_id,type,seq,time_created,time_updated FROM session_message WHERE time_created>=?1 AND (time_created>?1 OR id>?2) AND time_updated>?3 AND time_updated<=?4 ORDER BY time_created,id LIMIT ?5",
        ).map_err(|error|error.to_string())?;
        let rows = statement
            .query_map(
                params![
                    checkpoint.cursor_time,
                    checkpoint.cursor_id,
                    checkpoint.lower_updated,
                    checkpoint.upper_time,
                    MAX_BATCH_RECORDS
                ],
                |row| {
                    Ok(MessageRow {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        kind: row.get(2)?,
                        sequence: row.get(3)?,
                        created: row.get(4)?,
                        updated: row.get(5)?,
                        data: None,
                    })
                },
            )
            .map_err(|error| error.to_string())?;
        let mut messages = Vec::new();
        let mut bytes = 0;
        for row in rows {
            let mut row = row.map_err(|error| error.to_string())?;
            row.data = self.connection.query_row(
                "SELECT CASE WHEN length(CAST(data AS BLOB))<=?2 THEN data ELSE NULL END FROM session_message WHERE id=?1",
                params![row.id,MAX_RECORD_BYTES as i64],|row|row.get(0),
            ).map_err(|error|error.to_string())?;
            let size = row.data.as_ref().map_or(0, String::len);
            if bytes + size > MAX_BATCH_BYTES {
                break;
            }
            bytes += size;
            messages.push(row);
        }
        Ok(messages)
    }

    pub fn has_more(&self, checkpoint: &Checkpoint) -> UsageResult<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_message WHERE time_created>=?1 AND (time_created>?1 OR id>?2) AND time_updated>?3 AND time_updated<=?4)",
            params![checkpoint.cursor_time,checkpoint.cursor_id,checkpoint.lower_updated,checkpoint.upper_time],
            |row| row.get(0),
        ).map_err(|error| error.to_string())
    }

    pub fn contains(&self, id: &str) -> UsageResult<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_message WHERE id=?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    pub fn context(&self, message: &MessageRow) -> UsageResult<Context> {
        let first = self
            .session(&message.session_id)?
            .ok_or_else(|| "OpenCode message references a missing session".to_string())?;
        let mut context = Context {
            session: first.session.clone(),
            directory: first.directory.clone(),
            records: Vec::new(),
            inherited: false,
            partial: false,
        };
        if let Some(fork) = &first.fork_session {
            let boundary = first
                .fork_boundary
                .as_deref()
                .map(serde_json::from_str::<Value>)
                .transpose()
                .map_err(|error| error.to_string())?;
            let boundary_id = boundary
                .as_ref()
                .and_then(|boundary| boundary.get("messageID"))
                .and_then(Value::as_str);
            let sequence = boundary_id
                .map(|id| {
                    self.connection
                        .query_row(
                            "SELECT seq FROM session_message WHERE session_id=?1 AND id=?2",
                            params![fork, id],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                })
                .transpose()
                .map_err(|error| error.to_string())?
                .flatten();
            let Some(sequence) = sequence else {
                // V2 copies have an added _<seq> suffix. Generated native IDs
                // prove a new call even after the original boundary is deleted.
                context.inherited = !is_generated_message_id(&message.id);
                context.partial = true;
                context.records.push(Record::Session(first.session));
                if !context.inherited {
                    self.apply_historical_location(message, &mut context)?;
                }
                return Ok(context);
            };
            let before = boundary
                .as_ref()
                .and_then(|boundary| boundary.get("type"))
                .and_then(Value::as_str)
                == Some("before");
            context.inherited = if before {
                message.sequence < sequence
            } else {
                message.sequence <= sequence
            };
        }
        let mut next = Some(first);
        let mut seen = HashSet::new();
        while let Some(row) = next {
            if !seen.insert(row.session.native_id.clone()) || seen.len() > MAX_ANCESTORS {
                context.partial = true;
                break;
            }
            next = row
                .session
                .parent_native_id
                .as_deref()
                .map(|id| self.session(id))
                .transpose()?
                .flatten();
            if row.session.parent_native_id.is_some() && next.is_none() {
                context.partial = true;
            }
            context.records.push(Record::Session(row.session));
        }
        self.apply_historical_location(message, &mut context)?;
        Ok(context)
    }

    fn session(&self, id: &str) -> UsageResult<Option<SessionRow>> {
        self.connection.query_row("SELECT id,parent_id,directory,agent,CASE WHEN length(model)<=?2 THEN model ELSE NULL END,fork_session_id,CASE WHEN length(fork_boundary)<=?2 THEN fork_boundary ELSE NULL END FROM session_v2 WHERE id=?1",params![id,MAX_METADATA_BYTES as i64],|row| {
            let directory:String=row.get(2)?;
            let model:Option<String>=row.get(4)?;
            let parent:Option<String>=row.get(1)?;
            let project=(!directory.is_empty()).then(||PathBuf::from(&directory));
            let working_directory=project.clone();
            Ok(SessionRow {session:Session {agent:"opencode".into(),native_id:row.get(0)?,parent_native_id:parent.clone(),role:if parent.is_some(){WorkerRole::Subagent}else{WorkerRole::Main},worker_name:row.get(3)?,model:model.map(|model|serde_json::from_str::<Value>(&model)).transpose().map_err(|error|rusqlite::Error::FromSqlConversionFailure(4,rusqlite::types::Type::Text,Box::new(error)))?.and_then(|model|model.get("id").and_then(Value::as_str).map(str::to_string)),project},directory:working_directory,fork_session:row.get(5)?,fork_boundary:row.get(6)?})
        }).optional().map_err(|error|error.to_string())
    }

    fn apply_historical_location(
        &self,
        message: &MessageRow,
        context: &mut Context,
    ) -> UsageResult<()> {
        let prior: Option<String> = self.connection.query_row(
            "SELECT data FROM session_message WHERE session_id=?1 AND type='location-switched' AND seq<=?2 AND length(CAST(data AS BLOB))<=?3 ORDER BY seq DESC LIMIT 1",
            params![message.session_id,message.sequence,MAX_METADATA_BYTES as i64],|row|row.get(0),
        ).optional().map_err(|error|error.to_string())?;
        let later = if prior.is_none() {
            self.connection.query_row(
                "SELECT data FROM session_message WHERE session_id=?1 AND type='location-switched' AND seq>?2 AND length(CAST(data AS BLOB))<=?3 ORDER BY seq LIMIT 1",
                params![message.session_id,message.sequence,MAX_METADATA_BYTES as i64],|row|row.get::<_,String>(0),
            ).optional().map_err(|error|error.to_string())?
        } else {
            None
        };
        let location = match (prior, later) {
            (Some(value), _) => {
                serde_json::from_str::<Value>(&value).map_err(|error| error.to_string())?
            }
            (None, Some(value)) => serde_json::from_str::<Value>(&value)
                .map_err(|error| error.to_string())?
                .get("previous")
                .cloned()
                .unwrap_or(Value::Null),
            (None, None) => return Ok(()),
        };
        let Some(directory) = location
            .get("location")
            .and_then(|location| location.get("directory"))
            .and_then(Value::as_str)
        else {
            context.directory = None;
            context.partial = true;
            return Ok(());
        };
        let project = PathBuf::from(directory);
        context.session.project = Some(project.clone());
        context.directory = Some(project);
        Ok(())
    }
}

fn is_generated_message_id(id: &str) -> bool {
    const GENERATED_IDENTIFIER_BYTES: usize = 26;
    const TIMESTAMP_BYTES: usize = 12;
    let Some(identifier) = id.strip_prefix("msg_") else {
        return false;
    };
    let bytes = identifier.as_bytes();
    bytes.len() == GENERATED_IDENTIFIER_BYTES
        && bytes[..TIMESTAMP_BYTES]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        && bytes.iter().all(u8::is_ascii_alphanumeric)
}

fn validate_schema(connection: &Connection) -> UsageResult<()> {
    for (table, required) in [
        (
            "session_v2",
            &[
                "id",
                "parent_id",
                "directory",
                "path",
                "agent",
                "model",
                "fork_session_id",
                "fork_boundary",
            ][..],
        ),
        (
            "session_message",
            &[
                "id",
                "session_id",
                "type",
                "seq",
                "time_created",
                "time_updated",
                "data",
            ][..],
        ),
    ] {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| error.to_string())?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| error.to_string())?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|error| error.to_string())?;
        if required.iter().any(|column| !columns.contains(*column)) {
            return Err(
                "Unsupported OpenCode SQLite schema (expected v2 session projections)".into(),
            );
        }
    }
    Ok(())
}
