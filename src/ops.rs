//! Filesystem operations Skillshard performs itself.
//!
//! These are the actions the `skills` CLI has no command for: turning a skill
//! off without uninstalling it, and linking or unlinking one agent at a time.
//! Anything that needs to fetch from a source is delegated to the CLI instead
//! (see [`crate::skills_cli`]).

use crate::agents::Agent;
use crate::model::{Scope, Skill};
use crate::state;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Why an operation could not be carried out.
#[derive(Debug)]
pub enum OpError {
    /// The skill has no files to link from.
    NoContent(String),
    /// The agent reads the canonical directory directly, so it has no separate
    /// link to add or remove.
    SharesCanonicalDir(&'static str),
    /// Something already occupies the destination.
    Occupied(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::NoContent(name) => {
                write!(f, "'{name}' has no files on disk to link")
            }
            OpError::SharesCanonicalDir(agent) => write!(
                f,
                "{agent} loads it from a directory shared with other agents, \
                 so it cannot be toggled on its own"
            ),
            OpError::Occupied(path) => {
                write!(f, "{} already exists", path.display())
            }
            OpError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for OpError {
    fn from(e: io::Error) -> Self {
        OpError::Io(e)
    }
}

type Result<T> = std::result::Result<T, OpError>;

/// Express `target` relative to `base`, so links survive a moved checkout.
///
/// Mirrors the relative links the `skills` CLI writes, e.g.
/// `~/.claude/skills/x -> ../../.agents/skills/x`.
fn relative_from(base: &Path, target: &Path) -> PathBuf {
    let strip = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    let (base, target) = (strip(base), strip(target));
    let shared = base
        .iter()
        .zip(target.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut out = PathBuf::new();
    for _ in shared..base.len() {
        out.push("..");
    }
    for part in &target[shared..] {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Create a directory symlink, portably.
fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link)
    }
}

/// Remove a skill entry, whether it is a link or a real directory.
fn remove_entry(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(path),
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Point `agent` at a skill by symlinking its canonical copy.
///
/// Idempotent: an existing link to the same place is left alone.
pub fn link_agent(skill: &Skill, agent: &'static Agent) -> Result<()> {
    let Some(agent_dir) = skill.scope.agent_dir(agent) else {
        return Err(OpError::SharesCanonicalDir(agent.display));
    };
    if skill.scope.reads_canonical(agent) {
        // Already visible to this agent by virtue of the shared directory.
        return Ok(());
    }
    // Usually the canonical .agents/skills copy. When the CLI installed
    // straight into a single agent's directory (its `--copy` mode, and what it
    // does for a single-agent install), that copy is the only source there is.
    let source = skill
        .canonical
        .clone()
        .or_else(|| skill.installs.first().map(|i| i.path.clone()))
        .ok_or_else(|| OpError::NoContent(skill.name.clone()))?;

    let link = agent_dir.join(&skill.name);
    if link.exists() || std::fs::symlink_metadata(&link).is_ok() {
        return Ok(());
    }
    std::fs::create_dir_all(&agent_dir)?;
    symlink_dir(&relative_from(&agent_dir, &source), &link)?;
    Ok(())
}

/// Stop `agent` loading a skill, leaving the canonical copy in place.
pub fn unlink_agent(skill: &Skill, agent: &'static Agent) -> Result<()> {
    let Some(install) = skill.install_for(agent) else {
        return Ok(());
    };
    if !install.kind.is_unlinkable() {
        return Err(OpError::SharesCanonicalDir(agent.display));
    }
    remove_entry(&install.path)?;
    Ok(())
}

/// Turn a skill off everywhere without uninstalling it.
///
/// The canonical copy is parked in the scope's disabled store and every agent
/// link is removed. The set of agents is remembered so [`enable`] can put
/// things back exactly as they were.
pub fn disable(skill: &Skill) -> Result<()> {
    if skill.disabled {
        return Ok(());
    }
    let source = skill
        .canonical
        .clone()
        .or_else(|| skill.installs.first().map(|i| i.path.clone()))
        .ok_or_else(|| OpError::NoContent(skill.name.clone()))?;

    // Remember the agents that were loading it, including those that only saw
    // it through the shared canonical directory.
    let agents: Vec<String> = skill.installs.iter().map(|i| i.agent.key.into()).collect();

    for install in &skill.installs {
        if install.kind.is_unlinkable() {
            remove_entry(&install.path)?;
        }
    }

    let parked = skill.scope.disabled_dir().join(&skill.name);
    if parked.exists() {
        return Err(OpError::Occupied(parked));
    }
    std::fs::create_dir_all(skill.scope.disabled_dir())?;
    move_dir(&source, &parked)?;

    let mut st = state::load(&skill.scope);
    st.disabled
        .insert(skill.name.clone(), state::DisabledRecord { agents });
    state::save(&skill.scope, &st)?;
    Ok(())
}

/// Turn a disabled skill back on, restoring its previous agent links.
pub fn enable(skill: &Skill) -> Result<()> {
    if !skill.disabled {
        return Ok(());
    }
    let parked = skill.scope.disabled_dir().join(&skill.name);
    if !parked.exists() {
        return Err(OpError::NoContent(skill.name.clone()));
    }
    let canonical = skill.scope.canonical_dir().join(&skill.name);
    if canonical.exists() {
        return Err(OpError::Occupied(canonical));
    }
    std::fs::create_dir_all(skill.scope.canonical_dir())?;
    move_dir(&parked, &canonical)?;

    let mut st = state::load(&skill.scope);
    let record = st.disabled.remove(&skill.name).unwrap_or_default();
    state::save(&skill.scope, &st)?;

    // Relink with the canonical copy back in place.
    let restored = Skill {
        canonical: Some(canonical),
        disabled: false,
        installs: Vec::new(),
        ..skill.clone()
    };
    for key in &record.agents {
        if let Some(agent) = crate::agents::by_key(key) {
            link_agent(&restored, agent)?;
        }
    }
    Ok(())
}

/// Whether `agent` can be toggled independently in `scope`.
///
/// Agents that read the shared `.agents/skills` directory — as their own or as
/// an extra directory — see a skill purely
/// because it exists there, so they have no link of their own to add or
/// remove. They follow the skill's enabled state instead.
pub fn is_togglable(scope: &Scope, agent: &Agent) -> bool {
    scope.agent_dir(agent).is_some() && !scope.reads_canonical(agent)
}

/// Move a directory, falling back to copy-then-delete across filesystems.
fn move_dir(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_dir(from, to)?;
            std::fs::remove_dir_all(from)
        }
    }
}

/// Recursively copy `from` into `to`.
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::by_key;
    use crate::scan;

    fn fixture(tag: &str) -> Scope {
        let root = std::env::temp_dir().join(format!(
            "skillshard-ops-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scope::Project(root)
    }

    fn write_skill(scope: &Scope, name: &str) {
        let path = scope.canonical_dir().join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: d\n---\n"),
        )
        .unwrap();
    }

    fn get(scope: &Scope, name: &str) -> Skill {
        scan::scan(scope)
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} not found"))
    }

    #[test]
    fn computes_relative_link_targets() {
        assert_eq!(
            relative_from(
                Path::new("/home/u/.claude/skills"),
                Path::new("/home/u/.agents/skills/x")
            ),
            PathBuf::from("../../.agents/skills/x")
        );
    }

    #[test]
    fn links_and_unlinks_a_single_agent() {
        let scope = fixture("link");
        write_skill(&scope, "alpha");
        let claude = by_key("claude-code").unwrap();

        link_agent(&get(&scope, "alpha"), claude).unwrap();
        let skill = get(&scope, "alpha");
        assert!(skill.is_enabled_for(claude));
        // The CLI's own convention: a relative symlink, not a copy.
        let link = scope.agent_dir(claude).unwrap().join("alpha");
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());

        unlink_agent(&get(&scope, "alpha"), claude).unwrap();
        assert!(!get(&scope, "alpha").is_enabled_for(claude));
        // Unlinking never touches the canonical copy.
        assert!(scope.canonical_dir().join("alpha").is_dir());
    }

    #[test]
    fn refuses_to_unlink_an_agent_that_shares_the_canonical_dir() {
        let scope = fixture("shared");
        write_skill(&scope, "beta");
        let cursor = by_key("cursor").unwrap();
        let err = unlink_agent(&get(&scope, "beta"), cursor).unwrap_err();
        assert!(matches!(err, OpError::SharesCanonicalDir(_)));
        assert!(scope.canonical_dir().join("beta").is_dir());
    }

    #[test]
    fn agents_reading_the_canonical_dir_through_an_extra_directory_are_not_togglable() {
        assert!(!is_togglable(&Scope::Global, by_key("codex").unwrap()));
        assert!(!is_togglable(&Scope::Global, by_key("pi").unwrap()));
        assert!(is_togglable(&Scope::Global, by_key("claude-code").unwrap()));
    }

    #[test]
    fn unlinking_an_inherited_install_leaves_the_owner_alone() {
        let scope = fixture("inherited");
        let claude = by_key("claude-code").unwrap();
        let claude_dir = scope.agent_dir(claude).unwrap();
        std::fs::create_dir_all(claude_dir.join("zeta")).unwrap();
        std::fs::write(
            claude_dir.join("zeta/SKILL.md"),
            "---\nname: zeta\ndescription: d\n---\n",
        )
        .unwrap();

        let err = unlink_agent(&get(&scope, "zeta"), by_key("opencode").unwrap()).unwrap_err();
        assert!(matches!(err, OpError::SharesCanonicalDir(_)));
        assert!(claude_dir.join("zeta/SKILL.md").is_file());
    }

    #[test]
    fn links_from_an_agent_copy_when_there_is_no_canonical_one() {
        // A single-agent install leaves the files in that agent's directory
        // with no .agents/skills copy; another agent must still be linkable.
        let scope = fixture("copyonly");
        let claude = by_key("claude-code").unwrap();
        let windsurf = by_key("windsurf").unwrap();
        let claude_dir = scope.agent_dir(claude).unwrap();
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::create_dir_all(claude_dir.join("epsilon")).unwrap();
        std::fs::write(
            claude_dir.join("epsilon/SKILL.md"),
            "---\nname: epsilon\ndescription: d\n---\n",
        )
        .unwrap();

        let skill = get(&scope, "epsilon");
        assert!(skill.canonical.is_none());
        link_agent(&skill, windsurf).unwrap();
        assert!(get(&scope, "epsilon").is_enabled_for(windsurf));
    }

    #[test]
    fn disable_parks_the_skill_and_enable_restores_its_agents() {
        let scope = fixture("toggle");
        write_skill(&scope, "gamma");
        let claude = by_key("claude-code").unwrap();
        link_agent(&get(&scope, "gamma"), claude).unwrap();

        disable(&get(&scope, "gamma")).unwrap();
        let off = get(&scope, "gamma");
        assert!(off.disabled);
        assert!(off.installs.is_empty());
        assert!(!scope.canonical_dir().join("gamma").exists());
        assert!(scope
            .disabled_dir()
            .join("gamma")
            .join("SKILL.md")
            .is_file());

        enable(&get(&scope, "gamma")).unwrap();
        let on = get(&scope, "gamma");
        assert!(!on.disabled);
        assert!(on.is_enabled_for(claude), "previous agents are restored");
        assert!(scope.canonical_dir().join("gamma").is_dir());
    }
}
