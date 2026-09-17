//! Core domain types shared by the scanner, the filesystem operations and the UI.

use crate::agents::Agent;
use crate::paths;
use std::path::{Path, PathBuf};

/// Where a set of skills lives: user-wide, or inside one project checkout.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Installed under the user's home directory, available to every project.
    Global,
    /// Installed inside a project checkout and usually committed with it.
    Project(PathBuf),
}

impl Scope {
    /// The `--global` / `--project` flag this scope maps to.
    pub fn is_global(&self) -> bool {
        matches!(self, Scope::Global)
    }

    /// Short label for the UI.
    pub fn label(&self) -> String {
        match self {
            Scope::Global => "Global".to_string(),
            Scope::Project(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| paths::shorten(path)),
        }
    }

    /// Directory holding the canonical copy of each skill in this scope.
    ///
    /// This is the `.agents/skills` tree the `skills` CLI symlinks agents into.
    pub fn canonical_dir(&self) -> PathBuf {
        match self {
            Scope::Global => paths::home().join(".agents/skills"),
            Scope::Project(root) => root.join(".agents/skills"),
        }
    }

    /// Directory where Skillshard parks disabled skills.
    ///
    /// Kept outside `.agents/skills` so no agent loads them, and inside the
    /// same scope so disabling a project skill stays with the project.
    pub fn disabled_dir(&self) -> PathBuf {
        match self {
            Scope::Global => paths::home().join(".agents/.skillshard/disabled"),
            Scope::Project(root) => root.join(".agents/.skillshard/disabled"),
        }
    }

    /// Skillshard's own state file for this scope.
    pub fn state_file(&self) -> PathBuf {
        match self {
            Scope::Global => paths::home().join(".agents/.skillshard/state.json"),
            Scope::Project(root) => root.join(".agents/.skillshard/state.json"),
        }
    }

    /// Absolute skills directory for `agent` in this scope.
    ///
    /// `None` when the agent does not support the scope (a handful are
    /// project-only).
    pub fn agent_dir(&self, agent: &Agent) -> Option<PathBuf> {
        match self {
            Scope::Global => agent.global_dir.map(paths::expand),
            Scope::Project(root) => Some(root.join(agent.project_dir)),
        }
    }
}

/// How a skill is present in one agent's skills directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// A symlink into the canonical `.agents/skills` copy — the CLI default.
    Symlink { target: PathBuf },
    /// A symlink whose target no longer exists.
    Broken { target: PathBuf },
    /// An independent copy, as produced by `skills add --copy`.
    Copy,
    /// The agent reads the canonical directory directly, so it cannot be
    /// unlinked without removing the skill itself.
    Canonical,
}

impl InstallKind {
    /// Whether toggling this agent off is a link removal rather than a delete.
    pub fn is_unlinkable(&self) -> bool {
        !matches!(self, InstallKind::Canonical)
    }
}

/// A skill as one agent sees it.
#[derive(Debug, Clone)]
pub struct AgentInstall {
    pub agent: &'static Agent,
    pub path: PathBuf,
    pub kind: InstallKind,
}

/// Whether a newer version of a skill exists upstream.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateState {
    /// Not checked yet.
    #[default]
    Unknown,
    /// Checking upstream right now.
    Checking,
    /// Upstream tree hash matches the lock file.
    UpToDate,
    /// Upstream has moved on; `skills update` will pull it in.
    Available,
    /// No lock entry, or a source that cannot be checked over the API
    /// (local paths, plain git remotes, well-known endpoints).
    NotTracked,
    /// The check itself failed, e.g. rate limiting or no network.
    Failed(String),
}

/// One skill in one scope, with everything the UI needs to describe it.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Directory name — the identifier used for filesystem and CLI operations.
    pub name: String,
    /// `name` from the front matter, when it differs from the directory.
    pub title: Option<String>,
    pub description: String,
    pub scope: Scope,
    /// Canonical copy, when one exists in `.agents/skills`.
    pub canonical: Option<PathBuf>,
    /// Agents that currently load this skill.
    pub installs: Vec<AgentInstall>,
    /// Lock entry recorded by the `skills` CLI, when the skill was installed
    /// through it.
    pub lock: Option<LockEntry>,
    /// Disabled by Skillshard: parked outside every agent's reach.
    pub disabled: bool,
    pub update: UpdateState,
}

impl Skill {
    /// Display name, preferring the front-matter name.
    pub fn display_name(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }

    /// Whether `agent` currently loads this skill.
    pub fn is_enabled_for(&self, agent: &Agent) -> bool {
        self.installs.iter().any(|i| i.agent.key == agent.key)
    }

    /// The install record for `agent`, if any.
    pub fn install_for(&self, agent: &Agent) -> Option<&AgentInstall> {
        self.installs.iter().find(|i| i.agent.key == agent.key)
    }

    /// Where the skill's files actually live right now.
    pub fn content_path(&self) -> Option<&Path> {
        self.canonical
            .as_deref()
            .or_else(|| self.installs.first().map(|i| i.path.as_path()))
    }

    /// Short "owner/repo" style origin for the UI.
    pub fn source_label(&self) -> &str {
        self.lock
            .as_ref()
            .map(|l| l.source.as_str())
            .unwrap_or("local")
    }
}

/// A skill's entry in one of the `skills` CLI lock files.
///
/// The global lock (`~/.agents/.skill-lock.json`) stores a GitHub tree SHA;
/// the project lock (`skills-lock.json`) stores a content hash. Both are kept
/// here under `hash`, with `hash_is_tree_sha` recording which one it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockEntry {
    pub source: String,
    pub source_type: String,
    pub source_url: Option<String>,
    pub skill_path: Option<String>,
    pub git_ref: Option<String>,
    pub hash: Option<String>,
    pub hash_is_tree_sha: bool,
    pub installed_at: Option<String>,
    pub updated_at: Option<String>,
}

impl LockEntry {
    /// `owner/repo` when the source is a GitHub repository.
    ///
    /// Update checking uses the GitHub trees API, so only these can be checked.
    pub fn github_owner_repo(&self) -> Option<&str> {
        if self.source_type != "github" {
            return None;
        }
        let source = self.source.trim_end_matches(".git");
        let mut parts = source.split('/');
        let (owner, repo) = (parts.next()?, parts.next()?);
        if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
            return None;
        }
        Some(source)
    }
}
