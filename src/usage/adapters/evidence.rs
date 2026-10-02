use serde_json::Value;
use std::path::{Component, Path, PathBuf};

use super::{CallKind, Evidence, Outcome};

pub(super) fn is_skill_path(path: &str) -> bool {
    path.rsplit(['/', '\\']).next() == Some("SKILL.md")
}

pub(super) fn normalize_path(path: &Path) -> PathBuf {
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

pub(super) fn directory_path(directory: &str) -> Option<PathBuf> {
    let Some(uri_path) = directory.strip_prefix("file://") else {
        return Some(PathBuf::from(directory));
    };
    if !uri_path.starts_with('/') || uri_path.contains(['?', '#']) {
        return None;
    }
    let mut decoded = Vec::new();
    let mut bytes = uri_path.bytes();
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'%' {
            let first = (bytes.next()? as char).to_digit(16)?;
            let second = (bytes.next()? as char).to_digit(16)?;
            (first * 16 + second) as u8
        } else {
            byte
        };
        if byte == 0 {
            return None;
        }
        decoded.push(byte);
    }
    let decoded = String::from_utf8(decoded).ok()?;
    let has_windows_drive = decoded
        .as_bytes()
        .get(1)
        .is_some_and(u8::is_ascii_alphabetic)
        && decoded.as_bytes().get(2) == Some(&b':');
    let path = if cfg!(windows) && has_windows_drive {
        &decoded[1..]
    } else {
        &decoded
    };
    Some(PathBuf::from(path))
}

pub(super) fn file_references(arguments: &Value) -> Vec<String> {
    ["file_path", "path", "filePath"]
        .into_iter()
        .filter_map(|key| arguments.get(key).and_then(Value::as_str))
        .filter(|path| is_skill_path(path))
        .map(str::to_owned)
        .collect()
}

pub(super) fn tool_evidence(
    name: &str,
    namespace: Option<&str>,
    arguments: &Value,
) -> Option<(CallKind, Evidence, Vec<String>)> {
    match (namespace, name) {
        (_, "Skill") => {
            let reference = arguments.get("skill")?.as_str()?.to_owned();
            Some((CallKind::Skill, Evidence::SkillTool, vec![reference]))
        }
        (Some("skills"), "read") | (_, "skills.read") | (_, "skills__read") => {
            let package = arguments.get("package")?.as_str()?;
            let resource = arguments.get("resource").and_then(Value::as_str);
            if resource.is_some_and(|resource| !is_skill_path(resource)) {
                return None;
            }
            Some((
                CallKind::Skill,
                Evidence::SkillTool,
                vec![package.to_owned()],
            ))
        }
        (_, "Read" | "read_file" | "read" | "functions.read_file") => {
            let references = file_references(arguments);
            (!references.is_empty()).then_some((CallKind::Read, Evidence::FileRead, references))
        }
        (_, "Bash" | "exec_command" | "shell_command" | "functions.exec_command") => {
            let command = arguments
                .get("cmd")
                .or_else(|| arguments.get("command"))?
                .as_str()?;
            let references = super::shell::read_paths(command);
            (!references.is_empty()).then_some((CallKind::Shell, Evidence::ShellRead, references))
        }
        _ => None,
    }
}

pub(super) fn content_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn codex_outcome(output: &Value, kind: Option<&CallKind>) -> Outcome {
    let output = content_text(output);
    if output.starts_with("Error:") || output.starts_with("<tool_use_error>") {
        return Outcome::Failed;
    }
    if let Some(code) = output.lines().find_map(|line| {
        line.strip_prefix("Process exited with code ")?
            .trim()
            .parse::<i64>()
            .ok()
    }) {
        return if code == 0 {
            Outcome::Succeeded
        } else {
            Outcome::Failed
        };
    }
    let structured = serde_json::from_str::<Value>(&output).ok();
    if let Some(code) = structured
        .as_ref()
        .and_then(|value| value.get("exit_code"))
        .and_then(Value::as_i64)
    {
        return if code == 0 {
            Outcome::Succeeded
        } else {
            Outcome::Failed
        };
    }
    if matches!(kind, Some(CallKind::Skill))
        && structured.as_ref().is_some_and(|value| {
            value.get("resource").and_then(Value::as_str).is_some()
                && value.get("contents").and_then(Value::as_str).is_some()
        })
    {
        return Outcome::Succeeded;
    }
    Outcome::Unknown
}

pub(super) fn skill_blocks(text: &str) -> Vec<String> {
    text.split("<skill>")
        .skip(1)
        .filter_map(|block| block.split_once("</skill>").map(|(block, _)| block))
        .filter_map(|block| {
            tag(block, "name")?;
            tag(block, "path")
        })
        .map(str::to_owned)
        .collect()
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let (_, content) = text.split_once(&format!("<{name}>"))?;
    let (content, _) = content.split_once(&format!("</{name}>"))?;
    let content = content.trim();
    (!content.is_empty()).then_some(content)
}
