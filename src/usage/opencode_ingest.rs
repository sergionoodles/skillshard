//! Read-only v2 OpenCode projection imports with stable update cursors.
use super::adapters::opencode;
use super::ingest::Progress;
use super::storage::Database;
use super::{Observation, Record, Source, UsageResult};
use serde::{Deserialize, Serialize};
use std::fs::{self, Metadata};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::UNIX_EPOCH;

#[path = "opencode_source.rs"]
mod source;

pub(super) use super::ingest::{MAX_BATCH_BYTES, MAX_RECORD_BYTES};
const OVERLAP_MILLIS: i64 = 60_000;
const PARSER_VERSION: u32 = 2;
const AUDIT_BATCH_SIZE: usize = 256;
const MAX_DIAGNOSTICS: u64 = 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Checkpoint {
    pub cursor_time: i64,
    pub cursor_id: String,
    pub upper_time: i64,
    pub lower_updated: i64,
    active: bool,
    signature: String,
    #[serde(default)]
    auditing: bool,
    #[serde(default)]
    audit_cursor: String,
    #[serde(default)]
    count_after: i64,
}

pub fn import_batch(
    database: &mut Database,
    path: &Path,
    cutoff: i64,
    cancelled: Arc<AtomicBool>,
) -> UsageResult<Progress> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(Progress::default());
    }
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let signature = signature(path, &metadata)?;
    let previous = database
        .source("opencode", &path.to_string_lossy())?
        .unwrap_or_default();
    let checkpoint: Checkpoint = if previous.parser_state.is_empty() {
        Checkpoint::default()
    } else {
        serde_json::from_str(&previous.parser_state)
            .map_err(|error| format!("Invalid OpenCode checkpoint: {error}"))?
    };
    let identity = super::ingest::file_identity(&metadata);
    let replay = previous.adapter.is_empty()
        || previous.identity != identity
        || metadata.len() < previous.size
        || previous.parser_version != PARSER_VERSION
        || cutoff < previous.cutoff
        || (!checkpoint.active
            && matches!(
                previous.status.as_str(),
                "unsupported_format" | "partial_attribution" | "malformed"
            ));
    if !checkpoint.active
        && previous.modified == signature
        && previous.parser_version == PARSER_VERSION
        && previous.identity == identity
        && cutoff >= previous.cutoff
    {
        return Ok(Progress {
            complete: true,
            ..Progress::default()
        });
    }
    let reader = match source::Reader::open(path, cancelled.clone()) {
        Ok(reader) => reader,
        Err(error) if error.starts_with("Unsupported OpenCode") => {
            let mut unsupported = previous;
            unsupported.adapter = "opencode".into();
            unsupported.path = path.to_string_lossy().into_owned();
            unsupported.modified = signature;
            unsupported.identity = identity;
            unsupported.size = metadata.len();
            unsupported.parser_version = PARSER_VERSION;
            unsupported.cutoff = cutoff;
            unsupported.status = "unsupported_format".into();
            unsupported.diagnostics = 1;
            unsupported.parser_state =
                serde_json::to_string(&Checkpoint::default()).map_err(|error| error.to_string())?;
            database.commit_batch(&unsupported, &[], cutoff, false)?;
            return Ok(Progress {
                committed: true,
                complete: true,
                diagnostics: 1,
                ..Progress::default()
            });
        }
        Err(error) => return Err(error),
    };
    let mut source = if replay {
        Source {
            adapter: "opencode".into(),
            path: path.to_string_lossy().into_owned(),
            generation: previous.generation + 1,
            identity,
            parser_version: PARSER_VERSION,
            cutoff,
            status: "supported".into(),
            ..Source::default()
        }
    } else {
        previous
    };
    let mut checkpoint = if replay || !checkpoint.active {
        let previous_time = i64::try_from(source.offset).map_err(|error| error.to_string())?;
        Checkpoint {
            cursor_time: -1,
            lower_updated: previous_time
                .saturating_sub(OVERLAP_MILLIS)
                .max(cutoff)
                .saturating_sub(1),
            upper_time: reader.watermark()?,
            count_after: previous_time,
            active: true,
            signature,
            ..Checkpoint::default()
        }
    } else {
        checkpoint
    };
    read_batch(
        database,
        &reader,
        &mut source,
        &mut checkpoint,
        metadata.len(),
        cutoff,
        &cancelled,
    )
}

fn read_batch(
    database: &mut Database,
    reader: &source::Reader,
    source: &mut Source,
    checkpoint: &mut Checkpoint,
    size: u64,
    cutoff: i64,
    cancelled: &AtomicBool,
) -> UsageResult<Progress> {
    if checkpoint.auditing {
        return audit_batch(
            database, reader, source, checkpoint, size, cutoff, cancelled,
        );
    }
    let rows = reader.messages(checkpoint)?;
    let mut progress = Progress::default();
    let mut observations = Vec::new();
    let mut locators = Vec::new();
    let mut bytes = 0;
    for row in rows {
        if cancelled.load(Ordering::Acquire) {
            return Ok(Progress::default());
        }
        let row_bytes = row.data.as_ref().map_or(0, String::len);
        if bytes + row_bytes > MAX_BATCH_BYTES {
            break;
        }
        bytes += row_bytes;
        locators.push(row.id.clone());
        let parsed = parse_row(reader, &row);
        match parsed {
            Ok((records, partial)) => {
                if partial && row.updated > checkpoint.count_after {
                    source.status = "partial_attribution".into();
                    source.diagnostics = source.diagnostics.saturating_add(1).min(MAX_DIAGNOSTICS);
                    progress.diagnostics += 1;
                }
                if row.updated > checkpoint.count_after {
                    source.unknown_time = source.unknown_time.saturating_add(records.iter().filter(|record|matches!(record,Record::Activation(event) if event.occurred_at.is_none())).count() as u64).min(MAX_DIAGNOSTICS);
                }
                observations.extend(records.into_iter().map(|record| Observation {
                    locator: row.id.clone(),
                    record,
                }));
            }
            Err(error) => {
                source.status =
                    if error.starts_with("Unsupported") || source.status == "unsupported_format" {
                        "unsupported_format"
                    } else {
                        "malformed"
                    }
                    .into();
                if row.updated > checkpoint.count_after {
                    source.diagnostics = source.diagnostics.saturating_add(1).min(MAX_DIAGNOSTICS);
                    progress.diagnostics += 1;
                }
            }
        }
        checkpoint.cursor_time = row.created;
        checkpoint.cursor_id = row.id;
        progress.records += 1;
    }
    if cancelled.load(Ordering::Acquire) {
        return Ok(Progress::default());
    }
    checkpoint.auditing = !reader.has_more(checkpoint)?;
    source.offset =
        u64::try_from(checkpoint.upper_time.max(0)).map_err(|error| error.to_string())?;
    persist_batch(
        database,
        source,
        checkpoint,
        size,
        cutoff,
        &observations,
        &locators,
        false,
    )?;
    progress.committed = true;
    Ok(progress)
}

fn audit_batch(
    database: &mut Database,
    reader: &source::Reader,
    source: &mut Source,
    checkpoint: &mut Checkpoint,
    size: u64,
    cutoff: i64,
    cancelled: &AtomicBool,
) -> UsageResult<Progress> {
    let locators =
        database.observed_locators(source, &checkpoint.audit_cursor, AUDIT_BATCH_SIZE)?;
    let mut removed = Vec::new();
    for locator in &locators {
        if cancelled.load(Ordering::Acquire) {
            return Ok(Progress::default());
        }
        if !reader.contains(locator)? {
            removed.push(locator.clone());
        }
    }
    if let Some(last) = locators.last() {
        checkpoint.audit_cursor = last.clone();
    }
    let complete = locators.len() < AUDIT_BATCH_SIZE;
    if complete {
        checkpoint.active = false;
        source.modified = checkpoint.signature.clone();
    }
    persist_batch(
        database,
        source,
        checkpoint,
        size,
        cutoff,
        &[],
        &removed,
        complete,
    )?;
    Ok(Progress {
        committed: true,
        complete,
        ..Progress::default()
    })
}

#[allow(clippy::too_many_arguments)]
fn persist_batch(
    database: &mut Database,
    source: &mut Source,
    checkpoint: &Checkpoint,
    size: u64,
    cutoff: i64,
    observations: &[Observation],
    locators: &[String],
    complete: bool,
) -> UsageResult<()> {
    source.size = size;
    source.parser_state = serde_json::to_string(checkpoint).map_err(|error| error.to_string())?;
    database.commit_reconciled_batch(source, observations, locators, cutoff, complete)
}

fn parse_row(
    reader: &source::Reader,
    row: &source::MessageRow,
) -> UsageResult<(Vec<Record>, bool)> {
    let context = reader.context(row)?;
    if context.inherited {
        return Ok((context.records, context.partial));
    }
    let data = row
        .data
        .as_deref()
        .ok_or_else(|| "OpenCode record exceeds size limit".to_string())?;
    let data = serde_json::from_str(data).map_err(|error| error.to_string())?;
    let mut records = context.records;
    records.extend(opencode::parse_message(
        &context.session,
        &row.id,
        &row.kind,
        &data,
        Some(row.created),
        context.directory.as_deref(),
    )?);
    Ok((records, context.partial))
}

fn signature(path: &Path, metadata: &Metadata) -> UsageResult<String> {
    let database = format!("{}:{}", metadata.len(), modified(metadata));
    let wal = Path::new(&format!("{}-wal", path.to_string_lossy())).to_path_buf();
    let wal = match fs::metadata(wal) {
        Ok(metadata) => format!("{}:{}", metadata.len(), modified(&metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "none".into(),
        Err(error) => return Err(error.to_string()),
    };
    Ok(format!("{database}:{wal}"))
}

fn modified(metadata: &Metadata) -> String {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "opencode_ingest_tests.rs"]
mod tests;
