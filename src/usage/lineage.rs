//! Exact native entry identities distinguish copied fork context from new loads.

use crate::usage::UsageResult;
use rusqlite::Connection;

pub(super) fn reconcile_inherited_entries(connection: &Connection, full: bool) -> UsageResult<()> {
    let selection = if full {
        ""
    } else {
        "WHERE entry_id IN (SELECT entry_id FROM touched_entries)"
    };
    connection.execute_batch("CREATE TEMP TABLE IF NOT EXISTS entry_locators(source_id INTEGER,generation INTEGER,locator TEXT,inherited INTEGER,PRIMARY KEY(source_id,generation,locator)); DELETE FROM entry_locators;").map_err(|error| error.to_string())?;
    connection.execute(
        &format!("INSERT INTO entry_locators SELECT child.source_id,child.generation,child.locator,child.inherits_parent AND session.ancestry_cycle=0 AND EXISTS(SELECT 1 FROM session_entries parent WHERE parent.session_id=session.parent_id AND parent.entry_id=child.entry_id) FROM session_entries child JOIN sessions session ON session.id=child.session_id {selection}"),
        [],
    ).map_err(|error| error.to_string())?;
    connection.execute("INSERT OR IGNORE INTO touched_events SELECT event_id FROM event_observations JOIN entry_locators USING(source_id,generation,locator)", []).map_err(|error| error.to_string())?;
    connection.execute("INSERT OR IGNORE INTO touched_events SELECT event.id FROM activation_events event JOIN tool_results result ON result.session_id=event.session_id AND result.worker_id=event.worker_id AND result.native_id=event.native_id JOIN entry_locators location ON location.source_id=result.source_id AND location.generation=result.generation AND location.locator=result.locator", []).map_err(|error| error.to_string())?;
    for table in ["event_observations", "tool_results"] {
        connection.execute(
            &format!("UPDATE {table} SET inherited=(SELECT inherited FROM entry_locators location WHERE location.source_id={table}.source_id AND location.generation={table}.generation AND location.locator={table}.locator) WHERE (source_id,generation,locator) IN (SELECT source_id,generation,locator FROM entry_locators)"),
            [],
        ).map_err(|error| error.to_string())?;
    }
    connection.execute("UPDATE activation_events SET inherited=NOT EXISTS(SELECT 1 FROM event_observations observation WHERE observation.event_id=activation_events.id AND observation.inherited=0) WHERE id IN (SELECT id FROM touched_events)", []).map_err(|error| error.to_string())?;
    Ok(())
}
