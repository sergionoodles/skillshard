//! Pi v0.99.1 session-manager and message contracts; extension payloads are opaque.

use serde_json::Value;

use super::{activation, context, evidence, text, timestamp, ParserState};
use crate::usage::{Evidence, Outcome, Record, Session, WorkerRole};

pub(super) type EvidenceOverride = fn(&str, &Value) -> Option<(Evidence, Vec<String>)>;

pub(super) fn parse(
    value: &Value,
    state: &mut ParserState,
    locator: &str,
) -> Result<Vec<Record>, String> {
    match value.get("type").and_then(Value::as_str) {
        Some("session") => parse_header(value, state),
        Some("model_change") => {
            let Some(session) = state.session.as_mut() else {
                return Ok(Vec::new());
            };
            session.model = text(value, "modelId");
            Ok(vec![Record::Session(session.clone())])
        }
        Some("message") => parse_message(value, state, locator),
        _ => Ok(Vec::new()),
    }
}

fn parse_header(value: &Value, state: &mut ParserState) -> Result<Vec<Record>, String> {
    let Some(header_id) = text(value, "id") else {
        return Err("Pi session header has no native id".to_owned());
    };
    if !matches!(value.get("version").and_then(Value::as_u64), Some(2 | 3)) {
        return Err("Unsupported Pi session format version".to_owned());
    }
    let project = text(value, "cwd")
        .and_then(|path| evidence::directory_path(&path))
        .map(|path| evidence::normalize_path(&path))
        .ok_or("Pi session header has no valid working directory")?;
    let native_id = serde_json::json!([project.to_string_lossy(), header_id]).to_string();
    let parent_path = text(value, "parentSession").and_then(|path| {
        let path = evidence::directory_path(&path)?;
        if path.is_absolute() {
            return Some(
                evidence::normalize_path(&path)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        state.source_path.as_ref()?.parent().map(|directory| {
            evidence::normalize_path(&directory.join(path))
                .to_string_lossy()
                .into_owned()
        })
    });
    state.pi_is_fork = text(value, "parentSession").is_some();
    if state.pi_is_fork && parent_path.is_none() {
        state.has_unbounded_inherited_history = true;
    }
    let session = Session {
        agent: "pi".to_owned(),
        native_id,
        parent_native_id: parent_path,
        role: WorkerRole::Main,
        worker_name: None,
        model: None,
        project: Some(project),
    };
    state.session = Some(session.clone());
    let mut records = vec![Record::Session(session.clone())];
    if let Some(path) = state.source_path.clone() {
        records.push(Record::SessionPath {
            session,
            path: evidence::normalize_path(&path),
        });
    }
    Ok(records)
}

pub(super) fn parse_message(
    value: &Value,
    state: &mut ParserState,
    locator: &str,
) -> Result<Vec<Record>, String> {
    let mut records = parse_message_with(value, state, locator, None)?;
    if !records
        .iter()
        .any(|record| matches!(record, Record::Activation(_) | Record::ToolResult { .. }))
    {
        return Ok(records);
    }
    let Some(entry_id) = text(value, "id") else {
        return Err("Pi session message has no entry id".to_owned());
    };
    let Some(session) = state.session.clone() else {
        return Ok(records);
    };
    records.push(Record::SessionEntry {
        session,
        entry_id,
        inherits_parent: state.pi_is_fork,
        occurred_at: message_timestamp(value),
    });
    Ok(records)
}

pub(super) fn parse_message_with(
    value: &Value,
    state: &mut ParserState,
    locator: &str,
    override_evidence: Option<EvidenceOverride>,
) -> Result<Vec<Record>, String> {
    let Some(mut session) = state.session.clone() else {
        return Err("Pi message has no session header".to_owned());
    };
    let message = &value["message"];
    let role = message.get("role").and_then(Value::as_str);
    if role == Some("assistant") {
        session.model = text(message, "model").or(session.model);
        state.session = Some(session.clone());
    }
    let mut records = vec![Record::Session(session.clone())];
    if role == Some("assistant") {
        for block in message["content"].as_array().into_iter().flatten() {
            if block.get("type").and_then(Value::as_str) == Some("toolCall") {
                // The official subagent example uses --no-session; its details are not provenance.
                if session.agent == "pi"
                    && block.get("name").and_then(Value::as_str) == Some("subagent")
                {
                    state.has_unknown_worker_identity = true;
                }
                records.extend(parse_call(value, block, &session, override_evidence));
            }
        }
        return Ok(records);
    }
    if role == Some("toolResult") {
        let Some(call_id) = text(message, "toolCallId") else {
            return Err("Pi tool result has no call id".to_owned());
        };
        records.push(Record::ToolResult {
            agent: session.agent.clone(),
            session_id: session.native_id.clone(),
            worker_id: None,
            call_id: call_id.clone(),
            outcome: boolean_outcome(message.get("isError").and_then(Value::as_bool)),
            occurred_at: message_timestamp(value),
        });
        if session.agent == "pi" {
            records.extend(parse_nested(
                value,
                &session,
                &call_id,
                state,
                override_evidence,
            ));
        }
        return Ok(records);
    }
    if role == Some("user") && session.agent == "pi" {
        let content = evidence::content_text(&message["content"]);
        if let Some(reference) = delivered_skill(&content) {
            let mut activation_context = context(value, &session, None);
            activation_context.occurred_at = message_timestamp(value);
            records.push(activation(
                &session,
                None,
                text(value, "id").unwrap_or_else(|| format!("attachment:{locator}")),
                activation_context,
                Evidence::Attachment,
                reference,
                Outcome::Succeeded,
            ));
        }
    }
    Ok(records)
}

fn parse_call(
    value: &Value,
    block: &Value,
    session: &Session,
    override_evidence: Option<EvidenceOverride>,
) -> Vec<Record> {
    let (Some(name), Some(call_id)) =
        (block.get("name").and_then(Value::as_str), text(block, "id"))
    else {
        return Vec::new();
    };
    let arguments = &block["arguments"];
    if !arguments.is_object() {
        return Vec::new();
    }
    let explicit = override_evidence.and_then(|resolve| resolve(name, arguments));
    let recognized = explicit.or_else(|| {
        let name = if name == "bash" { "Bash" } else { name };
        let namespace = block.get("namespace").and_then(Value::as_str);
        evidence::tool_evidence(name, namespace, arguments)
            .map(|(_, evidence, references)| (evidence, references))
    });
    let Some((evidence, references)) = recognized else {
        return Vec::new();
    };
    references
        .into_iter()
        .map(|reference| {
            let mut activation_context = context(value, session, Some(arguments));
            activation_context.occurred_at = message_timestamp(value);
            activation(
                session,
                None,
                call_id.clone(),
                activation_context,
                evidence,
                reference,
                Outcome::Unknown,
            )
        })
        .collect()
}

fn parse_nested(
    value: &Value,
    session: &Session,
    outer_id: &str,
    state: &mut ParserState,
    override_evidence: Option<EvidenceOverride>,
) -> Vec<Record> {
    let summary = &value["message"]["nestedCalls"];
    if summary.is_null() {
        return Vec::new();
    }
    if summary.get("complete").and_then(Value::as_bool) != Some(true) {
        state.has_unknown_worker_identity = true;
    }
    let mut records = Vec::new();
    for call in summary["calls"].as_array().into_iter().flatten() {
        let Some(nested_id) = text(call, "id") else {
            continue;
        };
        let native_id = serde_json::json!([outer_id, nested_id]).to_string();
        let mut normalized = call.clone();
        normalized["id"] = Value::String(native_id.clone());
        let activations = parse_call(value, &normalized, session, override_evidence);
        if activations.is_empty() {
            continue;
        }
        records.extend(activations);
        records.push(Record::ToolResult {
            agent: session.agent.clone(),
            session_id: session.native_id.clone(),
            worker_id: None,
            call_id: native_id,
            outcome: match call.get("status").and_then(Value::as_str) {
                Some("ok") => Outcome::Succeeded,
                Some("error") => Outcome::Failed,
                _ => Outcome::Unknown,
            },
            occurred_at: message_timestamp(value),
        });
    }
    records
}

fn boolean_outcome(is_error: Option<bool>) -> Outcome {
    match is_error {
        Some(true) => Outcome::Failed,
        Some(false) => Outcome::Succeeded,
        None => Outcome::Unknown,
    }
}

fn message_timestamp(value: &Value) -> Option<i64> {
    timestamp(value).or_else(|| value["message"]["timestamp"].as_i64())
}

fn delivered_skill(content: &str) -> Option<String> {
    let block = content.strip_prefix("<skill name=\"")?;
    let (name, block) = block.split_once("\" location=\"")?;
    let (path, block) = block.split_once("\">\n")?;
    if name.is_empty() || path.is_empty() || path.len() > super::MAX_METADATA_BYTES {
        return None;
    }
    let (_, remainder) = block.split_once("\n</skill>")?;
    (remainder.is_empty() || remainder.starts_with("\n\n")).then(|| path.to_owned())
}
