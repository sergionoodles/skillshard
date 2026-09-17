//! Discovers which skills exist on disk and which agents load them.
//!
//! The filesystem is the source of truth. The scanner walks the canonical
//! `.agents/skills` tree, Skillshard's disabled store, and every agent's own
//! skills directory, then joins the result with the CLI's lock file.

use crate::agents::{Agent, AGENTS};
use crate::frontmatter;
use crate::lock;
use crate::model::{AgentInstall, InstallKind, LockEntry, Scope, Skill, UpdateState};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A directory entry that looks like a skill.
struct Found {
    name: String,
    path: PathBuf,
}

/// List the skill directories directly inside `dir`.
///
/// A skill directory is one containing a `SKILL.md`. Hidden entries are
/// skipped: agents keep private material like `.system` alongside skills.
fn read_skill_dirs(dir: &Path) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // Follows symlinks, so a link into the canonical copy still resolves.
        if path.join("SKILL.md").is_file() {
            found.push(Found { name, path });
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Read the front matter of a skill directory.
fn read_front_matter(path: &Path) -> frontmatter::FrontMatter {
    std::fs::read_to_string(path.join("SKILL.md"))
        .map(|text| frontmatter::parse(&text))
        .unwrap_or_default()
}

/// Classify how a skill appears inside one agent's directory.
fn classify(path: &Path, canonical_dir: &Path, agent_dir: &Path) -> InstallKind {
    if agent_dir == canonical_dir {
        return InstallKind::Canonical;
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(path).unwrap_or_default();
            // Links the CLI writes are relative to the agent directory.
            let absolute = if target.is_absolute() {
                target.clone()
            } else {
                agent_dir.join(&target)
            };
            if absolute.exists() {
                InstallKind::Symlink { target: absolute }
            } else {
                InstallKind::Broken { target: absolute }
            }
        }
        _ => InstallKind::Copy,
    }
}

/// Agents that can hold skills in `scope`, paired with their directory.
///
/// Several agents share one directory (everything universal reads
/// `.agents/skills`), so the same path legitimately appears more than once.
pub fn agent_dirs(scope: &Scope) -> Vec<(&'static Agent, PathBuf)> {
    AGENTS
        .iter()
        .filter_map(|agent| scope.agent_dir(agent).map(|dir| (agent, dir)))
        .collect()
}

/// Agents whose skills directory currently exists on disk.
///
/// Used to keep the UI focused on agents the user actually has, rather than
/// all 79 the CLI supports.
pub fn present_agents(scope: &Scope) -> Vec<&'static Agent> {
    agent_dirs(scope)
        .into_iter()
        .filter(|(_, dir)| dir.is_dir() || dir.parent().is_some_and(Path::is_dir))
        .map(|(agent, _)| agent)
        .collect()
}

/// Fetch or create the record for `name`, seeding it from the lock file.
fn entry_for<'a>(
    skills: &'a mut BTreeMap<String, Skill>,
    name: &str,
    scope: &Scope,
    locks: &BTreeMap<String, LockEntry>,
) -> &'a mut Skill {
    skills.entry(name.to_string()).or_insert_with(|| Skill {
        name: name.to_string(),
        title: None,
        description: String::new(),
        scope: scope.clone(),
        canonical: None,
        installs: Vec::new(),
        lock: locks.get(name).cloned(),
        disabled: false,
        update: UpdateState::Unknown,
    })
}

/// Scan one scope and return every skill it knows about, sorted by name.
pub fn scan(scope: &Scope) -> Vec<Skill> {
    let locks = lock::read(scope);
    let canonical_dir = scope.canonical_dir();
    let disabled_dir = scope.disabled_dir();

    let mut skills: BTreeMap<String, Skill> = BTreeMap::new();

    // 1. The canonical copies every agent symlinks into.
    for found in read_skill_dirs(&canonical_dir) {
        let fm = read_front_matter(&found.path);
        let skill = entry_for(&mut skills, &found.name, scope, &locks);
        skill.canonical = Some(found.path);
        skill.title = fm.name;
        skill.description = fm.description.unwrap_or_default();
    }

    // 2. Skills Skillshard has parked out of every agent's reach.
    for found in read_skill_dirs(&disabled_dir) {
        let fm = read_front_matter(&found.path);
        let skill = entry_for(&mut skills, &found.name, scope, &locks);
        skill.disabled = true;
        skill.canonical = Some(found.path);
        skill.title = fm.name;
        skill.description = fm.description.unwrap_or_default();
    }

    // 3. Everything each agent actually loads.
    for (agent, dir) in agent_dirs(scope) {
        for found in read_skill_dirs(&dir) {
            let kind = classify(&found.path, &canonical_dir, &dir);
            let fm = read_front_matter(&found.path);
            let skill = entry_for(&mut skills, &found.name, scope, &locks);
            if skill.description.is_empty() {
                skill.title = fm.name;
                skill.description = fm.description.unwrap_or_default();
            }
            skill.installs.push(AgentInstall {
                agent,
                path: found.path,
                kind,
            });
        }
    }

    // 4. Lock entries whose files have gone missing, so the UI can surface the
    //    inconsistency instead of silently hiding it.
    for name in locks.keys() {
        entry_for(&mut skills, name, scope, &locks);
    }

    let mut out: Vec<Skill> = skills.into_values().collect();
    for skill in &mut out {
        skill.installs.sort_by_key(|i| i.agent.display);
        if skill.lock.is_none() || skill.lock.as_ref().unwrap().github_owner_repo().is_none() {
            skill.update = UpdateState::NotTracked;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::by_key;

    /// Build a project tree: canonical skills plus agent links.
    fn fixture() -> (PathBuf, Scope) {
        let root = std::env::temp_dir().join(format!(
            "skillshard-scan-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        (root.clone(), Scope::Project(root))
    }

    fn write_skill(dir: &Path, name: &str, description: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nBody\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn reports_canonical_skills_and_their_agent_links() {
        let (root, scope) = fixture();
        let canonical = write_skill(&scope.canonical_dir(), "alpha", "First skill");

        // Claude Code symlinks into the canonical copy, the CLI's default.
        let claude = root.join(".claude/skills");
        std::fs::create_dir_all(&claude).unwrap();
        std::os::unix::fs::symlink(&canonical, claude.join("alpha")).unwrap();

        let skills = scan(&scope);
        let alpha = skills.iter().find(|s| s.name == "alpha").unwrap();
        assert_eq!(alpha.description, "First skill");
        assert_eq!(alpha.canonical.as_deref(), Some(canonical.as_path()));
        assert!(alpha.is_enabled_for(by_key("claude-code").unwrap()));
        assert!(matches!(
            alpha.install_for(by_key("claude-code").unwrap()).unwrap().kind,
            InstallKind::Symlink { .. }
        ));
    }

    #[test]
    fn agents_sharing_the_canonical_directory_are_marked_canonical() {
        let (_root, scope) = fixture();
        write_skill(&scope.canonical_dir(), "beta", "Shared");

        let skills = scan(&scope);
        let beta = skills.iter().find(|s| s.name == "beta").unwrap();
        // Cursor's project directory *is* .agents/skills, so it cannot be
        // unlinked without deleting the skill.
        let cursor = by_key("cursor").unwrap();
        assert!(beta.is_enabled_for(cursor));
        let install = beta.install_for(cursor).unwrap();
        assert_eq!(install.kind, InstallKind::Canonical);
        assert!(!install.kind.is_unlinkable());
    }

    #[test]
    fn detects_copies_and_broken_links() {
        let (root, scope) = fixture();
        let claude = root.join(".claude/skills");
        std::fs::create_dir_all(&claude).unwrap();
        write_skill(&claude, "copied", "An independent copy");

        let gone = write_skill(&root.join("scratch"), "orphan", "Will vanish");
        std::os::unix::fs::symlink(&gone, claude.join("orphan")).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();

        let skills = scan(&scope);
        let claude_agent = by_key("claude-code").unwrap();
        let copied = skills.iter().find(|s| s.name == "copied").unwrap();
        assert_eq!(copied.install_for(claude_agent).unwrap().kind, InstallKind::Copy);
        // A dangling link has no SKILL.md behind it, so it is not listed as a
        // skill at all — the scan reports only what an agent can actually load.
        assert!(skills.iter().all(|s| s.name != "orphan"));
    }

    #[test]
    fn disabled_skills_are_parked_and_flagged() {
        let (_root, scope) = fixture();
        write_skill(&scope.disabled_dir(), "gamma", "Turned off");

        let skills = scan(&scope);
        let gamma = skills.iter().find(|s| s.name == "gamma").unwrap();
        assert!(gamma.disabled);
        assert!(gamma.installs.is_empty());
    }

    #[test]
    fn hidden_directories_are_not_skills() {
        let (_root, scope) = fixture();
        write_skill(&scope.canonical_dir(), ".system", "Private agent data");
        assert!(scan(&scope).is_empty());
    }

    #[test]
    fn a_locked_skill_missing_from_disk_is_still_reported() {
        let (root, scope) = fixture();
        std::fs::write(
            root.join("skills-lock.json"),
            r#"{"version":1,"skills":{"vanished":{"source":"o/r","sourceType":"github"}}}"#,
        )
        .unwrap();

        let skills = scan(&scope);
        let vanished = skills.iter().find(|s| s.name == "vanished").unwrap();
        assert!(vanished.installs.is_empty());
        assert!(vanished.canonical.is_none());
        assert_eq!(vanished.lock.as_ref().unwrap().source, "o/r");
    }
}
