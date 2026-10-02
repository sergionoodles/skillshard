use super::*;

const COPIED_ID: &str = "msg_000000000001AbCdEf01234567_1";
const NEW_SKILL_ID: &str = "msg_000000000002AbCdEf01234567";
const NEW_READ_ID: &str = "msg_000000000003AbCdEf01234567";

#[test]
fn orphan_fork_counts_new_native_calls_and_excludes_copied_prefix() {
    let fixture = Fixture::new();
    fixture.session(
        "fork",
        None,
        Some((
            "deleted-parent",
            json!({"type":"after","messageID":"deleted-boundary"}),
        )),
    );
    fixture.message(COPIED_ID, "fork", 1, 2000, "assistant", tool("completed"));
    fixture.message(
        NEW_SKILL_ID,
        "fork",
        2,
        2001,
        "assistant",
        tool("completed"),
    );
    let mut read = tool("completed");
    read["content"][0]["name"] = json!("read");
    read["content"][0]["state"]["input"] = json!({"path":"/skills/alpha/SKILL.md"});
    fixture.message(NEW_READ_ID, "fork", 3, 2002, "assistant", read);
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 2);
    assert_eq!(
        database
            .source("opencode", &fixture.path.to_string_lossy())
            .unwrap()
            .unwrap()
            .status,
        "partial_attribution"
    );
}

#[test]
fn missing_reverted_parent_boundary_does_not_remove_new_fork_usage() {
    let fixture = Fixture::new();
    fixture.session("parent", None, None);
    fixture.session(
        "fork",
        None,
        Some((
            "parent",
            json!({"type":"after","messageID":"reverted-boundary"}),
        )),
    );
    fixture.message(COPIED_ID, "fork", 1, 2000, "assistant", tool("completed"));
    fixture.message(
        NEW_SKILL_ID,
        "fork",
        2,
        2001,
        "assistant",
        tool("completed"),
    );
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 1);
}

#[test]
fn ambiguous_fork_message_identity_is_not_guessed_from_timestamps() {
    let fixture = Fixture::new();
    fixture.session(
        "fork",
        None,
        Some((
            "missing-parent",
            json!({"type":"after","messageID":"missing-boundary"}),
        )),
    );
    fixture.message(
        "msg_000000000001AbCdEf01234567_7",
        "fork",
        6,
        9000,
        "assistant",
        tool("completed"),
    );
    fixture.message(
        "unrecognized-id",
        "fork",
        7,
        9001,
        "assistant",
        tool("completed"),
    );
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 0);
    assert_eq!(
        database
            .source("opencode", &fixture.path.to_string_lossy())
            .unwrap()
            .unwrap()
            .status,
        "partial_attribution"
    );
}

#[test]
fn changed_partial_source_recovers_old_calls_when_fork_boundary_appears() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    fixture.session(
        "fork",
        None,
        Some(("main", json!({"type":"after","messageID":"boundary"}))),
    );
    fixture.message("old-child", "fork", 2, 2000, "assistant", tool("completed"));
    fixture.message(
        "recent",
        "main",
        3,
        90_000,
        "user",
        json!({"text":"mention"}),
    );
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 0);
    assert_eq!(
        database
            .source("opencode", &fixture.path.to_string_lossy())
            .unwrap()
            .unwrap()
            .status,
        "partial_attribution"
    );
    fixture.message(
        "boundary",
        "main",
        1,
        1000,
        "user",
        json!({"text":"historical"}),
    );
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 1);
    assert_eq!(
        database
            .source("opencode", &fixture.path.to_string_lossy())
            .unwrap()
            .unwrap()
            .status,
        "supported"
    );
}
