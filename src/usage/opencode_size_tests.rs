use super::*;

#[test]
fn large_valid_assistant_preserves_skill_events_with_bounded_batches() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    let mut message = tool("completed");
    message["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"text","text":"x".repeat(1024 * 1024)}));
    fixture.message("large-message", "main", 1, 2000, "assistant", message);
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 1);
    let checkpoint = database
        .source("opencode", &fixture.path.to_string_lossy())
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.diagnostics, 0);
    assert!(checkpoint.parser_state.len() < 4096);
    import_all(&mut database, &fixture, 0);
    assert_eq!(counts(&database).activations, 1);
}
