use std::path::Path;

use serde_json::{json, Value};

use super::{parse, ParserState};
use crate::usage::{Evidence, Outcome, Record, WorkerRole};

fn fixture_records(fixture: &str) -> (ParserState, Vec<Record>) {
    let mut state = ParserState::default();
    let records = fixture
        .lines()
        .enumerate()
        .flat_map(|(index, line)| {
            let value = serde_json::from_str::<Value>(line).expect("synthetic JSON");
            parse("pi", &value, &mut state, &index.to_string()).expect("Pi parse")
        })
        .collect();
    (state, records)
}

fn initialized() -> ParserState {
    fixture_records(
        include_str!("fixtures/pi-main.jsonl")
            .lines()
            .next()
            .expect("header"),
    )
    .0
}

#[test]
fn pi_reads_delivery_results_and_nested_calls_have_native_identity() {
    let (state, records) = fixture_records(include_str!("fixtures/pi-main.jsonl"));
    let events = records
        .iter()
        .filter_map(|record| match record {
            Record::Activation(event) => Some(event),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 6);
    assert!(state.has_session());
    assert!(!state.has_partial_attribution());
    let attachment = events
        .iter()
        .find(|event| event.evidence == Evidence::Attachment)
        .expect("delivery");
    assert_eq!(attachment.native_id, "pi-attachment");
    assert_eq!(attachment.reference, "/tmp/example/SKILL.md");
    assert_eq!(attachment.outcome, Outcome::Succeeded);
    let shell = events
        .iter()
        .find(|event| event.native_id == "pi-bash")
        .expect("shell");
    assert_eq!(shell.evidence, Evidence::ShellRead);
    assert_eq!(
        shell.working_directory.as_deref(),
        Some(Path::new("/tmp/skillshard-synthetic-project"))
    );
    assert_eq!(shell.outcome, Outcome::Unknown);
    assert!(records.iter().any(|record| matches!(record, Record::ToolResult{call_id,outcome:Outcome::Failed,..} if call_id == "pi-bash")));
    assert!(records.iter().any(|record| matches!(record, Record::ToolResult{call_id,outcome:Outcome::Succeeded,..} if call_id == "[\"pi-code\",\"nested-read\"]")));
}

#[test]
fn pi_mirrors_and_checkpoint_resume_preserve_identity_without_persisting_content() {
    let fixture = include_str!("fixtures/pi-main.jsonl");
    let (_, original) = fixture_records(fixture);
    let (_, mirror) = fixture_records(fixture);
    assert_eq!(original, mirror);
    let mut state = initialized();
    let call: Value = serde_json::from_str(fixture.lines().nth(3).expect("call")).expect("JSON");
    parse("pi", &call, &mut state, "different-locator").expect("call");
    let checkpoint = serde_json::to_string(&state).expect("checkpoint");
    assert!(!checkpoint.contains("cat "));
    assert!(!checkpoint.contains("Synthetic instructions"));
    let mut resumed: ParserState = serde_json::from_str(&checkpoint).expect("restore");
    let result: Value =
        serde_json::from_str(fixture.lines().nth(4).expect("result")).expect("JSON");
    assert!(parse("pi", &result, &mut resumed, "result").expect("result").iter().any(|record| matches!(record,Record::ToolResult{call_id,outcome:Outcome::Succeeded,..} if call_id == "pi-read")));
}

#[test]
fn pi_catalog_mentions_restored_context_and_nonread_tools_are_excluded() {
    let mut state = initialized();
    let cases = [
        json!({"type":"custom","id":"extension","data":{"parentSession":"invented","agentId":"invented"}}),
        json!({"type":"compaction","id":"compact","summary":"<skill name=\"example\" location=\"/tmp/example/SKILL.md\">\nRestored\n</skill>"}),
        json!({"type":"message","id":"catalog","message":{"role":"system","content":"<skill name=\"example\" location=\"/tmp/example/SKILL.md\">\nCatalog\n</skill>"}}),
        json!({"type":"message","id":"mention","message":{"role":"user","content":"Please read /tmp/example/SKILL.md or invoke /skill:example"}}),
        json!({"type":"message","id":"tools","message":{"role":"assistant","content":[
            {"type":"toolCall","id":"write","name":"write","arguments":{"path":"/tmp/example/SKILL.md","content":"changed"}},
            {"type":"toolCall","id":"edit","name":"bash","arguments":{"command":"sed -i s/a/b/ /tmp/example/SKILL.md"}},
            {"type":"toolCall","id":"list","name":"bash","arguments":{"command":"ls /tmp/example/SKILL.md"}},
            {"type":"toolCall","id":"badargs","name":"read","arguments":"{\"path\":\"/tmp/example/SKILL.md\"}"}
        ]}}),
    ];
    for value in cases {
        assert!(parse("pi", &value, &mut state, "negative")
            .expect("ignored")
            .iter()
            .all(|record| !matches!(record, Record::Activation(_))));
    }
    assert!(state
        .session
        .as_ref()
        .expect("session")
        .parent_native_id
        .is_none());
}

#[test]
fn pi_results_require_explicit_status_and_use_verified_nested_timestamps() {
    let mut state = initialized();
    let result = json!({"type":"message","id":"result","message":{"role":"toolResult","toolCallId":"unknown","timestamp":1_790_762_400_000_i64,"content":[]}});
    let records = parse("pi", &result, &mut state, "result").expect("result");
    assert!(records.iter().any(|record| matches!(
        record,
        Record::ToolResult {
            outcome: Outcome::Unknown,
            occurred_at: Some(1_790_762_400_000),
            ..
        }
    )));
    let nested = json!({"type":"message","id":"nested","message":{"role":"toolResult","toolCallId":"code","isError":false,"nestedCalls":{"complete":false,"calls":[{"id":"omitted","name":"read","argumentsBytes":50000,"status":"unfinished"}]}}});
    assert!(parse("pi", &nested, &mut state, "nested")
        .expect("nested")
        .iter()
        .all(|record| !matches!(record, Record::Activation(_))));
    assert!(state.has_partial_attribution());
}

#[test]
fn pi_rejects_missing_session_identity_and_unsupported_formats() {
    let mut state = ParserState::default();
    assert!(parse(
        "pi",
        &json!({"type":"message","message":{"role":"assistant"}}),
        &mut state,
        "1"
    )
    .is_err());
    assert!(parse(
        "pi",
        &json!({"type":"session","version":3}),
        &mut state,
        "1"
    )
    .is_err());
    assert!(parse(
        "pi",
        &json!({"type":"session","version":99,"id":"unknown"}),
        &mut state,
        "1"
    )
    .is_err());
    assert!(parse(
        "pi",
        &json!({"type":"session","version":3,"id":"weekly"}),
        &mut state,
        "missing-cwd"
    )
    .is_err());
}

#[test]
fn pi_extension_subagent_calls_are_partial_without_invented_worker_links() {
    let mut state = initialized();
    let value = json!({"type":"message","id":"extension-call","message":{"role":"assistant","content":[{"type":"toolCall","id":"spawn","name":"subagent","arguments":{"agent":"invented","task":"read skill instructions"}}]}});
    let records = parse("pi", &value, &mut state, "extension").expect("extension");
    assert!(state.has_partial_attribution());
    assert!(records
        .iter()
        .all(|record| !matches!(record, Record::Link { .. } | Record::Activation(_))));
}

#[test]
fn pi_dedicated_skills_read_preserves_packages_and_excludes_other_resources() {
    let mut state = initialized();
    let value = json!({"type":"message","id":"packages","message":{"role":"assistant","content":[
        {"type":"toolCall","id":"skill","name":"read","namespace":"skills","arguments":{"package":"plugin:skill-name","resource":"skill://plugin:skill-name/SKILL.md"}},
        {"type":"toolCall","id":"resource","name":"read","namespace":"skills","arguments":{"package":"plugin:skill-name","resource":"reference.md"}}
    ]}});
    let records = parse("pi", &value, &mut state, "packages").expect("skills read");
    let events = records
        .iter()
        .filter_map(|record| match record {
            Record::Activation(event) => Some(event),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].reference, "plugin:skill-name");
    assert_eq!(events[0].evidence, Evidence::SkillTool);
}

#[test]
fn pi_forks_keep_exact_entry_ids_and_parent_source_without_inventing_subagents() {
    let mut state = ParserState {
        source_path: Some("/tmp/synthetic-fork.jsonl".into()),
        ..ParserState::default()
    };
    let mut records = Vec::new();
    for (index, line) in include_str!("fixtures/pi-fork.jsonl").lines().enumerate() {
        let value: Value = serde_json::from_str(line).expect("synthetic JSON");
        records.extend(parse("pi", &value, &mut state, &index.to_string()).expect("fork"));
    }
    let session = state.session.expect("session");
    assert_eq!(session.role, WorkerRole::Main);
    assert_eq!(
        session.parent_native_id.as_deref(),
        Some("/tmp/synthetic-parent.jsonl")
    );
    assert!(records.iter().any(|record| matches!(record,Record::SessionPath{path,..} if path == Path::new("/tmp/synthetic-fork.jsonl"))));
    assert!(records.iter().any(|record| matches!(record,Record::SessionEntry{entry_id,inherits_parent:true,occurred_at:Some(1_790_762_403_000),..} if entry_id == "pi-calls")));
    assert!(records.iter().any(|record| matches!(record,Record::SessionEntry{entry_id,inherits_parent:true,..} if entry_id == "pi-fork-new")));
    assert!(records
        .iter()
        .all(|record| !matches!(record, Record::Link { .. })));
}

#[test]
fn pi_fork_entry_timestamp_uses_the_same_verified_inner_fallback_as_activations() {
    let mut state = ParserState::default();
    let mut lines = include_str!("fixtures/pi-fork.jsonl").lines();
    let header: Value = serde_json::from_str(lines.next().expect("header")).expect("JSON");
    parse("pi", &header, &mut state, "header").expect("header");
    let mut copied: Value =
        serde_json::from_str(lines.next().expect("copied entry")).expect("JSON");
    copied.as_object_mut().expect("entry").remove("timestamp");
    let records = parse("pi", &copied, &mut state, "copied").expect("copied entry");
    let activation_time = records.iter().find_map(|record| match record {
        Record::Activation(event) => event.occurred_at,
        _ => None,
    });
    let entry_time = records.iter().find_map(|record| match record {
        Record::SessionEntry { occurred_at, .. } => *occurred_at,
        _ => None,
    });
    assert_eq!(activation_time, Some(1_790_762_403_000));
    assert_eq!(entry_time, activation_time);
}

#[test]
fn pi_custom_session_ids_are_distinct_across_projects_and_stable_across_mirrors() {
    let header = json!({"type":"session","version":3,"id":"weekly","cwd":"/tmp/project-one","timestamp":"2026-09-30T10:00:00Z"});
    let mut first = ParserState::default();
    parse("pi", &header, &mut first, "first").expect("first project");
    let mut other_header = header.clone();
    other_header["cwd"] = json!("/tmp/project-two");
    let mut second = ParserState::default();
    parse("pi", &other_header, &mut second, "second").expect("second project");
    let first_id = first.session.expect("first session").native_id;
    let second_id = second.session.expect("second session").native_id;
    assert_ne!(first_id, second_id);
    let mut mirror = ParserState::default();
    other_header["cwd"] = json!("/tmp/project-one/nested/..");
    parse("pi", &other_header, &mut mirror, "mirror").expect("mirror");
    assert_eq!(mirror.session.expect("mirror session").native_id, first_id);
}
