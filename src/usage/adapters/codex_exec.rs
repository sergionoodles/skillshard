//! Structured first-party command items preserve shell reads invoked through code mode.

use serde_json::Value;

use super::{
    activation, context, remember_call, text, timestamp, CallKind, ParserState, PendingCall,
};
use crate::usage::{Evidence, Outcome, Record};

pub(super) fn parse(value: &Value, payload: &Value, state: &mut ParserState) -> Vec<Record> {
    let kind = payload.get("type").and_then(Value::as_str);
    if kind == Some("item_completed") {
        return parse_completed(value, &payload["item"], state);
    }
    if !matches!(kind, Some("exec_command_begin" | "exec_command_end")) {
        return Vec::new();
    }
    if matches!(
        payload.get("source").and_then(Value::as_str),
        Some("user_shell" | "unified_exec_interaction")
    ) {
        return Vec::new();
    }
    let (Some(session), Some(call_id)) = (state.session.clone(), text(payload, "call_id")) else {
        return Vec::new();
    };
    if kind == Some("exec_command_end") {
        state.pending_calls.remove(&call_id);
        let outcome = command_outcome(payload);
        return vec![Record::ToolResult {
            agent: session.agent.clone(),
            session_id: session.native_id.clone(),
            worker_id: None,
            call_id,
            outcome,
            occurred_at: timestamp(value),
        }];
    }
    let paths = read_paths(&payload["command"]);
    if paths.is_empty() {
        return Vec::new();
    }
    remember_call(state, &call_id, PendingCall::new(CallKind::Shell));
    paths
        .into_iter()
        .map(|reference| {
            activation(
                &session,
                None,
                call_id.clone(),
                context(value, &session, Some(payload)),
                Evidence::ShellRead,
                reference,
                Outcome::Unknown,
            )
        })
        .collect()
}

fn parse_completed(value: &Value, item: &Value, state: &mut ParserState) -> Vec<Record> {
    let Some(session) = state.session.clone() else {
        return Vec::new();
    };
    let Some(call_id) = text(item, "id") else {
        return Vec::new();
    };
    if item.get("type").and_then(Value::as_str) == Some("FunctionCallOutput") {
        let pending = state.pending_calls.remove(&call_id);
        return vec![Record::ToolResult {
            agent: session.agent.clone(),
            session_id: session.native_id.clone(),
            worker_id: None,
            call_id,
            outcome: super::evidence::codex_outcome(
                &item["output"],
                pending.as_ref().map(|pending| &pending.kind),
            ),
            occurred_at: timestamp(value),
        }];
    }
    if item.get("type").and_then(Value::as_str) != Some("CommandExecution")
        || matches!(
            item.get("source").and_then(Value::as_str),
            Some("user_shell" | "unified_exec_interaction")
        )
    {
        return Vec::new();
    }
    let paths = read_paths(&item["command"]);
    state.pending_calls.remove(&call_id);
    let outcome = command_outcome(item);
    let mut records = paths
        .into_iter()
        .map(|reference| {
            activation(
                &session,
                None,
                call_id.clone(),
                context(value, &session, Some(item)),
                Evidence::ShellRead,
                reference,
                Outcome::Unknown,
            )
        })
        .collect::<Vec<_>>();
    records.push(Record::ToolResult {
        agent: session.agent.clone(),
        session_id: session.native_id.clone(),
        worker_id: None,
        call_id,
        outcome,
        occurred_at: timestamp(value),
    });
    records
}

fn command_outcome(item: &Value) -> Outcome {
    if matches!(
        item.get("status").and_then(Value::as_str),
        Some("failed" | "declined")
    ) {
        return Outcome::Failed;
    }
    match item.get("exit_code").and_then(Value::as_i64) {
        Some(0) => Outcome::Succeeded,
        Some(_) => Outcome::Failed,
        None => Outcome::Unknown,
    }
}

fn read_paths(command: &Value) -> Vec<String> {
    let Some(arguments) = command.as_array() else {
        return Vec::new();
    };
    let Some(arguments) = arguments
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
    else {
        return Vec::new();
    };
    let Some(executable) = arguments.first() else {
        return Vec::new();
    };
    let name = executable.rsplit('/').next().unwrap_or(executable);
    if matches!(name, "bash" | "sh" | "zsh") {
        if arguments.len() == 3 && matches!(arguments[1], "-c" | "-lc") {
            return super::shell::read_paths(arguments[2]);
        }
        return Vec::new();
    }
    if arguments.iter().any(|argument| argument.contains('\'')) {
        return Vec::new();
    }
    let command = arguments
        .iter()
        .map(|argument| format!("'{argument}'"))
        .collect::<Vec<_>>()
        .join(" ");
    super::shell::read_paths(&command)
}
