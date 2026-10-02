use super::*;

fn read_tool() -> serde_json::Value {
    let mut read = tool("completed");
    read["content"][0]["name"] = json!("read");
    read["content"][0]["state"]["input"] = json!({"path":"alpha/SKILL.md"});
    read
}

#[test]
fn native_subpath_metadata_is_not_appended_to_actual_read_directory() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    Connection::open(&fixture.path)
        .unwrap()
        .execute(
            "UPDATE session_v2 SET directory='/skills',path='skills'",
            [],
        )
        .unwrap();
    fixture.message("read", "main", 1, 2000, "assistant", read_tool());
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    let snapshot = database.snapshot(&Query::default(), 0, NOW).unwrap();
    assert_eq!(snapshot.unresolved, 0);
    assert_eq!(snapshot.rankings.len(), 1);
    assert_eq!(snapshot.rankings[0].counts.activations, 1);
}

#[test]
fn historical_subpath_metadata_does_not_change_location_read_directory() {
    let fixture = Fixture::new();
    fixture.session("main", None, None);
    fixture.message(
        "move",
        "main",
        1,
        2000,
        "location-switched",
        json!({"location":{"directory":"/skills"},"subpath":"skills"}),
    );
    fixture.message("read", "main", 2, 2001, "assistant", read_tool());
    let mut database = database();
    import_all(&mut database, &fixture, 0);
    let snapshot = database.snapshot(&Query::default(), 0, NOW).unwrap();
    assert_eq!(snapshot.unresolved, 0);
    assert_eq!(snapshot.rankings.len(), 1);
    assert_eq!(snapshot.rankings[0].counts.activations, 1);
}
