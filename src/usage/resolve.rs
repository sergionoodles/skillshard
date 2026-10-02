use crate::model::{AgentInstall, InstallKind, Scope, Skill};
use crate::usage::UsageResult;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

pub(super) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if normalized.file_name().is_some() => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub(super) fn skill_identity(skill: &Skill, scope: &Scope) -> UsageResult<String> {
    let scope = match scope {
        Scope::Global => "global".to_string(),
        Scope::Project(path) => normalize_path(path).to_string_lossy().into_owned(),
    };
    serde_json::to_string(&(
        scope,
        &skill.name,
        skill
            .lock
            .as_ref()
            .map(|lock| (&lock.source, &lock.skill_path)),
    ))
    .map_err(|error| error.to_string())
}

fn insert_alias(
    connection: &Connection,
    skill_id: i64,
    reference: &str,
    kind: &str,
    agent: &str,
    project: &str,
    active: bool,
) -> UsageResult<()> {
    connection.execute("INSERT INTO skill_aliases(skill_id,reference,kind,agent,project,active) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(skill_id,reference,kind,agent,project) DO UPDATE SET active=excluded.active", params![skill_id,reference,kind,agent,project,active]).map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn insert_path(
    connection: &Connection,
    skill_id: i64,
    path: &Path,
    agent: &str,
    project: &str,
) -> UsageResult<()> {
    let file = if path.file_name().is_some_and(|name| name == "SKILL.md") {
        path.to_path_buf()
    } else {
        path.join("SKILL.md")
    };
    let lexical = normalize_path(&file).to_string_lossy().into_owned();
    insert_alias(connection, skill_id, &lexical, "path", agent, project, true)?;
    if let Ok(resolved) = file.canonicalize() {
        insert_alias(
            connection,
            skill_id,
            &resolved.to_string_lossy(),
            "path",
            agent,
            project,
            true,
        )?;
    }
    Ok(())
}

pub(super) fn register_skills(connection: &Connection, skills: &[Skill]) -> UsageResult<()> {
    connection
        .execute("UPDATE skills SET installed=0", [])
        .map_err(|error| error.to_string())?;
    connection
        .execute("UPDATE skill_aliases SET active=0 WHERE kind='name'", [])
        .map_err(|error| error.to_string())?;
    for skill in skills {
        register_skill(connection, skill)?;
    }
    reconcile_references(connection, false)
}

pub(super) fn register_skill(connection: &Connection, skill: &Skill) -> UsageResult<()> {
    let primary_id = register_identity(connection, skill, &skill_identity(skill, &skill.scope)?)?;
    let project = match &skill.scope {
        Scope::Global => String::new(),
        Scope::Project(path) => normalize_path(path).to_string_lossy().into_owned(),
    };
    if let Some(path) = &skill.canonical {
        insert_path(connection, primary_id, path, "", &project)?;
    }
    for install in &skill.installs {
        let identity = install_identity(skill, install)?;
        let skill_id = register_identity(connection, skill, &identity)?;
        insert_path(
            connection,
            skill_id,
            &install.path,
            install.agent.key,
            &project,
        )?;
        insert_alias(
            connection,
            skill_id,
            &skill.name,
            "name",
            install.agent.key,
            &project,
            !skill.disabled,
        )?;
    }
    Ok(())
}

pub(super) fn installed_identities(skill: &Skill) -> UsageResult<Vec<String>> {
    let mut identities = Vec::new();
    if skill.canonical.is_some() || skill.installs.is_empty() {
        identities.push(skill_identity(skill, &skill.scope)?);
    }
    for install in &skill.installs {
        let identity = install_identity(skill, install)?;
        if !identities.contains(&identity) {
            identities.push(identity);
        }
    }
    Ok(identities)
}

pub(super) fn install_identity(skill: &Skill, install: &AgentInstall) -> UsageResult<String> {
    let independent_copy = skill.installs.iter().find(|copy| {
        matches!(copy.kind, InstallKind::Copy)
            && skill
                .canonical
                .as_ref()
                .is_none_or(|canonical| normalize_path(canonical) != normalize_path(&copy.path))
            && normalize_path(&copy.path) == normalize_path(&install.path)
    });
    let base = skill_identity(skill, &skill.scope)?;
    let Some(copy) = independent_copy else {
        return Ok(base);
    };
    serde_json::to_string(&(base, copy.agent.key, normalize_path(&copy.path)))
        .map_err(|error| error.to_string())
}

fn register_identity(connection: &Connection, skill: &Skill, identity: &str) -> UsageResult<i64> {
    let existing: Option<i64> = connection
        .query_row(
            "SELECT skill_id FROM skill_identities WHERE identity=?1",
            [&identity],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let skill_id = if let Some(id) = existing {
        connection
            .execute(
                "UPDATE skills SET name=?1,installed=?2 WHERE id=?3",
                params![skill.display_name(), !skill.disabled, id],
            )
            .map_err(|error| error.to_string())?;
        id
    } else {
        connection
            .execute(
                "INSERT INTO skills(identity,name,installed) VALUES(?1,?2,?3)",
                params![identity, skill.display_name(), !skill.disabled],
            )
            .map_err(|error| error.to_string())?;
        let id = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO skill_identities(identity,skill_id) VALUES(?1,?2)",
                params![identity, id],
            )
            .map_err(|error| error.to_string())?;
        id
    };
    Ok(skill_id)
}

pub(super) fn resolve_reference(
    connection: &Connection,
    reference: &str,
    agent: &str,
    project: Option<&Path>,
    working_directory: Option<&Path>,
) -> UsageResult<Option<i64>> {
    let project = project.map(normalize_path);
    let is_path =
        reference.contains('/') || reference.contains('\\') || reference.ends_with("SKILL.md");
    if !is_path {
        return lookup_alias(connection, reference, "name", agent, project.as_deref());
    }
    let path = Path::new(reference);
    let path = if path.is_absolute() {
        normalize_path(path)
    } else if let Some(directory) = working_directory.or(project.as_deref()) {
        normalize_path(&directory.join(path))
    } else {
        return Ok(None);
    };
    let lexical = path.to_string_lossy();
    let matched = lookup_alias(connection, &lexical, "path", agent, project.as_deref())?;
    if matched.is_some() {
        return Ok(matched);
    }
    let Ok(resolved) = path.canonicalize() else {
        return Ok(None);
    };
    lookup_alias(
        connection,
        &resolved.to_string_lossy(),
        "path",
        agent,
        project.as_deref(),
    )
}

fn lookup_alias(
    connection: &Connection,
    reference: &str,
    kind: &str,
    agent: &str,
    project: Option<&Path>,
) -> UsageResult<Option<i64>> {
    let project = project
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut statement = connection.prepare("SELECT DISTINCT skill_id FROM skill_aliases WHERE reference=?1 AND kind=?2 AND (agent='' OR agent=?3) AND (project='' OR project=?4) AND (kind='path' OR active=1) LIMIT 2").map_err(|error| error.to_string())?;
    let matches = statement
        .query_map(params![reference, kind, agent, project], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(if matches.len() == 1 {
        matches.first().copied()
    } else {
        None
    })
}

pub(super) fn reconcile_unresolved(connection: &Connection) -> UsageResult<()> {
    reconcile_references(connection, true)
}

fn reconcile_references(connection: &Connection, touched_only: bool) -> UsageResult<()> {
    const BATCH_SIZE: i64 = 256;
    let mut last_id = 0;
    loop {
        let condition = if touched_only {
            " AND e.skill_id IS NULL AND e.session_id IN (SELECT id FROM touched_sessions)"
        } else {
            " AND e.skill_id IS NULL"
        };
        let sql = format!("SELECT e.id,e.reference,s.agent,p.path,e.cwd FROM activation_events e JOIN sessions s ON s.id=e.session_id LEFT JOIN projects p ON p.id=s.project_id WHERE e.id>?1{condition} ORDER BY e.id LIMIT ?2");
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let references = statement
            .query_map(params![last_id, BATCH_SIZE], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if references.is_empty() {
            return Ok(());
        }
        for (id, reference, agent, project, working_directory) in references {
            let skill_id = resolve_reference(
                connection,
                &reference,
                &agent,
                project.as_deref().map(Path::new),
                working_directory.as_deref().map(Path::new),
            )?;
            connection
                .execute(
                    "UPDATE activation_events SET skill_id=?1 WHERE id=?2",
                    params![skill_id, id],
                )
                .map_err(|error| error.to_string())?;
            last_id = id;
        }
    }
}

pub(super) fn reconcile_sessions(connection: &Connection) -> UsageResult<bool> {
    const PARENT_LOOKUP: &str = "COALESCE((SELECT parent.id FROM sessions parent WHERE parent.agent=sessions.agent AND parent.native_id=sessions.parent_native_id),(SELECT MIN(session_id) FROM session_paths WHERE agent=sessions.agent AND path=sessions.parent_native_id HAVING COUNT(DISTINCT session_id)=1))";
    let changed = connection.execute(&format!("UPDATE sessions SET parent_id={PARENT_LOOKUP} WHERE parent_id IS NOT {PARENT_LOOKUP}"), []).map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare("SELECT id,parent_id,parent_native_id,role FROM sessions")
        .map_err(|error| error.to_string())?;
    let sessions = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                (
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?.is_some(),
                    row.get::<_, String>(3)? == "unknown",
                ),
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|error| error.to_string())?;
    let mut roots = HashMap::new();
    for id in sessions.keys() {
        let (root, provisional) = resolve_root_and_cache(*id, &sessions, &mut roots);
        let ancestry_cycle = roots.get(id).is_some_and(Option::is_none);
        connection
            .execute(
                "UPDATE sessions SET root_id=?1,provisional=?2,ancestry_cycle=?3 WHERE id=?4",
                params![root, provisional, ancestry_cycle, id],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(changed > 0)
}

fn resolve_root_and_cache(
    id: i64,
    sessions: &HashMap<i64, (Option<i64>, bool, bool)>,
    roots: &mut HashMap<i64, Option<(i64, bool)>>,
) -> (i64, bool) {
    let mut current = id;
    let mut visited = HashSet::new();
    loop {
        if let Some(cached) = roots.get(&current).copied() {
            for visited_id in visited {
                roots.insert(visited_id, cached);
            }
            return cached.unwrap_or((id, true));
        }
        if !visited.insert(current) {
            // A cycle has no defensible conversation root. Keep each session
            // provisional rather than choosing a root from import ordering.
            for visited_id in visited {
                roots.insert(visited_id, None);
            }
            return (id, true);
        }
        let Some((parent, has_parent, is_unknown)) = sessions.get(&current) else {
            return (id, true);
        };
        if let Some(parent) = parent {
            current = *parent;
            continue;
        }
        let root = (current, *has_parent || *is_unknown);
        for visited_id in visited {
            roots.insert(visited_id, Some(root));
        }
        return root;
    }
}
