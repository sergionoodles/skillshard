use std::path::Path;

use serde_json::{json, Value};

use super::{parse, ParserState};
use crate::usage::{Evidence, Outcome, Record};

#[test]
fn command_working_directory_changes_reference_context_without_changing_project() {
    let mut state = ParserState::default();
    let header: Value = serde_json::from_str(
        include_str!("fixtures/codex-main.jsonl")
            .lines()
            .next()
            .expect("header"),
    )
    .expect("JSON");
    parse("codex", &header, &mut state, "1").expect("header");
    let call = json!({"type":"response_item","timestamp":"2026-09-30T10:00:00Z",
        "payload":{"type":"function_call","call_id":"cwd-call","name":"exec_command",
        "arguments":"{\"cmd\":\"cat example/SKILL.md\",\"workdir\":\"/tmp/other\"}"}});
    let records = parse("codex", &call, &mut state, "2").expect("parse");
    let Some(Record::Activation(event)) = records.first() else {
        panic!("expected activation");
    };
    assert_eq!(
        event.working_directory.as_deref(),
        Some(Path::new("/tmp/other"))
    );
    assert_eq!(
        event.session.project.as_deref(),
        Some(Path::new("/tmp/skillshard-synthetic-project"))
    );
    assert_eq!(event.reference, "example/SKILL.md");
    assert_eq!(event.evidence, Evidence::ShellRead);
}

#[test]
fn result_dates_are_record_dates_and_delivery_confirms_an_orphaned_skill_call() {
    let mut state = ParserState::default();
    let call = json!({"type":"assistant","sessionId":"root","isSidechain":false,
        "message":{"content":[{"type":"tool_use","id":"skill-call","name":"Skill","input":{"skill":"example"}}]}});
    parse("claude", &call, &mut state, "1").expect("call");
    let expansion = json!({"type":"user","sessionId":"root","isSidechain":false,"isMeta":true,
        "timestamp":"2026-09-30T10:00:00Z","message":{"content":"Base directory for this skill: /tmp/example\n\nInstructions"}});
    let records = parse("claude", &expansion, &mut state, "2").expect("delivery");
    assert!(records
        .iter()
        .all(|record| !matches!(record, Record::Activation(_))));
    assert!(records
        .iter()
        .any(|record| matches!(record, Record::ToolResult {
        call_id,outcome:Outcome::Succeeded,occurred_at:Some(1_790_762_400_000),..
    } if call_id=="skill-call")));
}

#[test]
fn relative_working_directory_is_resolved_against_the_session_directory() {
    let mut state = ParserState::default();
    let call = json!({"type":"assistant","sessionId":"root","isSidechain":false,"cwd":"/tmp/project",
        "message":{"content":[{"type":"tool_use","id":"read-call","name":"Read",
        "input":{"file_path":"./example/SKILL.md","cwd":"nested"}}]}});
    let records = parse("claude", &call, &mut state, "1").expect("read");
    let event = records
        .iter()
        .find_map(|record| match record {
            Record::Activation(event) => Some(event),
            _ => None,
        })
        .expect("activation");
    assert_eq!(
        event.working_directory.as_deref(),
        Some(Path::new("/tmp/project/nested"))
    );
    assert_eq!(
        event.session.project.as_deref(),
        Some(Path::new("/tmp/project"))
    );
}

#[test]
fn a_sidechain_without_worker_identity_is_partial_and_is_never_attributed_to_main() {
    let mut state = ParserState::default();
    let call = json!({"type":"assistant","sessionId":"root","isSidechain":true,
        "message":{"content":[{"type":"tool_use","id":"call","name":"Skill","input":{"skill":"example"}}]}});
    assert!(parse("claude", &call, &mut state, "1")
        .expect("partial format")
        .is_empty());
    assert!(state.has_partial_attribution());
    assert!(!state.has_session());
}

#[test]
fn inline_worker_context_survives_other_worker_records() {
    let mut state = ParserState::default();
    let first = json!({"type":"assistant","sessionId":"root","agentId":"first","cwd":"/tmp/first",
        "message":{"content":[{"type":"tool_use","id":"first-call","name":"Read","input":{"file_path":"./SKILL.md"}}]}});
    parse("claude", &first, &mut state, "1").expect("first worker");
    let mut second = first.clone();
    second["agentId"] = json!("second");
    second["cwd"] = json!("/tmp/second");
    parse("claude", &second, &mut state, "2").expect("second worker");
    let mut resumed = first;
    resumed.as_object_mut().expect("object").remove("cwd");
    let records = parse("claude", &resumed, &mut state, "3").expect("resumed worker");
    let event = records
        .iter()
        .find_map(|record| match record {
            Record::Activation(event) => Some(event),
            _ => None,
        })
        .expect("activation");
    assert_eq!(
        event.working_directory.as_deref(),
        Some(Path::new("/tmp/first"))
    );
}

#[test]
fn codex_exec_events_load_skills_inside_code_mode_with_native_call_identity() {
    let mut state = ParserState::default();
    let header: Value = serde_json::from_str(
        include_str!("fixtures/codex-main.jsonl")
            .lines()
            .next()
            .expect("header"),
    )
    .expect("JSON");
    parse("codex", &header, &mut state, "1").expect("header");
    let fixture = include_str!("fixtures/codex-code-mode.jsonl");
    let begin: Value =
        serde_json::from_str(fixture.lines().nth(1).expect("begin event")).expect("synthetic JSON");
    let records = parse("codex", &begin, &mut state, "2").expect("begin");
    let Some(Record::Activation(event)) = records.first() else {
        panic!("expected activation")
    };
    assert_eq!(event.native_id, "nested-call");
    assert_eq!(
        event.working_directory.as_deref(),
        Some(Path::new("/tmp/my project"))
    );
    let end: Value =
        serde_json::from_str(fixture.lines().nth(2).expect("end event")).expect("synthetic JSON");
    let records = parse("codex", &end, &mut state, "3").expect("end");
    assert!(
        matches!(records.first(),Some(Record::ToolResult{call_id,outcome:Outcome::Succeeded,..}) if call_id=="nested-call")
    );
    let mut user_command = begin;
    user_command["payload"]["source"] = json!("user_shell");
    assert!(parse("codex", &user_command, &mut state, "4")
        .expect("manual shell")
        .is_empty());
    user_command["payload"]["source"] = json!("unified_exec_interaction");
    assert!(parse("codex", &user_command, &mut state, "5")
        .expect("stdin interaction")
        .is_empty());
}

#[test]
fn invalid_uri_working_directory_is_not_replaced_with_an_unrelated_project() {
    let mut state = ParserState::default();
    let call = json!({"type":"assistant","sessionId":"root","isSidechain":false,"cwd":"/tmp/project",
        "message":{"content":[{"type":"tool_use","id":"read-call","name":"Read",
        "input":{"file_path":"./example/SKILL.md","cwd":"file://other-host/project"}}]}});
    let records = parse("claude", &call, &mut state, "1").expect("read");
    let event = records
        .iter()
        .find_map(|record| match record {
            Record::Activation(event) => Some(event),
            _ => None,
        })
        .expect("event");
    assert_eq!(event.working_directory, None);
    assert_eq!(
        event.session.project.as_deref(),
        Some(Path::new("/tmp/project"))
    );
}
