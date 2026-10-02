use std::path::PathBuf;

use serde_json::Value;

use super::{
    activation, context, evidence, remember_call, text, timestamp, CallKind, ParserState,
    PendingCall,
};
use crate::usage::{Evidence, Outcome, Record, Session, WorkerRole};

const SKILL_DIRECTORY_PREFIX: &str = "Base directory for this skill: ";

pub(super) fn parse(
    value: &Value,
    state: &mut ParserState,
    locator: &str,
) -> Result<Vec<Record>, String> {
    let kind = value.get("type").and_then(Value::as_str);
    if kind == Some("fork-context-ref") {
        return parse_fork(value, state);
    }
    if !matches!(kind, Some("user" | "assistant")) {
        // Claude also retains valid files containing only bookkeeping records.
        state.recognized_metadata |= matches!(
            kind,
            Some(
                "progress"
                    | "file-history-snapshot"
                    | "file-history-delta"
                    | "last-prompt"
                    | "continued-in"
                    | "content-replacement"
                    | "api-request-shape"
                    | "api-request-blob"
                    | "api-request"
                    | "frame-link"
                    | "summary"
                    | "custom-title"
                    | "ai-title"
                    | "agent-name"
                    | "agent-color"
                    | "agent-setting"
                    | "history-suppression"
                    | "attribution-snapshot"
                    | "mode"
                    | "permission-mode"
                    | "isolation-latch"
                    | "dev-mods"
                    | "memory-mode"
                    | "atis-latch"
                    | "worktree-state"
                    | "cost-state"
                    | "queue-operation"
                    | "observer-ref"
                    | "artifact-autoreact-ledger"
            )
        );
        return Ok(Vec::new());
    }
    let Some(root_id) = text(value, "sessionId") else {
        return Err("Claude message has no session id".to_owned());
    };
    let worker_id = text(value, "agentId");
    if worker_id.is_none() && value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        state.has_unknown_worker_identity = true;
        return Ok(Vec::new());
    }
    let session = session(value, &root_id, worker_id.as_deref(), state);
    if state.worker_sessions.len() >= super::MAX_PENDING_CALLS
        && !state.worker_sessions.contains_key(&session.native_id)
    {
        state.worker_sessions.pop_first();
    }
    state
        .worker_sessions
        .insert(session.native_id.clone(), session.clone());
    state.root_id = Some(root_id.clone());
    state.session = Some(session.clone());
    let mut records = vec![Record::Session(session.clone())];
    let message = &value["message"];
    if kind == Some("assistant") {
        state.pending_skill_deliveries.remove(&session.native_id);
        for block in message["content"].as_array().into_iter().flatten() {
            records.extend(parse_call(value, block, &session, worker_id.clone(), state));
        }
        return Ok(records);
    }
    let mut has_result = false;
    for block in message["content"].as_array().into_iter().flatten() {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        has_result = true;
        records.extend(parse_result(
            value,
            block,
            &session,
            &root_id,
            worker_id.clone(),
            state,
        ));
    }
    if !has_result {
        records.extend(parse_expansion(value, &session, worker_id, locator, state));
    }
    Ok(records)
}

fn native_id(root_id: &str, worker_id: Option<&str>) -> String {
    serde_json::json!([root_id, worker_id]).to_string()
}

fn session(value: &Value, root_id: &str, worker_id: Option<&str>, state: &ParserState) -> Session {
    let native_id = native_id(root_id, worker_id);
    let previous = state
        .session
        .as_ref()
        .filter(|session| session.native_id == native_id)
        .or_else(|| state.worker_sessions.get(&native_id));
    let role = if worker_id.is_some() {
        WorkerRole::Subagent
    } else if value.get("isSidechain").and_then(Value::as_bool) == Some(false) {
        WorkerRole::Main
    } else {
        WorkerRole::Unknown
    };
    Session {
        agent: "claude-code".to_owned(),
        native_id,
        parent_native_id: previous.and_then(|session| session.parent_native_id.clone()),
        role,
        worker_name: text(value, "agentName")
            .or_else(|| previous.and_then(|session| session.worker_name.clone())),
        model: text(&value["message"], "model")
            .or_else(|| previous.and_then(|session| session.model.clone())),
        project: text(value, "cwd")
            .map(PathBuf::from)
            .or_else(|| previous.and_then(|session| session.project.clone())),
    }
}

fn pending_key(session: &Session, call_id: &str) -> String {
    serde_json::json!([session.native_id, call_id]).to_string()
}

fn parse_call(
    value: &Value,
    block: &Value,
    session: &Session,
    worker_id: Option<String>,
    state: &mut ParserState,
) -> Vec<Record> {
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return Vec::new();
    }
    let (Some(name), Some(call_id)) =
        (block.get("name").and_then(Value::as_str), text(block, "id"))
    else {
        return Vec::new();
    };
    let arguments = &block["input"];
    let key = pending_key(session, &call_id);
    if matches!(name, "Agent" | "Task") {
        remember_call(
            state,
            &key,
            PendingCall {
                kind: CallKind::Spawn,
                worker_name: text(arguments, "name"),
                model: text(arguments, "model"),
            },
        );
        return Vec::new();
    }
    let Some((kind, evidence, references)) = evidence::tool_evidence(name, None, arguments) else {
        return Vec::new();
    };
    if matches!(kind, CallKind::Skill) {
        if state.pending_skill_deliveries.len() >= super::MAX_PENDING_CALLS
            && !state
                .pending_skill_deliveries
                .contains_key(&session.native_id)
        {
            state.pending_skill_deliveries.pop_first();
        }
        state
            .pending_skill_deliveries
            .insert(session.native_id.clone(), call_id.clone());
    }
    remember_call(state, &key, PendingCall::new(kind));
    references
        .into_iter()
        .map(|reference| {
            activation(
                session,
                worker_id.clone(),
                call_id.clone(),
                context(value, session, Some(arguments)),
                evidence,
                reference,
                Outcome::Unknown,
            )
        })
        .collect()
}

fn parse_result(
    value: &Value,
    block: &Value,
    session: &Session,
    root_id: &str,
    worker_id: Option<String>,
    state: &mut ParserState,
) -> Vec<Record> {
    let Some(call_id) = text(block, "tool_use_id") else {
        return Vec::new();
    };
    let pending = state.pending_calls.remove(&pending_key(session, &call_id));
    let has_failed = block.get("is_error").and_then(Value::as_bool) == Some(true)
        || value["toolUseResult"]
            .get("interrupted")
            .and_then(Value::as_bool)
            == Some(true);
    let outcome = if has_failed {
        Outcome::Failed
    } else {
        Outcome::Succeeded
    };
    let mut records = vec![Record::ToolResult {
        agent: session.agent.clone(),
        session_id: session.native_id.clone(),
        worker_id,
        call_id,
        outcome,
        occurred_at: timestamp(value),
    }];
    if pending
        .as_ref()
        .is_some_and(|pending| matches!(pending.kind, CallKind::Skill))
        && has_failed
    {
        state.pending_skill_deliveries.remove(&session.native_id);
    }
    if !has_failed {
        if let Some(pending) = pending.filter(|pending| matches!(pending.kind, CallKind::Spawn)) {
            records.extend(spawn_records(value, block, session, root_id, &pending));
        }
    }
    records
}

fn spawn_records(
    value: &Value,
    block: &Value,
    session: &Session,
    root_id: &str,
    pending: &PendingCall,
) -> Vec<Record> {
    let child_worker = text(&value["toolUseResult"], "agentId").or_else(|| {
        let output = evidence::content_text(&block["content"]);
        output.lines().find_map(|line| {
            line.strip_prefix("agentId: ")?
                .split_whitespace()
                .next()
                .map(str::to_owned)
        })
    });
    let Some(child_worker) = child_worker else {
        return Vec::new();
    };
    let child_id = native_id(root_id, Some(&child_worker));
    vec![
        Record::Session(Session {
            agent: session.agent.clone(),
            native_id: child_id.clone(),
            parent_native_id: Some(session.native_id.clone()),
            role: WorkerRole::Subagent,
            worker_name: pending.worker_name.clone(),
            model: pending.model.clone(),
            project: None,
        }),
        Record::Link {
            agent: session.agent.clone(),
            parent_id: session.native_id.clone(),
            child_id,
        },
    ]
}

fn parse_expansion(
    value: &Value,
    session: &Session,
    worker_id: Option<String>,
    locator: &str,
    state: &mut ParserState,
) -> Vec<Record> {
    let called_id = state.pending_skill_deliveries.remove(&session.native_id);
    if value.get("isMeta").and_then(Value::as_bool) != Some(true) {
        return Vec::new();
    }
    let contents = evidence::content_text(&value["message"]["content"]);
    let Some(directory) = contents
        .lines()
        .next()
        .and_then(|line| line.strip_prefix(SKILL_DIRECTORY_PREFIX))
    else {
        return Vec::new();
    };
    if directory.trim().is_empty() {
        return Vec::new();
    }
    if let Some(call_id) = called_id {
        return vec![Record::ToolResult {
            agent: session.agent.clone(),
            session_id: session.native_id.clone(),
            worker_id,
            call_id,
            outcome: Outcome::Succeeded,
            occurred_at: timestamp(value),
        }];
    }
    let reference = PathBuf::from(directory.trim())
        .join("SKILL.md")
        .to_string_lossy()
        .into_owned();
    let identity = text(value, "uuid").unwrap_or_else(|| format!("record:{locator}"));
    vec![activation(
        session,
        worker_id,
        format!("attachment:{identity}"),
        context(value, session, None),
        Evidence::Attachment,
        reference,
        Outcome::Succeeded,
    )]
}

fn parse_fork(value: &Value, state: &mut ParserState) -> Result<Vec<Record>, String> {
    let (Some(root_id), Some(worker_id)) = (text(value, "parentSessionId"), text(value, "agentId"))
    else {
        return Err("Claude fork reference lacks parent or worker identity".to_owned());
    };
    let parent_id = native_id(&root_id, None);
    let child_id = native_id(&root_id, Some(&worker_id));
    state.root_id = Some(root_id);
    let session = Session {
        agent: "claude-code".to_owned(),
        native_id: child_id.clone(),
        parent_native_id: Some(parent_id.clone()),
        role: WorkerRole::Subagent,
        ..Session::default()
    };
    state.session = Some(session.clone());
    Ok(vec![
        Record::Session(session),
        Record::Link {
            agent: "claude-code".to_owned(),
            parent_id,
            child_id,
        },
    ])
}
