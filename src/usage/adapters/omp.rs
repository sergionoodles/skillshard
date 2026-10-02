//! OMP 18.2.11 session v3; core messages share Pi's persisted message schema.
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use super::{activation, context, evidence, pi, text, timestamp, ParserState};
use crate::usage::{Evidence, Outcome, Record, Session, WorkerRole};

pub(super) fn parse(
    value: &Value,
    state: &mut ParserState,
    locator: &str,
) -> Result<Vec<Record>, String> {
    match value.get("type").and_then(Value::as_str) {
        Some("session") => parse_header(value, state),
        Some("session_init") => parse_initialization(value, state),
        Some("model_change") => parse_model(value, state),
        Some("message") => parse_message(value, state, locator),
        Some("custom_message") => parse_skill_prompt(value, state, locator),
        _ => Ok(Vec::new()),
    }
}

fn parse_header(value: &Value, state: &mut ParserState) -> Result<Vec<Record>, String> {
    let native_id = text(value, "id").ok_or("OMP session header has no native id")?;
    if value.get("version").and_then(Value::as_u64) != Some(3) {
        return Err("Unsupported OMP session format version".to_owned());
    }
    let parent_native_id = text(value, "parentSession").map(|parent| {
        if Path::new(&parent).is_absolute() {
            return normalize_path(Path::new(&parent))
                .to_string_lossy()
                .into_owned();
        }
        // OMP also writes opaque native IDs here. Relative paths have no documented base.
        parent
    });
    let session = Session {
        agent: "omp".to_owned(),
        native_id,
        parent_native_id,
        role: WorkerRole::Main,
        worker_name: None,
        model: None,
        project: text(value, "cwd").and_then(|path| evidence::directory_path(&path)),
    };
    state.session = Some(session.clone());
    let mut records = vec![Record::Session(session.clone())];
    if let Some(path) = state.source_path.as_ref() {
        records.push(Record::SessionPath {
            session,
            path: normalize_path(path),
        });
    }
    Ok(records)
}

fn parse_initialization(value: &Value, state: &mut ParserState) -> Result<Vec<Record>, String> {
    let Some(session) = state.session.as_mut() else {
        return Err("OMP initialization has no session header".to_owned());
    };
    // This entry is written by the task executor, unlike generic fork lineage.
    session.role = WorkerRole::Subagent;
    session.worker_name = text(value, "agent");
    session.model = text(value, "resolvedModel").or(session.model.clone());
    Ok(vec![Record::Session(session.clone())])
}

fn parse_model(value: &Value, state: &mut ParserState) -> Result<Vec<Record>, String> {
    if value
        .get("role")
        .and_then(Value::as_str)
        .is_some_and(|role| role != "default")
    {
        return Ok(Vec::new());
    }
    let Some(session) = state.session.as_mut() else {
        return Err("OMP model change has no session header".to_owned());
    };
    session.model = text(value, "model").or(session.model.clone());
    Ok(vec![Record::Session(session.clone())])
}

fn parse_message(
    value: &Value,
    state: &mut ParserState,
    locator: &str,
) -> Result<Vec<Record>, String> {
    if value["message"]["role"].as_str() == Some("fileMention") {
        return parse_file_mentions(value, state, locator);
    }
    let mut records = pi::parse_message_with(value, state, locator, Some(omp_evidence))?;
    append_entry(&mut records, value, state)?;
    Ok(records)
}

fn parse_skill_prompt(
    value: &Value,
    state: &ParserState,
    locator: &str,
) -> Result<Vec<Record>, String> {
    if value.get("customType").and_then(Value::as_str) != Some("skill-prompt") {
        return Ok(Vec::new());
    }
    let Some(reference) = text(&value["details"], "path") else {
        return Ok(Vec::new());
    };
    if !evidence::is_skill_path(&reference) || evidence::content_text(&value["content"]).is_empty()
    {
        return Ok(Vec::new());
    }
    let session = state
        .session
        .as_ref()
        .ok_or("OMP skill prompt has no session header")?;
    let mut records = vec![activation(
        session,
        None,
        text(value, "id").unwrap_or_else(|| format!("attachment:{locator}")),
        context(value, session, None),
        Evidence::Attachment,
        reference,
        Outcome::Succeeded,
    )];
    append_entry(&mut records, value, state)?;
    Ok(records)
}

fn parse_file_mentions(
    value: &Value,
    state: &ParserState,
    locator: &str,
) -> Result<Vec<Record>, String> {
    let session = state
        .session
        .as_ref()
        .ok_or("OMP file delivery has no session header")?;
    let mut records = Vec::new();
    for file in value["message"]["files"].as_array().into_iter().flatten() {
        let Some(reference) = text(file, "path") else {
            continue;
        };
        if !evidence::is_skill_path(&reference)
            || file
                .get("skippedReason")
                .is_some_and(|reason| !reason.is_null())
            || file
                .get("content")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            continue;
        }
        records.push(activation(
            session,
            None,
            text(value, "id").unwrap_or_else(|| format!("attachment:{locator}")),
            context(value, session, None),
            Evidence::Attachment,
            reference,
            Outcome::Succeeded,
        ));
    }
    append_entry(&mut records, value, state)?;
    Ok(records)
}

fn append_entry(
    records: &mut Vec<Record>,
    value: &Value,
    state: &ParserState,
) -> Result<(), String> {
    let session = state
        .session
        .as_ref()
        .ok_or("OMP entry has no session header")?;
    let Some(entry_id) = text(value, "id") else {
        if session.parent_native_id.is_some() {
            return Err(
                "OMP fork entry has no identity for inherited history attribution".to_owned(),
            );
        }
        return Ok(());
    };
    records.push(Record::SessionEntry {
        session: session.clone(),
        entry_id,
        inherits_parent: session.parent_native_id.is_some(),
        occurred_at: timestamp(value).or_else(|| value["message"]["timestamp"].as_i64()),
    });
    Ok(())
}

fn omp_evidence(name: &str, arguments: &Value) -> Option<(Evidence, Vec<String>)> {
    if name == "read" {
        let path = arguments.get("path")?.as_str()?;
        return skill_uri_references(path).map(|references| (Evidence::SkillTool, references));
    }
    if name != "bash" {
        return None;
    }
    let command = arguments.get("command")?.as_str()?;
    let references = super::shell::read_targets(command)
        .into_iter()
        .flat_map(|path| {
            skill_uri_references(&path).unwrap_or_else(|| {
                if evidence::is_skill_path(&path) {
                    vec![path]
                } else {
                    Vec::new()
                }
            })
        })
        .collect();
    Some((Evidence::ShellRead, references))
}

fn skill_uri_references(path: &str) -> Option<Vec<String>> {
    if !path
        .get(.."skill://".len())?
        .eq_ignore_ascii_case("skill://")
    {
        return None;
    }
    let resource = &path["skill://".len()..];
    let resource = resource.split(['?', '#']).next()?;
    let (host, path) = resource.split_once('/').unwrap_or((resource, ""));
    let (Some(host), Some(path)) = (decode_uri(host), decode_uri(path)) else {
        return Some(Vec::new());
    };
    let is_instruction = matches!(path.as_str(), "" | "SKILL.md");
    let is_valid_host = !host.is_empty()
        && host.len() <= super::MAX_METADATA_BYTES
        && !host.contains(['/', '\\', '\0'])
        && !host.chars().any(char::is_whitespace);
    // Empty evidence prevents generic file handling from counting URI resource paths.
    Some(if is_instruction && is_valid_host {
        vec![host]
    } else {
        Vec::new()
    })
}

fn decode_uri(value: &str) -> Option<String> {
    let mut decoded = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'%' {
            let first = (bytes.next()? as char).to_digit(16)?;
            let second = (bytes.next()? as char).to_digit(16)?;
            (first * 16 + second) as u8
        } else {
            byte
        };
        decoded.push(byte);
    }
    String::from_utf8(decoded).ok()
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if normalized.file_name().is_some() => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
#[path = "omp_tests.rs"]
mod tests;
