use serde_json::{json, Value};

use crate::usage::adapters::{parse, ParserState};
use crate::usage::{Evidence, Outcome, Record};

fn header() -> Value {
    json!({"type":"session_meta","timestamp":"2026-10-01T10:00:00Z","ordinal":0,
        "payload":{"id":"synthetic-main","session_id":"synthetic-main","source":"cli","cwd":"/tmp/synthetic-project","history_mode":"paginated"}})
}

fn read_call(ordinal: u64) -> Value {
    json!({"type":"response_item","timestamp":"2026-10-01T10:00:01Z","ordinal":ordinal,
        "payload":{"type":"function_call","call_id":"synthetic-read","name":"exec_command",
        "arguments":"{\"cmd\":\"cat /tmp/example/SKILL.md\"}"}})
}

#[test]
fn ordinary_codex_fork_uses_its_logical_boundary_instead_of_discarding_new_reads() {
    let mut state = ParserState::default();
    let mut fork = header();
    fork["payload"]["forked_from_id"] = json!("synthetic-parent");
    fork["payload"]["forked_from_ordinal_exclusive"] = json!(43);
    fork["payload"]["history_base"] =
        json!({"thread_id":"synthetic-parent","end_ordinal_exclusive":20,"end_byte_offset":4096});
    parse("codex", &fork, &mut state, "header").expect("fork header");
    assert!(parse("codex", &read_call(42), &mut state, "copied")
        .expect("copied")
        .is_empty());
    let records = parse("codex", &read_call(43), &mut state, "new").expect("new read");
    assert!(records.iter().any(|record| matches!(record,Record::Activation(event) if event.reference=="/tmp/example/SKILL.md")));
    assert!(!state.has_partial_attribution());
}

#[test]
fn durable_codex_command_execution_counts_code_mode_read_and_explicit_result() {
    let mut state = ParserState::default();
    parse("codex", &header(), &mut state, "header").expect("header");
    let completed = json!({"type":"event_msg","timestamp":"2026-10-01T10:00:01Z","ordinal":1,
        "payload":{"type":"item_completed","thread_id":"synthetic-main","turn_id":"synthetic-turn",
        "item":{"type":"CommandExecution","id":"synthetic-read","command":["/bin/bash","-lc","cat /tmp/example/SKILL.md"],
        "cwd":"file:///tmp/synthetic-project","parsed_cmd":[],"source":"agent","status":"completed","exit_code":0}}});
    let records = parse("codex", &completed, &mut state, "completed").expect("durable command");
    assert!(records.iter().any(|record| matches!(record,Record::Activation(event) if event.native_id=="synthetic-read" && event.evidence==Evidence::ShellRead)));
    assert!(records.iter().any(|record| matches!(record,Record::ToolResult{call_id,outcome:Outcome::Succeeded,..} if call_id=="synthetic-read")));
}

#[test]
fn unrelated_codex_function_arguments_do_not_mark_a_valid_transcript_malformed() {
    let mut state = ParserState::default();
    parse("codex", &header(), &mut state, "header").expect("header");
    let unrelated = json!({"type":"response_item","timestamp":"2026-10-01T10:00:01Z","ordinal":1,
        "payload":{"type":"function_call","call_id":"unrelated","name":"update_plan","arguments":"not encoded JSON"}});
    assert!(parse("codex", &unrelated, &mut state, "unrelated")
        .expect("irrelevant call")
        .is_empty());
    assert!(!state.has_partial_attribution());
}

#[test]
fn durable_codex_spawn_links_native_parent_and_worker_ids() {
    let mut state = ParserState::default();
    parse("codex", &header(), &mut state, "header").expect("header");
    let completed = json!({"type":"event_msg","timestamp":"2026-10-01T10:00:01Z","ordinal":1,
        "payload":{"type":"item_completed","thread_id":"synthetic-main","turn_id":"synthetic-turn",
        "item":{"type":"CollabAgentToolCall","id":"synthetic-spawn","tool":"spawn_agent","status":"completed",
        "sender_thread_id":"synthetic-main","receiver_thread_ids":["synthetic-child"],
        "receiver_agents":[{"thread_id":"synthetic-child","agent_nickname":"synthetic-worker"}],"agents_states":{},"model":"synthetic-model"}}});
    let records = parse("codex", &completed, &mut state, "spawn").expect("spawn");
    assert!(records.iter().any(|record| matches!(record,Record::Link{parent_id,child_id,..} if parent_id=="synthetic-main" && child_id=="synthetic-child")));
    assert!(records.iter().any(|record| matches!(record,Record::Session(session) if session.native_id=="synthetic-child" && session.worker_name.as_deref()==Some("synthetic-worker"))));
    assert_eq!(
        records.len(),
        2,
        "receiver metadata and ids describe one worker"
    );
}

fn completed_command(command: Value, status: &str, exit_code: Value) -> Value {
    json!({"type":"event_msg","timestamp":"2026-10-01T10:00:01Z","ordinal":1,
        "payload":{"type":"item_completed","thread_id":"synthetic-main","turn_id":"synthetic-turn",
        "item":{"type":"CommandExecution","id":"synthetic-read","command":command,
        "cwd":"file:///tmp/command-directory","parsed_cmd":[],"source":"unified_exec_startup",
        "status":status,"exit_code":exit_code}}})
}

#[test]
fn durable_commands_preserve_request_identity_across_mirrors_and_working_directories() {
    let mut state = ParserState::default();
    parse("codex", &header(), &mut state, "header").expect("header");
    let request = parse("codex", &read_call(1), &mut state, "request").expect("request");
    let completed = completed_command(
        json!(["cat", "/tmp/example/SKILL.md"]),
        "completed",
        json!(0),
    );
    let records = parse("codex", &completed, &mut state, "completed").expect("completed");
    let mut mirror_state = ParserState::default();
    parse("codex", &header(), &mut mirror_state, "mirror-header").expect("mirror header");
    let mirror = parse("codex", &completed, &mut mirror_state, "mirror-record").expect("mirror");
    let identities = |records: &[Record]| {
        records
            .iter()
            .filter_map(|record| match record {
                Record::Activation(event) => Some((
                    event.session.native_id.clone(),
                    event.native_id.clone(),
                    event.reference.clone(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(identities(&request), identities(&records));
    assert_eq!(identities(&records), identities(&mirror));
    assert!(
        records
            .iter()
            .any(|record| matches!(record, Record::Activation(event)
        if event.working_directory.as_deref()==Some(std::path::Path::new("/tmp/command-directory"))
        && event.session.project.as_deref()==Some(std::path::Path::new("/tmp/synthetic-project"))))
    );
}

#[test]
fn durable_command_failures_and_missing_exit_codes_keep_explicit_outcomes() {
    for (status, exit_code, expected) in [
        ("completed", json!(1), Outcome::Failed),
        ("failed", json!(0), Outcome::Failed),
        ("declined", Value::Null, Outcome::Failed),
        ("completed", Value::Null, Outcome::Unknown),
    ] {
        let mut state = ParserState::default();
        parse("codex", &header(), &mut state, "header").expect("header");
        let completed =
            completed_command(json!(["cat", "/tmp/example/SKILL.md"]), status, exit_code);
        let records = parse("codex", &completed, &mut state, "command").expect("command");
        assert!(records.iter().any(
            |record| matches!(record, Record::Activation(event) if event.outcome==Outcome::Unknown)
        ));
        assert!(records
            .iter()
            .any(|record| matches!(record, Record::ToolResult{outcome,..} if *outcome==expected)));
    }
}

#[test]
fn durable_commands_do_not_infer_reads_from_mentions_outputs_or_manual_shell() {
    for command in [
        json!(["ls", "/tmp/example/SKILL.md"]),
        json!(["echo", "/tmp/example/SKILL.md"]),
        json!(["sed", "-i", "s/a/b/", "/tmp/example/SKILL.md"]),
        json!(["bash", "-lc", "cat /tmp/example/SKILL.md | wc -l"]),
    ] {
        let mut state = ParserState::default();
        parse("codex", &header(), &mut state, "header").expect("header");
        let mut completed = completed_command(command, "completed", json!(0));
        completed["payload"]["item"]["stdout"] = json!("cat /tmp/example/SKILL.md");
        let records = parse("codex", &completed, &mut state, "command").expect("command");
        assert!(!records
            .iter()
            .any(|record| matches!(record, Record::Activation(_))));
    }
    for source in ["user_shell", "unified_exec_interaction"] {
        let mut state = ParserState::default();
        parse("codex", &header(), &mut state, "header").expect("header");
        let mut completed = completed_command(
            json!(["cat", "/tmp/example/SKILL.md"]),
            "completed",
            json!(0),
        );
        completed["payload"]["item"]["source"] = json!(source);
        assert!(parse("codex", &completed, &mut state, "manual")
            .expect("manual")
            .is_empty());
    }
}

#[test]
fn durable_function_output_joins_request_without_inventing_input_evidence() {
    let mut state = ParserState::default();
    parse("codex", &header(), &mut state, "header").expect("header");
    parse("codex", &read_call(1), &mut state, "request").expect("request");
    let output = json!({"type":"event_msg","timestamp":"2026-10-01T10:00:01Z","ordinal":2,
        "payload":{"type":"item_completed","item":{"type":"FunctionCallOutput","id":"synthetic-read",
        "name":"exec_command","namespace":"functions","output":"Process exited with code 0\nOutput:\ncat /tmp/example/SKILL.md"}}});
    let records = parse("codex", &output, &mut state, "output").expect("output");
    assert_eq!(records.len(), 1);
    assert!(
        matches!(&records[0], Record::ToolResult{call_id,outcome:Outcome::Succeeded,..} if call_id=="synthetic-read")
    );
    let replay = parse("codex", &output, &mut state, "mirror-output").expect("mirror output");
    assert!(!replay
        .iter()
        .any(|record| matches!(record, Record::Activation(_))));
}

#[test]
fn durable_spawn_does_not_link_failed_or_non_spawn_tools() {
    for (tool, status) in [
        ("spawn_agent", "failed"),
        ("send_input", "completed"),
        ("spawn_agent", "in_progress"),
    ] {
        let mut state = ParserState::default();
        parse("codex", &header(), &mut state, "header").expect("header");
        let completed = json!({"type":"event_msg","timestamp":"2026-10-01T10:00:01Z","ordinal":1,
            "payload":{"type":"item_completed","item":{"type":"CollabAgentToolCall","id":"synthetic-spawn",
            "tool":tool,"status":status,"sender_thread_id":"synthetic-main","receiver_thread_ids":["synthetic-child"]}}});
        assert!(parse("codex", &completed, &mut state, "spawn")
            .expect("spawn")
            .is_empty());
    }
}

#[test]
fn worker_boundary_excludes_durable_copied_reads_and_unknown_forks_stay_partial() {
    let mut state = ParserState::default();
    let mut worker = header();
    worker["payload"]["forked_from_id"] = json!("synthetic-parent");
    worker["payload"]["forked_from_ordinal_exclusive"] = json!(1);
    worker["payload"]["subagent_history_start_ordinal"] = json!(2);
    parse("codex", &worker, &mut state, "header").expect("worker header");
    let mut completed = completed_command(
        json!(["cat", "/tmp/example/SKILL.md"]),
        "completed",
        json!(0),
    );
    assert!(parse("codex", &completed, &mut state, "copied")
        .expect("copied")
        .is_empty());
    completed["ordinal"] = json!(2);
    assert!(parse("codex", &completed, &mut state, "own")
        .expect("own")
        .iter()
        .any(|record| matches!(record, Record::Activation(_))));
    worker["payload"]
        .as_object_mut()
        .expect("metadata")
        .remove("subagent_history_start_ordinal");
    worker["payload"]
        .as_object_mut()
        .expect("metadata")
        .remove("forked_from_ordinal_exclusive");
    parse("codex", &worker, &mut state, "unknown-fork").expect("unknown fork");
    assert!(state.has_partial_attribution());
    assert!(parse("codex", &completed, &mut state, "unknown")
        .expect("unknown")
        .is_empty());
}
