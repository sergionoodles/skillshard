//! Bounded discovery and incremental JSONL reads. No agent-owned file is written.

use super::adapters::{self, ParserState};
use super::storage::Database;
use super::{Observation, Record, Source, UsageResult};
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

#[path = "discovery.rs"]
mod discovery;
pub use discovery::{discover, Discovery, HistoryFile, HistoryRoot};

// Valid tool results and assistant projections routinely exceed 512 KiB.
// Keep records and batches bounded while accepting those native histories.
pub(super) const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_BATCH_BYTES: usize = MAX_RECORD_BYTES;
const MAX_BATCH_RECORDS: usize = 256;
const MAX_CORRUPT_RECORDS: u64 = 100;
const FINGERPRINT_BYTES: u64 = 4096;

#[derive(Debug, Default)]
pub struct Progress {
    pub records: u64,
    pub committed: bool,
    pub complete: bool,
    pub diagnostics: u64,
}

pub fn import_batch(
    database: &mut Database,
    history: &HistoryFile,
    cutoff: i64,
    cancelled: &AtomicBool,
) -> UsageResult<Progress> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(Progress::default());
    }
    let mut file = File::open(&history.path)
        .map_err(|error| format!("{}: {error}", history.path.display()))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    let previous = database.source(&history.adapter, &history.path.to_string_lossy())?;
    let source = prepare_source(&mut file, history, &metadata, previous, cutoff)?;
    if source.offset == metadata.len() && source.modified == modified(&metadata) {
        return Ok(Progress {
            complete: true,
            ..Progress::default()
        });
    }
    file.seek(SeekFrom::Start(source.offset))
        .map_err(|error| error.to_string())?;
    read_batch(database, source, file, cutoff, cancelled)
}

fn prepare_source(
    file: &mut File,
    history: &HistoryFile,
    metadata: &Metadata,
    previous: Option<Source>,
    cutoff: i64,
) -> UsageResult<Source> {
    let previous = previous.unwrap_or_default();
    let identity = file_identity(metadata);
    let checkpoint = fingerprint(file, previous.offset.min(metadata.len()))?;
    let must_replay = previous.adapter.is_empty()
        || previous.identity != identity
        || metadata.len() < previous.offset
        || previous.parser_version != adapters::PARSER_VERSION
        || cutoff < previous.cutoff
        || previous.fingerprint != checkpoint
        || (metadata.len() == previous.size && previous.modified != modified(metadata));
    if !must_replay {
        return Ok(previous);
    }
    Ok(Source {
        adapter: history.adapter.clone(),
        path: history.path.to_string_lossy().into_owned(),
        generation: previous.generation + 1,
        identity,
        parser_version: adapters::PARSER_VERSION,
        cutoff,
        status: "supported".into(),
        ..Source::default()
    })
}

fn read_batch(
    database: &mut Database,
    mut source: Source,
    file: File,
    cutoff: i64,
    cancelled: &AtomicBool,
) -> UsageResult<Progress> {
    let mut state: ParserState = if source.parser_state.is_empty() {
        ParserState::default()
    } else {
        serde_json::from_str(&source.parser_state)
            .map_err(|error| format!("invalid parser checkpoint: {error}"))?
    };
    state.source_path = Some(PathBuf::from(&source.path));
    let mut reader = BufReader::new(file);
    let mut observations = Vec::new();
    let mut progress = Progress::default();
    let start = source.offset;
    while progress.records < MAX_BATCH_RECORDS as u64
        && source.offset - start < MAX_BATCH_BYTES as u64
    {
        if cancelled.load(Ordering::Acquire) {
            return Ok(Progress::default());
        }
        let line = read_record(&mut reader, cancelled).map_err(|error| error.to_string())?;
        let Some((bytes, consumed)) = line else {
            progress.complete = true;
            break;
        };
        let locator = source.offset.to_string();
        source.offset += consumed;
        progress.records += 1;
        match parse_record(&source.adapter, &bytes, &mut state, &locator) {
            Ok(records) => {
                if state.has_partial_attribution() && source.diagnostics == 0 {
                    source.diagnostics = 1;
                    source.status = "partial_attribution".into();
                    progress.diagnostics += 1;
                }
                source.unknown_time += records.iter().filter(|record| matches!(record, Record::Activation(event) if event.occurred_at.is_none())).count() as u64;
                observations.extend(records.into_iter().map(|record| Observation {
                    locator: locator.clone(),
                    record,
                }));
            }
            Err(_) => {
                source.status = "malformed".into();
                source.diagnostics += 1;
                progress.diagnostics += 1;
            }
        }
        if source.diagnostics >= MAX_CORRUPT_RECORDS {
            return Err(format!(
                "{}: excessive malformed or unsupported records; source paused",
                source.path
            ));
        }
    }
    if cancelled.load(Ordering::Acquire) {
        return Ok(Progress::default());
    }
    if progress.complete && !state.has_supported_format() && source.diagnostics == 0 {
        source.diagnostics = 1;
        source.status = "unsupported_format".into();
        progress.diagnostics += 1;
    }
    let mut file = reader.into_inner();
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if file_identity(&metadata) != source.identity || metadata.len() < source.offset {
        return Err(format!(
            "{} changed during import; retrying next scan",
            source.path
        ));
    }
    source.size = metadata.len();
    source.modified = modified(&metadata);
    source.fingerprint = fingerprint(&mut file, source.offset)?;
    source.parser_state = serde_json::to_string(&state).map_err(|error| error.to_string())?;
    database.commit_batch(&source, &observations, cutoff, progress.complete)?;
    progress.committed = true;
    Ok(progress)
}

fn parse_record(
    adapter: &str,
    bytes: &[u8],
    state: &mut ParserState,
    locator: &str,
) -> UsageResult<Vec<Record>> {
    if bytes.is_empty() {
        return Err("record exceeds size limit".into());
    }
    let bytes = if matches!(adapter, "claude" | "claude-code") {
        let prefix = bytes.iter().take_while(|byte| **byte == 0).count();
        &bytes[prefix..]
    } else {
        bytes
    };
    let value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    adapters::parse(adapter, &value, state, locator)
}

/// An incomplete trailing record is discarded without advancing the checkpoint.
fn read_record(
    reader: &mut impl BufRead,
    cancelled: &AtomicBool,
) -> std::io::Result<Option<(Vec<u8>, u64)>> {
    let mut bytes = Vec::new();
    let mut consumed = 0;
    let mut oversized = false;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Ok(None);
        }
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(None);
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let amount = newline.map_or(available.len(), |index| index + 1);
        consumed += amount as u64;
        if bytes.len() + amount <= MAX_RECORD_BYTES && !oversized {
            bytes.extend_from_slice(&available[..amount]);
        } else {
            oversized = true;
            bytes.clear();
        }
        reader.consume(amount);
        if newline.is_some() {
            return Ok(Some((bytes, consumed)));
        }
    }
}

fn fingerprint(file: &mut File, offset: u64) -> UsageResult<String> {
    // Deterministic FNV-1a on bounded prefix and checkpoint bytes, not chat storage.
    let mut hash = 0xcbf29ce484222325_u64;
    for (start, length) in [
        (0, offset.min(FINGERPRINT_BYTES)),
        (
            offset.saturating_sub(FINGERPRINT_BYTES),
            offset.min(FINGERPRINT_BYTES),
        ),
    ] {
        file.seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        let mut bytes = vec![0; length as usize];
        file.read_exact(&mut bytes)
            .map_err(|error| error.to_string())?;
        for byte in bytes {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("{offset}:{hash:016x}"))
}

fn modified(metadata: &Metadata) -> String {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_default()
}

pub(super) fn file_identity(metadata: &Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        metadata
            .created()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_default()
    }
}
