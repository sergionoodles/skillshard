//! Discovers which skills exist on disk and which agents load them.
//!
//! The filesystem is the source of truth. The scanner walks the canonical
//! `.agents/skills` tree, Skillshard's disabled store, and every agent's own
//! skills directory, then joins the result with the CLI's lock file.

use crate::agents::{Agent, AGENTS};
use crate::frontmatter;
use crate::lock;
use crate::model::{AgentInstall, InstallKind, LockEntry, Scope, Skill, UpdateState};
use crate::tokens::{self, TokenCost};
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

/// Agents installed on this machine: those whose own directory, the one
/// holding their global skills directory, exists (see [`Agent::has_own_home`]).
///
/// The same set applies to every scope. A project's directory layout says
/// nothing about which agents the user has: `skills/` is OpenClaw's project
/// directory, and `.agents/skills` is read by dozens of agents.
pub fn installed_agents() -> Vec<&'static Agent> {
    AGENTS
        .iter()
        .filter(|agent| {
            agent.has_own_home()
                && Scope::Global
                    .agent_dir(agent)
                    .is_some_and(|dir| dir.parent().is_some_and(Path::is_dir))
        })
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
        cost: TokenCost::default(),
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
        let skill = entry_for(&mut skills, &found.name, scope, &locks);
        if skill.canonical.is_some() {
            continue;
        }
        let fm = read_front_matter(&found.path);
        skill.disabled = true;
        skill.canonical = Some(found.path);
        skill.title = fm.name;
        skill.description = fm.description.unwrap_or_default();
    }

    // 3. Everything each agent actually loads. An agent's own directory comes
    //    first, so when it holds the skill too that entry is the one recorded.
    for agent in AGENTS {
        for (i, dir) in scope.agent_read_dirs(agent).into_iter().enumerate() {
            for found in read_skill_dirs(&dir) {
                let kind = if i == 0 || dir == canonical_dir {
                    classify(&found.path, &canonical_dir, &dir)
                } else {
                    InstallKind::Inherited
                };
                let fm = read_front_matter(&found.path);
                let skill = entry_for(&mut skills, &found.name, scope, &locks);
                if skill.description.is_empty() {
                    skill.title = fm.name;
                    skill.description = fm.description.unwrap_or_default();
                }
                if skill.is_enabled_for(agent) {
                    continue;
                }
                skill.installs.push(AgentInstall {
                    agent,
                    path: found.path,
                    kind,
                });
            }
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
        // Measured once from wherever the files actually are, rather than per
        // agent: every link points at the same content.
        if let Some(path) = skill.content_path().map(Path::to_path_buf) {
            skill.cost = tokens::measure(&path);
        }
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
            alpha
                .install_for(by_key("claude-code").unwrap())
                .unwrap()
                .kind,
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
        assert_eq!(
            copied.install_for(claude_agent).unwrap().kind,
            InstallKind::Copy
        );
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
    fn canonical_copy_wins_over_a_parked_copy_with_the_same_name() {
        let (_root, scope) = fixture();
        let canonical = write_skill(&scope.canonical_dir(), "gamma", "Installed again");
        write_skill(&scope.disabled_dir(), "gamma", "Old disabled copy");

        let skills = scan(&scope);
        let gamma = skills.iter().find(|s| s.name == "gamma").unwrap();
        assert!(!gamma.disabled);
        assert_eq!(gamma.canonical.as_deref(), Some(canonical.as_path()));
        assert!(gamma.description.contains("Installed again"));
    }

    #[test]
    fn hidden_directories_are_not_skills() {
        let (_root, scope) = fixture();
        write_skill(&scope.canonical_dir(), ".system", "Private agent data");
        assert!(scan(&scope).is_empty());
    }

    #[test]
    fn agents_also_see_skills_in_the_other_directories_they_read() {
        let (root, scope) = fixture();
        let claude = root.join(".claude/skills");
        std::fs::create_dir_all(&claude).unwrap();
        write_skill(&claude, "from-claude", "Claude's copy");

        let skills = scan(&scope);
        let skill = skills.iter().find(|s| s.name == "from-claude").unwrap();
        // OpenCode reads .claude/skills as well as its own directory, but the
        // entry belongs to Claude Code: OpenCode must not be able to unlink it.
        let install = skill.install_for(by_key("opencode").unwrap()).unwrap();
        assert_eq!(install.kind, InstallKind::Inherited);
        assert!(!install.kind.is_unlinkable());
        let claude = skill.install_for(by_key("claude-code").unwrap()).unwrap();
        assert_eq!(claude.kind, InstallKind::Copy);
    }

    #[test]
    fn pi_and_omp_read_the_canonical_directory() {
        let (_root, scope) = fixture();
        write_skill(&scope.canonical_dir(), "shared", "Everyone");

        let skills = scan(&scope);
        let shared = skills.iter().find(|s| s.name == "shared").unwrap();
        for key in ["pi", "omp"] {
            let install = shared.install_for(by_key(key).unwrap()).unwrap();
            assert_eq!(install.kind, InstallKind::Canonical, "{key}");
        }
    }

    #[test]
    fn an_agent_is_listed_once_even_when_several_of_its_directories_hold_the_skill() {
        let (root, scope) = fixture();
        let canonical = write_skill(&scope.canonical_dir(), "twice", "Linked and shared");
        let omp = root.join(".omp/skills");
        std::fs::create_dir_all(&omp).unwrap();
        std::os::unix::fs::symlink(&canonical, omp.join("twice")).unwrap();

        let skills = scan(&scope);
        let twice = skills.iter().find(|s| s.name == "twice").unwrap();
        let omp_agent = by_key("omp").unwrap();
        let installs: Vec<_> = twice
            .installs
            .iter()
            .filter(|i| i.agent.key == omp_agent.key)
            .collect();
        assert_eq!(installs.len(), 1);
        // Its own directory wins over one it merely reads.
        assert!(matches!(installs[0].kind, InstallKind::Symlink { .. }));
    }

    #[test]
    fn codex_reads_the_global_canonical_directory() {
        let codex = by_key("codex").unwrap();
        let dirs = Scope::Global.agent_read_dirs(codex);
        assert_eq!(dirs.first(), Scope::Global.agent_dir(codex).as_ref());
        assert!(dirs.contains(&Scope::Global.canonical_dir()));
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
