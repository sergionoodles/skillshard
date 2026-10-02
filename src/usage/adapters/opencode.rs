//! Verified against anomalyco/opencode v2.0.20 schema/session-message.ts.
//! V2 projections contain settled tool state; text and arguments never persist.
use super::{activation, evidence, shell, text, ActivationContext};
use crate::usage::{Evidence, Outcome, Record, Session, UsageResult};
use serde_json::Value;
use std::path::{Path, PathBuf};

const MAX_REFERENCES: usize = 128;

pub fn parse_message(
    session: &Session,
    message_id: &str,
    message_type: &str,
    data: &Value,
    created_at: Option<i64>,
    working_directory: Option<&Path>,
) -> UsageResult<Vec<Record>> {
    let timestamp = data
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(Value::as_i64)
        .or(created_at);
    let context = ActivationContext {
        occurred_at: timestamp,
        working_directory: working_directory.map(Path::to_path_buf),
    };
    match message_type {
        "assistant" => parse_assistant(session, message_id, data, working_directory),
        "skill" => {
            let reference = text(data, "skill")
                .ok_or_else(|| "Unsupported OpenCode skill message".to_string())?;
            if !data.get("text").is_some_and(Value::is_string) {
                return Err("OpenCode skill delivery is missing content".into());
            }
            Ok(vec![activation(
                session,
                None,
                message_id.into(),
                context,
                Evidence::Attachment,
                reference,
                Outcome::Succeeded,
            )])
        }
        "user" => parse_attachments(session, message_id, data, timestamp, working_directory),
        "shell" => parse_shell(session, message_id, data, context),
        "agent-switched" | "model-switched" | "location-switched" | "synthetic" | "system"
        | "compaction" | "idle" => Ok(Vec::new()),
        _ => Err("Unsupported OpenCode message type".into()),
    }
}

fn parse_assistant(
    session: &Session,
    message_id: &str,
    data: &Value,
    working_directory: Option<&Path>,
) -> UsageResult<Vec<Record>> {
    let content = data
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| "Unsupported OpenCode assistant content".to_string())?;
    let mut emitting = session.clone();
    emitting.worker_name = text(data, "agent").or(emitting.worker_name);
    emitting.model = data
        .get("model")
        .and_then(|model| text(model, "id"))
        .or(emitting.model);
    let mut records = Vec::new();
    for (ordinal, part) in content.iter().enumerate() {
        if part.get("type").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        records.extend(parse_tool(
            &emitting,
            message_id,
            ordinal,
            part,
            working_directory,
        )?);
    }
    Ok(records)
}

fn parse_tool(
    session: &Session,
    message_id: &str,
    ordinal: usize,
    tool: &Value,
    working_directory: Option<&Path>,
) -> UsageResult<Vec<Record>> {
    let name = text(tool, "name").ok_or_else(|| "OpenCode tool is missing its name".to_string())?;
    let call_id =
        text(tool, "id").ok_or_else(|| "OpenCode tool is missing its native ID".to_string())?;
    let state = tool
        .get("state")
        .ok_or_else(|| "OpenCode tool is missing its state".to_string())?;
    let status = state.get("status").and_then(Value::as_str);
    if status == Some("streaming") {
        return Ok(Vec::new());
    }
    if !matches!(status, Some("running" | "completed" | "error")) {
        return Err("Unsupported OpenCode tool state".into());
    }
    let input = state
        .get("input")
        .filter(|input| input.is_object())
        .ok_or_else(|| "OpenCode tool input is not an object".to_string())?;
    let Some((evidence, references)) = tool_references(&name, input, state) else {
        return Ok(Vec::new());
    };
    if references.len() > MAX_REFERENCES {
        return Err("OpenCode tool contains too many references".into());
    }
    let outcome = tool_outcome(&name, state);
    let timestamp = tool
        .get("time")
        .and_then(|time| time.get("ran").or_else(|| time.get("created")))
        .and_then(Value::as_i64);
    let directory = tool_directory(input, working_directory);
    let native_id = format!("{message_id}:tool:{ordinal}:{call_id}");
    let mut records: Vec<_> = references
        .into_iter()
        .map(|reference| {
            activation(
                session,
                None,
                native_id.clone(),
                ActivationContext {
                    occurred_at: timestamp,
                    working_directory: directory.clone(),
                },
                evidence,
                reference,
                outcome,
            )
        })
        .collect();
    if outcome != Outcome::Unknown {
        records.push(Record::ToolResult {
            agent: session.agent.clone(),
            session_id: session.native_id.clone(),
            worker_id: None,
            call_id: native_id,
            outcome,
            occurred_at: timestamp,
        });
    }
    Ok(records)
}

fn tool_references(name: &str, input: &Value, state: &Value) -> Option<(Evidence, Vec<String>)> {
    match name {
        "skill" => {
            let directory = state
                .get("metadata")
                .and_then(|metadata| text(metadata, "directory"));
            let reference = directory
                .map(|directory| {
                    Path::new(&directory)
                        .join("SKILL.md")
                        .to_string_lossy()
                        .into_owned()
                })
                .or_else(|| text(input, "id"))?;
            Some((Evidence::SkillTool, vec![reference]))
        }
        "read" => {
            let is_listing = state
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|content| content.get("text").and_then(Value::as_str))
                .any(|text| text.starts_with("Read directory "));
            if is_listing {
                return None;
            }
            let references = evidence::file_references(input);
            (!references.is_empty()).then_some((Evidence::FileRead, references))
        }
        "shell" => {
            let references = shell::read_paths(input.get("command")?.as_str()?);
            (!references.is_empty()).then_some((Evidence::ShellRead, references))
        }
        _ => None,
    }
}

fn tool_outcome(name: &str, state: &Value) -> Outcome {
    match state.get("status").and_then(Value::as_str) {
        Some("error") => Outcome::Failed,
        Some("completed") if name != "shell" => Outcome::Succeeded,
        Some("completed") => {
            let metadata = state.get("metadata");
            if metadata
                .and_then(|metadata| metadata.get("status"))
                .and_then(Value::as_str)
                == Some("running")
            {
                return Outcome::Unknown;
            }
            match metadata
                .and_then(|metadata| metadata.get("exit"))
                .and_then(Value::as_i64)
            {
                Some(0) => Outcome::Succeeded,
                Some(_) => Outcome::Failed,
                None => Outcome::Unknown,
            }
        }
        _ => Outcome::Unknown,
    }
}

fn tool_directory(input: &Value, default: Option<&Path>) -> Option<PathBuf> {
    let Some(explicit) = input.get("workdir").and_then(Value::as_str) else {
        return default.map(Path::to_path_buf);
    };
    let directory = evidence::directory_path(explicit)?;
    if directory.is_absolute() {
        return Some(directory);
    }
    default.map(|default| default.join(directory))
}

fn parse_attachments(
    session: &Session,
    message_id: &str,
    data: &Value,
    timestamp: Option<i64>,
    directory: Option<&Path>,
) -> UsageResult<Vec<Record>> {
    let skill_references = data
        .get("skills")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|skill| skill.get("text").is_some_and(Value::is_string))
        .filter_map(|skill| text(skill, "id"));
    let file_references = data
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|file| file.get("data").is_some_and(Value::is_string))
        .filter_map(|file| file.get("source"))
        .filter(|source| source.get("type").and_then(Value::as_str) == Some("uri"))
        .filter_map(|source| text(source, "uri"))
        .filter_map(|uri| evidence::directory_path(&uri))
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| evidence::is_skill_path(path));
    let references: Vec<_> = skill_references.chain(file_references).collect();
    if references.len() > MAX_REFERENCES {
        return Err("OpenCode message contains too many attachments".into());
    }
    Ok(references
        .into_iter()
        .enumerate()
        .map(|(ordinal, reference)| {
            activation(
                session,
                None,
                format!("{message_id}:attachment:{ordinal}"),
                ActivationContext {
                    occurred_at: timestamp,
                    working_directory: directory.map(Path::to_path_buf),
                },
                Evidence::Attachment,
                reference,
                Outcome::Succeeded,
            )
        })
        .collect())
}

fn parse_shell(
    session: &Session,
    message_id: &str,
    data: &Value,
    context: ActivationContext,
) -> UsageResult<Vec<Record>> {
    let references = data
        .get("command")
        .and_then(Value::as_str)
        .map(shell::read_paths)
        .unwrap_or_default();
    let outcome = if data.get("status").and_then(Value::as_str) == Some("running") {
        Outcome::Unknown
    } else {
        match data.get("exit").and_then(Value::as_i64) {
            Some(0) => Outcome::Succeeded,
            Some(_) => Outcome::Failed,
            None => Outcome::Unknown,
        }
    };
    if references.len() > MAX_REFERENCES {
        return Err("OpenCode shell contains too many references".into());
    }
    Ok(references
        .into_iter()
        .map(|reference| {
            activation(
                session,
                None,
                message_id.into(),
                ActivationContext {
                    occurred_at: context.occurred_at,
                    working_directory: context.working_directory.clone(),
                },
                Evidence::ShellRead,
                reference,
                outcome,
            )
        })
        .collect())
}

#[cfg(test)]
#[path = "opencode_tests.rs"]
mod tests;
