//! Alias changes backed by successful explicit filesystem operations.
use super::resolve;
use crate::model::{Scope, Skill};
use crate::usage::UsageResult;
use rusqlite::{params, Connection};
use std::path::Path;

pub(super) fn record_move(
    connection: &Connection,
    skill: &Skill,
    destination: &Scope,
) -> UsageResult<()> {
    let source_id = content_identity(connection, skill)?;
    let destination_identity = resolve::skill_identity(skill, destination)?;
    map_identity(connection, &destination_identity, source_id)?;
    let project = project_path(destination);
    resolve::insert_path(
        connection,
        source_id,
        &destination.canonical_dir().join(&skill.name),
        "",
        &project,
    )?;
    for install in &skill.installs {
        if let Some(directory) = destination.agent_dir(install.agent) {
            resolve::insert_path(
                connection,
                source_id,
                &directory.join(&skill.name),
                install.agent.key,
                &project,
            )?;
        }
    }
    Ok(())
}

pub(super) fn record_toggle(
    connection: &Connection,
    skill: &Skill,
    disabled: bool,
) -> UsageResult<()> {
    let source_id = content_identity(connection, skill)?;
    map_identity(
        connection,
        &resolve::skill_identity(skill, &skill.scope)?,
        source_id,
    )?;
    let directory = if disabled {
        skill.scope.disabled_dir()
    } else {
        skill.scope.canonical_dir()
    };
    resolve::insert_path(
        connection,
        source_id,
        &directory.join(&skill.name),
        "",
        &project_path(&skill.scope),
    )
}

fn content_identity(connection: &Connection, skill: &Skill) -> UsageResult<i64> {
    resolve::register_skill(connection, skill)?;
    let path = skill
        .content_path()
        .ok_or_else(|| "Cannot record relocation of a skill with no content path".to_string())?;
    let identity = if skill
        .canonical
        .as_deref()
        .is_some_and(|canonical| same_path(canonical, path))
    {
        resolve::skill_identity(skill, &skill.scope)?
    } else if let Some(install) = skill
        .installs
        .iter()
        .find(|install| same_path(&install.path, path))
    {
        resolve::install_identity(skill, install)?
    } else {
        return Err("Cannot identify the skill content being relocated".into());
    };
    connection
        .query_row(
            "SELECT skill_id FROM skill_identities WHERE identity=?1",
            [identity],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn same_path(left: &Path, right: &Path) -> bool {
    resolve::normalize_path(left) == resolve::normalize_path(right)
}

fn project_path(scope: &Scope) -> String {
    match scope {
        Scope::Global => String::new(),
        Scope::Project(path) => resolve::normalize_path(path).to_string_lossy().into_owned(),
    }
}

fn map_identity(connection: &Connection, identity: &str, skill_id: i64) -> UsageResult<()> {
    connection.execute("INSERT INTO skill_identities(identity,skill_id) VALUES(?1,?2) ON CONFLICT(identity) DO UPDATE SET skill_id=excluded.skill_id", params![identity,skill_id]).map_err(|error| error.to_string())?;
    Ok(())
}
