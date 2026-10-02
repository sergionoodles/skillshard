//! Versioned, pure transcript adapters. No transcript contents enter checkpoints.

mod claude;
mod codex;
mod codex_exec;
mod evidence;
mod omp;
pub mod opencode;
mod pi;
mod shell;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Activation, Evidence, Outcome, Record, Session};

pub const PARSER_VERSION: u32 = 3;
const MAX_PENDING_CALLS: usize = 1024;
const MAX_METADATA_BYTES: usize = 4096;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParserState {
    #[serde(default)]
    pub(super) source_path: Option<PathBuf>,
    #[serde(default)]
    pi_is_fork: bool,
    #[serde(default)]
    recognized_metadata: bool,
    session: Option<Session>,
    worker_sessions: BTreeMap<String, Session>,
    root_id: Option<String>,
    history_start: Option<u64>,
    has_unbounded_inherited_history: bool,
    has_missing_child_ordinal: bool,
    has_unknown_worker_identity: bool,
    pending_calls: BTreeMap<String, PendingCall>,
    pending_skill_deliveries: BTreeMap<String, String>,
}

impl ParserState {
    pub fn has_session(&self) -> bool {
        self.session.is_some()
    }

    pub fn has_supported_format(&self) -> bool {
        self.has_session() || self.recognized_metadata
    }

    pub fn has_partial_attribution(&self) -> bool {
        self.has_unbounded_inherited_history
            || self.has_missing_child_ordinal
            || self.has_unknown_worker_identity
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CallKind {
    Read,
    Shell,
    Skill,
    Spawn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingCall {
    kind: CallKind,
    worker_name: Option<String>,
    model: Option<String>,
}

impl PendingCall {
    fn new(kind: CallKind) -> Self {
        Self {
            kind,
            worker_name: None,
            model: None,
        }
    }
}

pub fn parse(
    adapter: &str,
    value: &Value,
    state: &mut ParserState,
    locator: &str,
) -> Result<Vec<Record>, String> {
    match adapter {
        "codex" => codex::parse(value, state, locator),
        "claude" | "claude-code" => claude::parse(value, state, locator),
        "pi" => pi::parse(value, state, locator),
        "omp" => omp::parse(value, state, locator),
        _ => Err(format!("Unsupported usage adapter: {adapter}")),
    }
}

fn remember_call(state: &mut ParserState, call_id: &str, pending: PendingCall) {
    if state.pending_calls.len() >= MAX_PENDING_CALLS && !state.pending_calls.contains_key(call_id)
    {
        state.pending_calls.pop_first();
    }
    state.pending_calls.insert(call_id.to_owned(), pending);
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= MAX_METADATA_BYTES)
        .map(str::to_owned)
}

fn timestamp(value: &Value) -> Option<i64> {
    let timestamp = value.get("timestamp")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

fn activation(
    session: &Session,
    worker_id: Option<String>,
    native_id: String,
    context: ActivationContext,
    evidence: Evidence,
    reference: String,
    outcome: Outcome,
) -> Record {
    Record::Activation(Activation {
        session: session.clone(),
        worker_id,
        native_id,
        occurred_at: context.occurred_at,
        working_directory: context.working_directory,
        evidence,
        reference,
        outcome,
    })
}

struct ActivationContext {
    occurred_at: Option<i64>,
    working_directory: Option<PathBuf>,
}

fn context(value: &Value, session: &Session, arguments: Option<&Value>) -> ActivationContext {
    let explicit_directory = arguments.and_then(|arguments| {
        arguments
            .get("workdir")
            .or_else(|| arguments.get("cwd"))
            .and_then(Value::as_str)
    });
    let working_directory = match explicit_directory {
        Some(directory) => evidence::directory_path(directory).map(|directory| {
            if directory.is_absolute() {
                return directory;
            }
            session
                .project
                .as_ref()
                .map_or_else(|| directory.clone(), |project| project.join(&directory))
        }),
        None => session.project.clone(),
    };
    ActivationContext {
        occurred_at: timestamp(value),
        working_directory,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod context_tests;

#[cfg(test)]
mod pi_tests;
