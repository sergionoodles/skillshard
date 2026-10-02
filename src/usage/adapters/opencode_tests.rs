use super::*;
use crate::usage::WorkerRole;
use serde_json::json;

fn session(parent: Option<&str>) -> Session {
    Session {
        agent: "opencode".into(),
        native_id: if parent.is_some() { "child" } else { "main" }.into(),
        parent_native_id: parent.map(str::to_string),
        role: if parent.is_some() {
            WorkerRole::Subagent
        } else {
            WorkerRole::Main
        },
        project: Some("/project".into()),
        ..Session::default()
    }
}

fn tool(name: &str, input: Value, status: &str, metadata: Value) -> Value {
    json!({"type":"tool","id":"call","name":name,"time":{"created":1000,"ran":1001},"state":{"status":status,"input":input,"metadata":metadata,"content":[{"type":"text","text":"sanitized tool output"}]}})
}

fn parse(session: &Session, content: Vec<Value>) -> Vec<Record> {
    parse_message(
        session,
        "message",
        "assistant",
        &json!({"content":content,"agent":"build","model":{"id":"test-model"}}),
        Some(1000),
        Some(Path::new("/project")),
    )
    .unwrap()
}

#[test]
fn completed_skill_tool_and_child_result_are_attributed() {
    let records = parse(
        &session(Some("main")),
        vec![tool(
            "skill",
            json!({"id":"alpha"}),
            "completed",
            json!({"directory":"/skills/alpha"}),
        )],
    );
    let Record::Activation(event) = &records[0] else {
        panic!("missing activation")
    };
    assert_eq!(event.session.role, WorkerRole::Subagent);
    assert_eq!(event.session.parent_native_id.as_deref(), Some("main"));
    assert_eq!(event.reference, "/skills/alpha/SKILL.md");
    assert_eq!(event.occurred_at, Some(1001));
    assert_eq!(event.outcome, Outcome::Succeeded);
    assert!(
        matches!(&records[1],Record::ToolResult {session_id,outcome:Outcome::Succeeded,..} if session_id=="child")
    );
}

#[test]
fn pending_failed_read_and_shell_exit_remain_separate() {
    let records = parse(
        &session(None),
        vec![
            tool("skill", json!({"id":"alpha"}), "running", json!({})),
            tool(
                "read",
                json!({"path":"/skills/alpha/SKILL.md"}),
                "error",
                json!({}),
            ),
            tool(
                "shell",
                json!({"command":"cat alpha/SKILL.md","workdir":"/nested"}),
                "completed",
                json!({"exit":0}),
            ),
            tool(
                "shell",
                json!({"command":"cat alpha/SKILL.md"}),
                "completed",
                json!({"exit":1}),
            ),
        ],
    );
    let events: Vec<_> = records
        .iter()
        .filter_map(|record| {
            if let Record::Activation(event) = record {
                Some(event)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(events.len(), 4);
    assert_eq!(
        events.iter().map(|event| event.outcome).collect::<Vec<_>>(),
        vec![
            Outcome::Unknown,
            Outcome::Failed,
            Outcome::Succeeded,
            Outcome::Failed
        ]
    );
    assert_eq!(
        events[2].working_directory.as_deref(),
        Some(Path::new("/nested"))
    );
    assert_ne!(events[0].native_id, events[1].native_id);
}

#[test]
fn attachments_require_recorded_delivery_and_mentions_do_not_count() {
    let records = parse_message(&session(None),"user","user",&json!({"text":"Mention alpha/SKILL.md","skills":[{"id":"alpha","name":"Alpha","text":"delivered instructions"},{"id":"catalog-only","name":"Catalog"}],"files":[{"data":"c2FuaXRpemVk","source":{"type":"uri","uri":"file:///skills/beta/SKILL.md"}}]}),Some(1000),None).unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| matches!(record,Record::Activation(event) if event.evidence==Evidence::Attachment && event.outcome==Outcome::Succeeded)));
    assert!(parse(
        &session(None),
        vec![
            json!({"type":"text","text":"read alpha/SKILL.md"}),
            tool(
                "edit",
                json!({"path":"/skills/alpha/SKILL.md"}),
                "completed",
                json!({})
            )
        ]
    )
    .is_empty());
}

#[test]
fn listings_background_shells_and_unknown_shapes_are_not_confirmed() {
    let mut listing = tool(
        "read",
        json!({"path":"/directory/SKILL.md"}),
        "completed",
        json!({}),
    );
    listing["state"]["content"] =
        json!([{"type":"text","text":"Read directory /directory/SKILL.md, 1 entries"}]);
    assert!(parse(&session(None), vec![listing]).is_empty());
    let records = parse(
        &session(None),
        vec![tool(
            "shell",
            json!({"command":"cat alpha/SKILL.md"}),
            "completed",
            json!({"status":"running"}),
        )],
    );
    assert!(matches!(&records[0],Record::Activation(event) if event.outcome==Outcome::Unknown));
    assert!(parse_message(
        &session(None),
        "message",
        "unknown",
        &json!({}),
        Some(1000),
        None
    )
    .is_err());
}

#[test]
fn long_valid_assistant_keeps_skill_and_file_reads_after_non_tool_parts() {
    let mut content = vec![json!({"type":"text","text":"sanitized text"}); 129];
    content.push(tool(
        "skill",
        json!({"id":"alpha"}),
        "completed",
        json!({"directory":"/skills/alpha"}),
    ));
    content.push(tool(
        "read",
        json!({"path":"/skills/alpha/SKILL.md"}),
        "completed",
        json!({}),
    ));
    let records = parse(&session(None), content);
    assert_eq!(records.iter().filter(|record|matches!(record,Record::Activation(event) if event.outcome==Outcome::Succeeded)).count(),2);
}
