use serde_json::{json, Value};
use std::path::PathBuf;

use crate::usage::adapters::omp::parse;
use crate::usage::adapters::ParserState;
use crate::usage::{Activation, Evidence, Outcome, Record, WorkerRole};

fn parse_fixture(fixture: &str, path: &str) -> (Vec<Record>, ParserState) {
    let mut state = ParserState {
        source_path: Some(PathBuf::from(path)),
        ..ParserState::default()
    };
    let records = fixture
        .lines()
        .enumerate()
        .flat_map(|(index, line)| {
            let value: Value = serde_json::from_str(line).expect("synthetic JSON");
            parse(&value, &mut state, &index.to_string()).expect("supported fixture")
        })
        .collect();
    (records, state)
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

fn header(parent: Option<&str>) -> Value {
    json!({"type":"session","version":3,"id":"omp-session","timestamp":"2026-09-30T12:00:00Z","cwd":"/tmp/project","parentSession":parent})
}

fn read_message(path: &str) -> Value {
    json!({"type":"message","id":"read-entry","timestamp":"2026-09-30T12:00:01Z",
        "message":{"role":"assistant","content":[{"type":"toolCall","id":"read-call","name":"read","arguments":{"path":path}}]}})
}

#[test]
fn main_observes_deliveries_reads_and_results_without_mentions_or_resources() {
    let (records, state) = parse_fixture(
        include_str!("fixtures/omp-main.jsonl"),
        "/tmp/omp-sessions/main.jsonl",
    );
    let events = activations(&records);
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].reference, "plugin:example");
    assert_eq!(events[0].evidence, Evidence::SkillTool);
    assert_eq!(events[0].outcome, Outcome::Unknown);
    assert_eq!(events[1].evidence, Evidence::ShellRead);
    assert_eq!(events[2].evidence, Evidence::Attachment);
    assert_eq!(events[3].reference, "/tmp/attached/SKILL.md");
    assert!(events
        .iter()
        .all(|event| event.session.role == WorkerRole::Main));
    assert!(records.iter().any(|record| matches!(record, Record::ToolResult {call_id,outcome:Outcome::Succeeded,..} if call_id == "omp-uri-read")));
    assert!(records.iter().any(|record| matches!(record, Record::ToolResult {call_id,outcome:Outcome::Failed,..} if call_id == "omp-shell-read")));
    assert!(records.iter().any(|record| matches!(record, Record::SessionPath {session,path} if session.native_id == "omp-main-native" && path == &PathBuf::from("/tmp/omp-sessions/main.jsonl"))));
    assert!(!state.has_partial_attribution());
}

#[test]
fn task_initialization_provides_child_metadata_without_counting_initial_catalogs() {
    let (records, state) = parse_fixture(
        include_str!("fixtures/omp-child.jsonl"),
        "/tmp/omp-sessions/main/reviewer.jsonl",
    );
    let events = activations(&records);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].outcome, Outcome::Succeeded);
    assert!(events
        .iter()
        .all(|event| event.session.role == WorkerRole::Subagent));
    assert_eq!(events[1].session.native_id, "omp-child-native");
    assert_eq!(
        events[1].session.parent_native_id.as_deref(),
        Some("/tmp/omp-sessions/main.jsonl")
    );
    assert_eq!(events[1].session.worker_name.as_deref(), Some("reviewer"));
    assert_eq!(
        events[0].session.model.as_deref(),
        Some("fixture/child-model")
    );
    assert!(!state.has_partial_attribution());
}

#[test]
fn resumed_parser_preserves_worker_identity_without_persisting_message_content() {
    let (_, state) = parse_fixture(
        include_str!("fixtures/omp-child.jsonl"),
        "/tmp/omp-sessions/main/reviewer.jsonl",
    );
    let checkpoint = serde_json::to_string(&state).expect("checkpoint");
    assert!(!checkpoint.contains("Synthetic"));
    assert!(!checkpoint.contains("systemPrompt"));
    let mut resumed: ParserState = serde_json::from_str(&checkpoint).expect("restored metadata");
    let records =
        parse(&read_message("skill://example"), &mut resumed, "200").expect("continued read");
    let event = activations(&records)[0];
    assert_eq!(event.session.native_id, "omp-child-native");
    assert_eq!(event.session.worker_name.as_deref(), Some("reviewer"));
    assert_eq!(event.session.role, WorkerRole::Subagent);
}

#[test]
fn task_result_worker_labels_do_not_get_guessed_as_native_session_ids() {
    let mut state = ParserState::default();
    parse(&header(None), &mut state, "0").expect("header");
    let result = json!({"type":"message","id":"task-result-entry","timestamp":"2026-09-30T12:00:01Z",
        "message":{"role":"toolResult","toolCallId":"task-call","toolName":"task","isError":false,
            "content":[{"type":"text","text":"Synthetic output."}],
            "details":{"results":[{"id":"reviewer-worker","agent":"reviewer","exitCode":0,"resolvedModel":"fixture/child","outputPath":"/tmp/results/reviewer-worker.md"}],"totalDurationMs":20}}});
    let records = parse(&result, &mut state, "1").expect("task result");
    assert!(activations(&records).is_empty());
    assert!(!records
        .iter()
        .any(|record| matches!(record, Record::Link { .. })));
    assert!(records
        .iter()
        .filter_map(|record| match record {
            Record::Session(session) => Some(session),
            _ => None,
        })
        .all(|session| session.native_id == "omp-session"));
}

#[test]
fn forks_preserve_original_entry_identity_for_storage_reconciliation() {
    let mut state = ParserState::default();
    parse(&header(Some("native-parent")), &mut state, "0").expect("header");
    let records = parse(&read_message("/tmp/example/SKILL.md"), &mut state, "100").expect("entry");
    assert_eq!(activations(&records)[0].session.role, WorkerRole::Main);
    assert!(records.iter().any(|record| matches!(record, Record::SessionEntry {entry_id,inherits_parent:true,..} if entry_id == "read-entry")));
    let mut missing_id = read_message("/tmp/example/SKILL.md");
    missing_id.as_object_mut().expect("object").remove("id");
    assert!(parse(&missing_id, &mut state, "200").is_err());
}

#[test]
fn skill_uris_preserve_namespaces_and_reject_non_instruction_resources() {
    let mut state = ParserState::default();
    parse(&header(None), &mut state, "0").expect("header");
    for path in [
        "skill://example",
        "skill://example/",
        "skill://example/SKILL.md",
        "skill://example/%53KILL.md",
        "SKILL://example/SKILL.md",
    ] {
        let records = parse(&read_message(path), &mut state, "1").expect("entry");
        assert_eq!(activations(&records)[0].reference, "example", "{path}");
    }
    for path in [
        "skill://",
        "skill://example/docs/SKILL.md",
        "skill://example/../SKILL.md",
        "skill://example/scripts/helper.rs",
        "skill:///SKILL.md",
        "skill://bad%zz/SKILL.md",
    ] {
        let records = parse(&read_message(path), &mut state, "1").expect("entry");
        assert!(activations(&records).is_empty(), "{path}");
    }
    let records = parse(
        &read_message("skill://plugin%3Aexample/SKILL.md"),
        &mut state,
        "1",
    )
    .expect("entry");
    assert_eq!(activations(&records)[0].reference, "plugin:example");
}

#[test]
fn shell_reads_resolve_skill_uris_and_paths_without_counting_directory_commands() {
    let mut state = ParserState::default();
    parse(&header(None), &mut state, "0").expect("header");
    let message = |command: &str| {
        json!({"type":"message","id":"shell-entry","timestamp":"2026-09-30T12:00:01Z",
        "message":{"role":"assistant","content":[{"type":"toolCall","id":"shell-call","name":"bash","arguments":{"command":command}}]}})
    };
    let records = parse(
        &message("cat skill://example skill://plugin:example/SKILL.md /tmp/local/SKILL.md"),
        &mut state,
        "1",
    )
    .expect("shell read");
    let events = activations(&records);
    assert_eq!(
        events
            .iter()
            .map(|event| event.reference.as_str())
            .collect::<Vec<_>>(),
        ["example", "plugin:example", "/tmp/local/SKILL.md"]
    );
    assert!(events
        .iter()
        .all(|event| event.evidence == Evidence::ShellRead));
    for command in [
        "ls skill://example",
        "cd skill://example",
        "cat skill://example/scripts/helper.rs",
        "cat skill://example/docs/SKILL.md",
        "cat skill://example; echo other",
        "cat $(echo skill://example)",
    ] {
        assert!(
            activations(&parse(&message(command), &mut state, "2").expect("excluded command"))
                .is_empty(),
            "{command}"
        );
    }
}

#[test]
fn unknown_versions_missing_headers_and_unsupported_types_fail_conservatively() {
    let mut state = ParserState::default();
    assert!(parse(&read_message("/tmp/example/SKILL.md"), &mut state, "0").is_err());
    let mut value = header(None);
    value["version"] = json!(42);
    assert!(parse(&value, &mut state, "0").is_err());
    assert!(
        parse(&json!({"type":"title","title":"SKILL.md"}), &mut state, "0")
            .expect("ignored title")
            .is_empty()
    );
    assert!(parse(
        &json!({"type":"custom_message","customType":"other","content":"SKILL.md"}),
        &mut state,
        "0"
    )
    .expect("ignored custom")
    .is_empty());
}

#[test]
fn nondefault_model_changes_do_not_replace_the_worker_model() {
    let mut state = ParserState::default();
    parse(&header(None), &mut state, "0").expect("header");
    parse(
        &json!({"type":"model_change","model":"fixture/main"}),
        &mut state,
        "1",
    )
    .expect("model");
    parse(
        &json!({"type":"model_change","model":"fixture/smol","role":"smol"}),
        &mut state,
        "2",
    )
    .expect("secondary model");
    let records = parse(&read_message("/tmp/example/SKILL.md"), &mut state, "3").expect("entry");
    assert_eq!(
        activations(&records)[0].session.model.as_deref(),
        Some("fixture/main")
    );
}

#[test]
fn pi_specific_delivery_and_nested_summary_shapes_do_not_invent_omp_activations() {
    let mut state = ParserState::default();
    parse(&header(None), &mut state, "0").expect("header");
    let mention = json!({"type":"message","id":"user-entry","timestamp":"2026-09-30T12:00:01Z",
        "message":{"role":"user","content":"<skill name=\"example\" location=\"/tmp/example/SKILL.md\">\nSynthetic user-authored XML.\n</skill>"}});
    assert!(activations(&parse(&mention, &mut state, "1").expect("user")).is_empty());
    let result = json!({"type":"message","id":"result-entry","timestamp":"2026-09-30T12:00:02Z",
        "message":{"role":"toolResult","toolCallId":"outer-call","isError":false,"content":[],
            "nestedCalls":{"complete":true,"calls":[{"id":"nested-call","name":"read","arguments":{"path":"/tmp/example/SKILL.md"},"status":"ok"}]}}});
    assert!(activations(&parse(&result, &mut state, "2").expect("result")).is_empty());
}
