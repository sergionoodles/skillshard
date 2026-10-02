use std::{collections::BTreeMap, path::PathBuf};

use serde_json::Value;

use super::{
    activation, context, evidence, remember_call, text, timestamp, ParserState, PendingCall,
};
use crate::usage::{Evidence, Outcome, Record, Session, WorkerRole};

pub(super) fn parse(
    value: &Value,
    state: &mut ParserState,
    locator: &str,
) -> Result<Vec<Record>, String> {
    let kind = value.get("type").and_then(Value::as_str);
    let payload = &value["payload"];
    if kind == Some("session_meta") {
        return parse_session(payload, state);
    }
    if kind == Some("turn_context") {
        return Ok(update_context(payload, state));
    }
    if kind == Some("event_msg") {
        if is_inherited(value, state)? {
            return Ok(Vec::new());
        }
        let mut records = parse_spawn(payload);
        records.extend(super::codex_exec::parse(value, payload, state));
        return Ok(records);
    }
    if kind != Some("response_item") || is_inherited(value, state)? {
        return Ok(Vec::new());
    }
    let Some(session) = state.session.clone() else {
        return Err("Codex response record has no session metadata".to_owned());
    };
    match payload.get("type").and_then(Value::as_str) {
        Some("function_call") => parse_call(value, payload, &session, state),
        Some("function_call_output" | "custom_tool_call_output") => {
            Ok(parse_result(value, payload, &session, state))
        }
        Some("message") => Ok(parse_attachment(value, payload, &session, locator)),
        _ => Ok(Vec::new()),
    }
}

fn parse_session(payload: &Value, state: &mut ParserState) -> Result<Vec<Record>, String> {
    let native_id =
        text(payload, "id").ok_or_else(|| "Codex session metadata has no thread id".to_owned())?;
    let spawn = &payload["source"]["subagent"]["thread_spawn"];
    let parent_native_id =
        text(payload, "parent_thread_id").or_else(|| text(spawn, "parent_thread_id"));
    let source = payload.get("source");
    let is_subagent = source.is_some_and(|source| source.get("subagent").is_some());
    let role = if is_subagent || parent_native_id.is_some() {
        WorkerRole::Subagent
    } else if source
        .and_then(Value::as_str)
        .is_some_and(|source| matches!(source, "cli" | "exec" | "vscode" | "mcp"))
    {
        WorkerRole::Main
    } else {
        WorkerRole::Unknown
    };
    let session = Session {
        agent: "codex".to_owned(),
        native_id,
        parent_native_id,
        role,
        worker_name: text(payload, "agent_nickname").or_else(|| text(spawn, "agent_nickname")),
        model: text(payload, "model"),
        project: text(payload, "cwd").map(PathBuf::from),
    };
    state.root_id = text(payload, "session_id");
    state.history_start = payload
        .get("subagent_history_start_ordinal")
        .and_then(Value::as_u64)
        .or_else(|| {
            payload
                .get("forked_from_ordinal_exclusive")
                .and_then(Value::as_u64)
        });
    state.has_unbounded_inherited_history = payload
        .get("forked_from_id")
        .is_some_and(|id| !id.is_null())
        && state.history_start.is_none();
    state.session = Some(session.clone());
    state.pending_calls.clear();
    Ok(vec![Record::Session(session)])
}

fn update_context(payload: &Value, state: &mut ParserState) -> Vec<Record> {
    let Some(session) = state.session.as_mut() else {
        return Vec::new();
    };
    if let Some(project) = text(payload, "cwd") {
        session.project = Some(PathBuf::from(project));
    }
    if let Some(model) = text(payload, "model") {
        session.model = Some(model);
    }
    vec![Record::Session(session.clone())]
}

fn is_inherited(value: &Value, state: &mut ParserState) -> Result<bool, String> {
    if ["inherited_user_message", "compaction_output"]
        .into_iter()
        .any(|flag| value["metadata"].get(flag).and_then(Value::as_bool) == Some(true))
    {
        return Ok(true);
    }
    if let Some(boundary) = state.history_start {
        let Some(ordinal) = value.get("ordinal").and_then(Value::as_u64) else {
            state.has_missing_child_ordinal = true;
            return Err("Codex child record lacks its inheritance ordinal".to_owned());
        };
        return Ok(ordinal < boundary);
    }
    if state.has_unbounded_inherited_history {
        return Ok(true);
    }
    Ok(false)
}

fn parse_call(
    value: &Value,
    payload: &Value,
    session: &Session,
    state: &mut ParserState,
) -> Result<Vec<Record>, String> {
    let Some(name) = payload.get("name").and_then(Value::as_str) else {
        return Ok(Vec::new());
    };
    if !is_evidence_tool(name) {
        return Ok(Vec::new());
    }
    let Some(call_id) = text(payload, "call_id") else {
        return Err("Codex tool call has no call id".to_owned());
    };
    let arguments = payload
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex tool arguments are not encoded JSON".to_owned())?;
    let arguments: Value = serde_json::from_str(arguments)
        .map_err(|_| "Codex tool arguments contain invalid JSON".to_owned())?;
    let namespace = payload.get("namespace").and_then(Value::as_str);
    let Some((kind, evidence, references)) = evidence::tool_evidence(name, namespace, &arguments)
    else {
        return Ok(Vec::new());
    };
    remember_call(state, &call_id, PendingCall::new(kind));
    Ok(references
        .into_iter()
        .map(|reference| {
            activation(
                session,
                None,
                call_id.clone(),
                context(value, session, Some(&arguments)),
                evidence,
                reference,
                Outcome::Unknown,
            )
        })
        .collect())
}

fn is_evidence_tool(name: &str) -> bool {
    matches!(
        name,
        "Skill"
            | "skills.read"
            | "skills__read"
            | "Read"
            | "read_file"
            | "read"
            | "functions.read_file"
            | "Bash"
            | "exec_command"
            | "shell_command"
            | "functions.exec_command"
    )
}

fn parse_result(
    value: &Value,
    payload: &Value,
    session: &Session,
    state: &mut ParserState,
) -> Vec<Record> {
    let Some(call_id) = text(payload, "call_id") else {
        return Vec::new();
    };
    let pending = state.pending_calls.remove(&call_id);
    let outcome = evidence::codex_outcome(
        &payload["output"],
        pending.as_ref().map(|pending| &pending.kind),
    );
    vec![Record::ToolResult {
        agent: session.agent.clone(),
        session_id: session.native_id.clone(),
        worker_id: None,
        call_id,
        outcome,
        occurred_at: timestamp(value),
    }]
}

fn parse_attachment(
    value: &Value,
    payload: &Value,
    session: &Session,
    locator: &str,
) -> Vec<Record> {
    if payload.get("role").and_then(Value::as_str) != Some("user") {
        return Vec::new();
    }
    let contents = evidence::content_text(&payload["content"]);
    let identity = text(payload, "id").unwrap_or_else(|| {
        value
            .get("ordinal")
            .and_then(Value::as_u64)
            .map(|ordinal| format!("ordinal:{ordinal}"))
            .unwrap_or_else(|| format!("record:{locator}"))
    });
    evidence::skill_blocks(&contents)
        .into_iter()
        .enumerate()
        .map(|(index, reference)| {
            activation(
                session,
                None,
                format!("attachment:{identity}:{index}"),
                context(value, session, None),
                Evidence::Attachment,
                reference,
                Outcome::Succeeded,
            )
        })
        .collect()
}

fn parse_spawn(payload: &Value) -> Vec<Record> {
    if payload.get("type").and_then(Value::as_str) == Some("item_completed") {
        return parse_completed_spawn(&payload["item"]);
    }
    if payload.get("type").and_then(Value::as_str) != Some("collab_agent_spawn_end") {
        return Vec::new();
    }
    let (Some(parent_id), Some(child_id)) = (
        text(payload, "sender_thread_id"),
        text(payload, "new_thread_id"),
    ) else {
        return Vec::new();
    };
    spawn_records(
        parent_id,
        child_id,
        text(payload, "new_agent_nickname"),
        text(payload, "model"),
    )
}

fn parse_completed_spawn(item: &Value) -> Vec<Record> {
    if item.get("type").and_then(Value::as_str) != Some("CollabAgentToolCall")
        || item.get("tool").and_then(Value::as_str) != Some("spawn_agent")
        || item.get("status").and_then(Value::as_str) != Some("completed")
    {
        return Vec::new();
    }
    let Some(parent_id) = text(item, "sender_thread_id") else {
        return Vec::new();
    };
    let mut receivers = BTreeMap::new();
    if let Some(ids) = item["receiver_thread_ids"].as_array() {
        for id in ids.iter().filter_map(Value::as_str) {
            receivers.insert(id.to_owned(), None);
        }
    }
    if let Some(agents) = item["receiver_agents"].as_array() {
        for agent in agents {
            if let Some(id) = text(agent, "thread_id") {
                receivers.insert(id, text(agent, "agent_nickname"));
            }
        }
    }
    receivers
        .into_iter()
        .flat_map(|(child_id, worker_name)| {
            spawn_records(
                parent_id.clone(),
                child_id,
                worker_name,
                text(item, "model"),
            )
        })
        .collect()
}

fn spawn_records(
    parent_id: String,
    child_id: String,
    worker_name: Option<String>,
    model: Option<String>,
) -> Vec<Record> {
    vec![
        Record::Session(Session {
            agent: "codex".to_owned(),
            native_id: child_id.clone(),
            parent_native_id: Some(parent_id.clone()),
            role: WorkerRole::Subagent,
            worker_name,
            model,
            project: None,
        }),
        Record::Link {
            agent: "codex".to_owned(),
            parent_id,
            child_id,
        },
    ]
}

#[cfg(test)]
#[path = "codex_regression_tests.rs"]
mod regression_tests;
