use serde_json::{json, Value};

use super::{parse, ParserState};
use crate::usage::{Activation, Evidence, Outcome, Record, WorkerRole};

const CODEX_MAIN: &str = include_str!("fixtures/codex-main.jsonl");
const CODEX_CHILD: &str = include_str!("fixtures/codex-child.jsonl");
const CLAUDE_MAIN: &str = include_str!("fixtures/claude-main.jsonl");
const CLAUDE_CHILD: &str = include_str!("fixtures/claude-child.jsonl");
const CLAUDE_NESTED: &str = include_str!("fixtures/claude-nested-child.jsonl");
const CLAUDE_FORK: &str = include_str!("fixtures/claude-fork.jsonl");

fn parse_fixture(adapter: &str, fixture: &str) -> Vec<Record> {
    let mut state = ParserState::default();
    fixture
        .lines()
        .enumerate()
        .flat_map(|(index, line)| {
            let value: Value = serde_json::from_str(line).expect("valid synthetic JSON");
            parse(adapter, &value, &mut state, &index.to_string()).expect("valid fixture schema")
        })
        .collect()
}

fn activations(records: &[Record]) -> Vec<&Activation> {
    records
        .iter()
        .filter_map(|record| match record {
            Record::Activation(activation) => Some(activation),
            _ => None,
        })
        .collect()
}

fn results(records: &[Record]) -> Vec<(&str, Outcome)> {
    records
        .iter()
        .filter_map(|record| match record {
            Record::ToolResult {
                call_id, outcome, ..
            } => Some((call_id.as_str(), *outcome)),
            _ => None,
        })
        .collect()
}

#[test]
fn codex_main_loads_and_duplicate_exports_have_identical_event_identities() {
    let records = parse_fixture("codex", CODEX_MAIN);
    let mirror = parse_fixture("codex", include_str!("fixtures/codex-main-mirror.jsonl"));
    assert_eq!(records, mirror);
    let events = activations(&records);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].evidence, Evidence::ShellRead);
    assert_eq!(events[0].outcome, Outcome::Unknown);
    assert_eq!(events[1].outcome, Outcome::Succeeded);
    assert_eq!(events[0].session.role, WorkerRole::Main);
    assert_eq!(
        results(&records),
        [("fixture-main-read", Outcome::Succeeded)]
    );
    assert!(records
        .iter()
        .any(|record| matches!(record, Record::Link { .. })));
}

#[test]
fn codex_child_excludes_parent_reads_and_attachments_and_keeps_pending_results() {
    let records = parse_fixture("codex", CODEX_CHILD);
    let events = activations(&records);
    assert_eq!(events.len(), 3);
    assert!(events
        .iter()
        .all(|event| event.session.role == WorkerRole::Subagent));
    assert!(events
        .iter()
        .all(|event| event.session.native_id.starts_with("22222222")));
    assert_eq!(events[0].reference, "example");
    assert_eq!(events[0].evidence, Evidence::SkillTool);
    assert_eq!(events[2].native_id, "fixture-child-pending");
    assert_eq!(events[2].outcome, Outcome::Unknown);
    assert_eq!(
        results(&records),
        [
            ("fixture-child-read", Outcome::Succeeded),
            ("fixture-child-failure", Outcome::Failed),
        ]
    );
}

#[test]
fn claude_skill_expansion_is_one_load_and_results_establish_worker_parentage() {
    let records = parse_fixture("claude-code", CLAUDE_MAIN);
    let events = activations(&records);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].evidence, Evidence::SkillTool);
    assert_eq!(events[0].session.role, WorkerRole::Main);
    assert_eq!(events[0].session.agent, "claude-code");
    assert!(records
        .iter()
        .any(|record| matches!(record, Record::Link { child_id, .. }
        if child_id.ends_with("\"fixture-child\"]"))));
}

#[test]
fn claude_nested_workers_keep_distinct_sessions_and_immediate_parents() {
    let child = parse_fixture("claude", CLAUDE_CHILD);
    let nested = parse_fixture("claude", CLAUDE_NESTED);
    let child_events = activations(&child);
    let nested_events = activations(&nested);
    assert_eq!(child_events.len(), 1);
    assert_eq!(nested_events.len(), 3);
    assert_ne!(
        child_events[0].session.native_id,
        nested_events[0].session.native_id
    );
    assert_eq!(
        nested_events[0].worker_id.as_deref(),
        Some("fixture-nested")
    );
    assert!(child.iter().any(
        |record| matches!(record, Record::Link { parent_id, child_id, .. }
        if parent_id == &child_events[0].session.native_id
            && child_id == &nested_events[0].session.native_id)
    ));
    assert_eq!(
        results(&nested),
        [
            ("fixture-claude-nested-call", Outcome::Succeeded),
            ("fixture-claude-failed-call", Outcome::Failed),
        ]
    );
}

#[test]
fn claude_fork_reference_links_context_without_replaying_parent_loads() {
    let records = parse_fixture("claude", CLAUDE_FORK);
    let events = activations(&records);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].worker_id.as_deref(), Some("fixture-fork"));
    assert_eq!(
        events[0].session.parent_native_id.as_deref(),
        Some("[\"33333333-3333-4333-8333-333333333333\",null]")
    );
}

fn claude_message(kind: &str, content: Value) -> Value {
    json!({"type":kind,"uuid":"message","sessionId":"root","isSidechain":false,
        "timestamp":"2026-09-30T12:00:00+02:00","message":{"role":kind,"content":content}})
}

#[test]
fn independent_delivered_expansion_counts_but_plain_mentions_and_catalogs_do_not() {
    let mut state = ParserState::default();
    let mut value = claude_message(
        "user",
        json!("Base directory for this skill: /tmp/example\n\nInstructions."),
    );
    assert!(activations(&parse("claude", &value, &mut state, "1").expect("parse")).is_empty());
    value["isMeta"] = json!(true);
    let records = parse("claude", &value, &mut state, "1").expect("parse");
    let events = activations(&records);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].reference, "/tmp/example/SKILL.md");
    assert_eq!(events[0].occurred_at, Some(1_790_762_400_000));
    for value in [
        json!({"type":"attachment","attachment":{"type":"skill_listing","content":"example"}}),
        json!({"type":"attachment","attachment":{"type":"invoked_skills","skills":[{"name":"example"}]}}),
    ] {
        assert!(parse("claude", &value, &mut state, "2")
            .expect("parse")
            .is_empty());
    }
}

#[test]
fn pending_call_metadata_survives_checkpoint_without_persisting_arguments() {
    let mut state = ParserState::default();
    let value = claude_message(
        "assistant",
        json!([
            {"type":"tool_use","id":"call","name":"Read","input":{"file_path":"./example/SKILL.md","private":"secret conversation"}}
        ]),
    );
    let records = parse("claude", &value, &mut state, "1").expect("parse");
    assert_eq!(activations(&records)[0].reference, "./example/SKILL.md");
    let saved = serde_json::to_string(&state).expect("serialize state");
    assert!(!saved.contains("secret conversation"));
    assert!(!saved.contains("SKILL.md"));
    let mut restored = serde_json::from_str(&saved).expect("restore state");
    let result = claude_message(
        "user",
        json!([
            {"type":"tool_result","tool_use_id":"call","is_error":true,"content":"Not allowed"}
        ]),
    );
    let records = parse("claude", &result, &mut restored, "2").expect("parse result");
    assert_eq!(results(&records), [("call", Outcome::Failed)]);
}

#[test]
fn unknown_shapes_missing_timestamps_and_non_read_tools_are_conservative() {
    assert!(parse("other", &json!({}), &mut ParserState::default(), "1").is_err());
    let mut state = ParserState::default();
    assert!(parse("codex", &json!({"type":"unknown"}), &mut state, "1")
        .expect("parse")
        .is_empty());
    for name in ["Edit", "Write", "Glob", "Grep"] {
        let value = claude_message(
            "assistant",
            json!([
                {"type":"tool_use","id":"call","name":name,"input":{"file_path":"/tmp/SKILL.md","command":"cat /tmp/SKILL.md"}}
            ]),
        );
        assert!(activations(&parse("claude", &value, &mut state, "2").expect("parse")).is_empty());
    }
    let mut value = claude_message(
        "assistant",
        json!([
            {"type":"tool_use","id":"call","name":"Skill","input":{"skill":"plugin:example"}}
        ]),
    );
    value.as_object_mut().expect("object").remove("timestamp");
    let records = parse("claude", &value, &mut state, "3").expect("parse");
    assert_eq!(activations(&records)[0].occurred_at, None);
    assert_eq!(activations(&records)[0].reference, "plugin:example");
}

#[test]
fn shell_reads_accept_quoted_paths_and_refuse_other_operations() {
    for command in [
        "cat '/tmp/my skill/SKILL.md'",
        "sed -n '1,100p' /tmp/my/SKILL.md",
        "head -n 30 /tmp/my/SKILL.md",
        "tail -20 /tmp/my/SKILL.md",
    ] {
        assert_eq!(super::shell::read_paths(command).len(), 1, "{command}");
    }
    for command in [
        "ls /tmp/SKILL.md",
        "diff /tmp/a/SKILL.md /tmp/b/SKILL.md",
        "sed -i '1p' /tmp/SKILL.md",
        "sed -n '1e' /tmp/SKILL.md",
        "echo cat /tmp/SKILL.md",
        "cat $(echo /tmp/SKILL.md)",
        "cat /tmp/SKILL.md > /tmp/copy",
        "cat /tmp/SKILL.md | wc",
        "cat /tmp/SKILL.md; rm /tmp/file",
        "cat '/tmp/SKILL.md",
        "rg SKILL.md /tmp",
    ] {
        assert!(super::shell::read_paths(command).is_empty(), "{command}");
    }
}

#[test]
fn codex_namespace_and_result_success_are_explicit() {
    let mut state = ParserState::default();
    let header: Value =
        serde_json::from_str(CODEX_MAIN.lines().next().expect("header")).expect("JSON");
    parse("codex", &header, &mut state, "1").expect("header");
    let call = json!({"type":"response_item","payload":{"type":"function_call","call_id":"call","name":"read","namespace":"skills","arguments":"{\"package\":\"plugin:example\",\"resource\":\"skill://example/references/help.md\"}"}});
    assert!(parse("codex", &call, &mut state, "2")
        .expect("parse")
        .is_empty());
    let output = json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call","output":"some text"}});
    assert_eq!(
        results(&parse("codex", &output, &mut state, "3").expect("parse")),
        [("call", Outcome::Unknown)]
    );
}

#[test]
fn inherited_codex_metadata_excludes_delivery_even_without_an_ordinal_boundary() {
    let mut state = ParserState::default();
    let records: Vec<Value> = CODEX_MAIN
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON"))
        .collect();
    parse("codex", &records[0], &mut state, "1").expect("header");
    let mut attachment = records[3].clone();
    attachment["metadata"] = json!({"inherited_user_message":true});
    assert!(parse("codex", &attachment, &mut state, "2")
        .expect("parse")
        .is_empty());
    attachment["metadata"] = json!({"compaction_output":true});
    assert!(parse("codex", &attachment, &mut state, "3")
        .expect("parse")
        .is_empty());
}

#[test]
fn interleaved_workers_do_not_share_pending_calls_or_skill_expansion_suppression() {
    let mut state = ParserState::default();
    let mut first = claude_message(
        "assistant",
        json!([
            {"type":"tool_use","id":"same-call","name":"Skill","input":{"skill":"example"}}
        ]),
    );
    first["agentId"] = json!("first");
    first["isSidechain"] = json!(true);
    let first_records = parse("claude", &first, &mut state, "1").expect("first worker");
    let mut second = first.clone();
    second["agentId"] = json!("second");
    let second_records = parse("claude", &second, &mut state, "2").expect("second worker");
    assert_ne!(
        activations(&first_records)[0].session.native_id,
        activations(&second_records)[0].session.native_id
    );
    let mut expansion = claude_message(
        "user",
        json!("Base directory for this skill: /tmp/example\n\nInstructions"),
    );
    expansion["agentId"] = json!("first");
    expansion["isMeta"] = json!(true);
    assert!(
        activations(&parse("claude", &expansion, &mut state, "3").expect("expansion")).is_empty()
    );
    expansion["agentId"] = json!("third");
    assert_eq!(
        activations(&parse("claude", &expansion, &mut state, "4").expect("independent expansion"))
            .len(),
        1
    );
    let mut result = claude_message(
        "user",
        json!([
            {"type":"tool_result","tool_use_id":"same-call","content":"Launching skill: example"}
        ]),
    );
    result["agentId"] = json!("second");
    let records = parse("claude", &result, &mut state, "5").expect("result");
    assert_eq!(results(&records), [("same-call", Outcome::Succeeded)]);
    assert_eq!(state.pending_calls.len(), 1);
}

#[test]
fn unbounded_inherited_history_and_missing_child_ordinals_report_partial_attribution() {
    let mut state = ParserState::default();
    let mut records: Vec<Value> = CODEX_CHILD
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON"))
        .collect();
    parse("codex", &records[0], &mut state, "1").expect("header");
    records[4]
        .as_object_mut()
        .expect("object")
        .remove("ordinal");
    assert!(parse("codex", &records[4], &mut state, "2").is_err());
    records[0]["payload"]
        .as_object_mut()
        .expect("payload")
        .remove("subagent_history_start_ordinal");
    records[0]["payload"]
        .as_object_mut()
        .expect("object")
        .remove("forked_from_ordinal_exclusive");
    parse("codex", &records[0], &mut state, "3").expect("legacy header");
    assert!(parse("codex", &records[4], &mut state, "4")
        .expect("legacy format")
        .is_empty());
    assert!(state.has_partial_attribution());
}

#[test]
fn dedicated_skill_references_keep_namespaces_and_unknown_roles_stay_unknown() {
    let mut state = ParserState::default();
    let mut header: Value =
        serde_json::from_str(CODEX_MAIN.lines().next().expect("header")).expect("JSON");
    header["payload"]
        .as_object_mut()
        .expect("payload")
        .remove("source");
    parse("codex", &header, &mut state, "1").expect("header");
    let call = json!({"type":"response_item","payload":{"type":"function_call","call_id":"call","name":"read","namespace":"skills","arguments":"{\"package\":\"plugin:example\"}"}});
    let records = parse("codex", &call, &mut state, "2").expect("parse");
    assert_eq!(activations(&records)[0].reference, "plugin:example");
    assert_eq!(activations(&records)[0].session.role, WorkerRole::Unknown);
    assert!(
        super::evidence::skill_blocks("Mention <skill><name>example</name></skill>").is_empty()
    );
}
