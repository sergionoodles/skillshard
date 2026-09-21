//! Registry of coding agents that consume skills.
//!
//! Generated from the `skills` CLI supported-agents table (v1.7.0) —
//! see `docs/skills-sh-integration.md`. Paths mirror that table exactly so
//! Skillshard reads and writes the same locations the CLI does.

/// A coding agent and the directories it loads skills from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agent {
    /// Identifier accepted by `npx skills --agent <key>`.
    pub key: &'static str,
    /// Human-readable name shown in the UI.
    pub display: &'static str,
    /// Skills directory relative to a project root.
    pub project_dir: &'static str,
    /// Skills directory for global installs, `~`-relative.
    /// `None` for agents that only support project scope.
    pub global_dir: Option<&'static str>,
}

/// Every agent Skillshard knows about: all the `skills` CLI can install to,
/// plus those in [`NOT_IN_CLI`].
pub const AGENTS: &[Agent] = &[
    Agent {
        key: "adal",
        display: "AdaL",
        project_dir: ".adal/skills",
        global_dir: Some("~/.adal/skills"),
    },
    Agent {
        key: "aider-desk",
        display: "AiderDesk",
        project_dir: ".aider-desk/skills",
        global_dir: Some("~/.aider-desk/skills"),
    },
    Agent {
        key: "amp",
        display: "Amp",
        project_dir: ".agents/skills",
        global_dir: Some("~/.config/agents/skills"),
    },
    Agent {
        key: "antigravity",
        display: "Antigravity",
        project_dir: ".agents/skills",
        global_dir: Some("~/.gemini/antigravity/skills"),
    },
    Agent {
        key: "antigravity-cli",
        display: "Antigravity CLI",
        project_dir: ".agents/skills",
        global_dir: Some("~/.gemini/antigravity-cli/skills"),
    },
    Agent {
        key: "astrbot",
        display: "AstrBot",
        project_dir: "data/skills",
        global_dir: Some("~/.astrbot/data/skills"),
    },
    Agent {
        key: "augment",
        display: "Augment",
        project_dir: ".augment/skills",
        global_dir: Some("~/.augment/skills"),
    },
    Agent {
        key: "autohand-code",
        display: "Autohand Code CLI",
        project_dir: ".autohand/skills",
        global_dir: Some("~/.autohand/skills"),
    },
    Agent {
        key: "claude-code",
        display: "Claude Code",
        project_dir: ".claude/skills",
        global_dir: Some("~/.claude/skills"),
    },
    Agent {
        key: "cline",
        display: "Cline",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
    },
    Agent {
        key: "codestudio",
        display: "Code Studio",
        project_dir: ".codestudio/skills",
        global_dir: Some("~/.codestudio/skills"),
    },
    Agent {
        key: "codearts-agent",
        display: "CodeArts Agent",
        project_dir: ".codeartsdoer/skills",
        global_dir: Some("~/.codeartsdoer/skills"),
    },
    Agent {
        key: "codebuddy",
        display: "CodeBuddy",
        project_dir: ".codebuddy/skills",
        global_dir: Some("~/.codebuddy/skills"),
    },
    Agent {
        key: "codemaker",
        display: "Codemaker",
        project_dir: ".codemaker/skills",
        global_dir: Some("~/.codemaker/skills"),
    },
    Agent {
        key: "codex",
        display: "Codex",
        project_dir: ".agents/skills",
        global_dir: Some("~/.codex/skills"),
    },
    Agent {
        key: "command-code",
        display: "Command Code",
        project_dir: ".commandcode/skills",
        global_dir: Some("~/.commandcode/skills"),
    },
    Agent {
        key: "continue",
        display: "Continue",
        project_dir: ".continue/skills",
        global_dir: Some("~/.continue/skills"),
    },
    Agent {
        key: "cortex",
        display: "Cortex Code",
        project_dir: ".cortex/skills",
        global_dir: Some("~/.snowflake/cortex/skills"),
    },
    Agent {
        key: "crush",
        display: "Crush",
        project_dir: ".crush/skills",
        global_dir: Some("~/.config/crush/skills"),
    },
    Agent {
        key: "cursor",
        display: "Cursor",
        project_dir: ".agents/skills",
        global_dir: Some("~/.cursor/skills"),
    },
    Agent {
        key: "deepagents",
        display: "Deep Agents",
        project_dir: ".agents/skills",
        global_dir: Some("~/.deepagents/agent/skills"),
    },
    Agent {
        key: "devin",
        display: "Devin for Terminal",
        project_dir: ".devin/skills",
        global_dir: Some("~/.config/devin/skills"),
    },
    Agent {
        key: "dexto",
        display: "Dexto",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
    },
    Agent {
        key: "droid",
        display: "Droid",
        project_dir: ".agents/skills",
        global_dir: Some("~/.factory/skills"),
    },
    Agent {
        key: "eve",
        display: "Eve",
        project_dir: "agent/skills",
        global_dir: None,
    },
    Agent {
        key: "firebender",
        display: "Firebender",
        project_dir: ".agents/skills",
        global_dir: Some("~/.firebender/skills"),
    },
    Agent {
        key: "forgecode",
        display: "ForgeCode",
        project_dir: ".forge/skills",
        global_dir: Some("~/.forge/skills"),
    },
    Agent {
        key: "fx",
        display: "fx",
        project_dir: ".fx/skills",
        global_dir: Some("~/.fx/skills"),
    },
    Agent {
        key: "gemini-cli",
        display: "Gemini CLI",
        project_dir: ".agents/skills",
        global_dir: Some("~/.gemini/skills"),
    },
    Agent {
        key: "github-copilot",
        display: "GitHub Copilot",
        project_dir: ".agents/skills",
        global_dir: Some("~/.copilot/skills"),
    },
    Agent {
        key: "goose",
        display: "Goose",
        project_dir: ".goose/skills",
        global_dir: Some("~/.config/goose/skills"),
    },
    Agent {
        key: "grok",
        display: "Grok Build",
        project_dir: ".grok/skills",
        global_dir: Some("~/.grok/skills"),
    },
    Agent {
        key: "hermes-agent",
        display: "Hermes Agent",
        project_dir: ".hermes/skills",
        global_dir: Some("~/.hermes/skills"),
    },
    Agent {
        key: "bob",
        display: "IBM Bob",
        project_dir: ".bob/skills",
        global_dir: Some("~/.bob/skills"),
    },
    Agent {
        key: "iflow-cli",
        display: "iFlow CLI",
        project_dir: ".iflow/skills",
        global_dir: Some("~/.iflow/skills"),
    },
    Agent {
        key: "inference-sh",
        display: "inference.sh",
        project_dir: ".inferencesh/skills",
        global_dir: Some("~/.inferencesh/skills"),
    },
    Agent {
        key: "jazz",
        display: "Jazz",
        project_dir: ".jazz/skills",
        global_dir: Some("~/.jazz/skills"),
    },
    Agent {
        key: "junie",
        display: "Junie",
        project_dir: ".junie/skills",
        global_dir: Some("~/.junie/skills"),
    },
    Agent {
        key: "kilo",
        display: "Kilo Code",
        project_dir: ".agents/skills",
        global_dir: Some("~/.kilo/skills"),
    },
    Agent {
        key: "kimchi",
        display: "Kimchi",
        project_dir: ".kimchi/skills",
        global_dir: Some("~/.config/kimchi/harness/skills"),
    },
    Agent {
        key: "kimi-code-cli",
        display: "Kimi Code CLI",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
    },
    Agent {
        key: "kiro-cli",
        display: "Kiro CLI",
        project_dir: ".kiro/skills",
        global_dir: Some("~/.kiro/skills"),
    },
    Agent {
        key: "kode",
        display: "Kode",
        project_dir: ".kode/skills",
        global_dir: Some("~/.kode/skills"),
    },
    Agent {
        key: "lingma",
        display: "Lingma",
        project_dir: ".lingma/skills",
        global_dir: Some("~/.lingma/skills"),
    },
    Agent {
        key: "loaf",
        display: "Loaf",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
    },
    Agent {
        key: "mcpjam",
        display: "MCPJam",
        project_dir: ".mcpjam/skills",
        global_dir: Some("~/.mcpjam/skills"),
    },
    Agent {
        key: "minimax-code",
        display: "MiniMax Code",
        project_dir: ".minimax/skills",
        global_dir: Some("~/.minimax/skills"),
    },
    Agent {
        key: "mistral-vibe",
        display: "Mistral Vibe",
        project_dir: ".vibe/skills",
        global_dir: Some("~/.vibe/skills"),
    },
    Agent {
        key: "moxby",
        display: "Moxby",
        project_dir: ".moxby/skills",
        global_dir: Some("~/.moxby/skills"),
    },
    Agent {
        key: "mux",
        display: "Mux",
        project_dir: ".mux/skills",
        global_dir: Some("~/.mux/skills"),
    },
    Agent {
        key: "neovate",
        display: "Neovate",
        project_dir: ".neovate/skills",
        global_dir: Some("~/.neovate/skills"),
    },
    Agent {
        key: "ona",
        display: "Ona",
        project_dir: ".ona/skills",
        global_dir: Some("~/.ona/skills"),
    },
    Agent {
        key: "omp",
        display: "OMP",
        project_dir: ".omp/skills",
        global_dir: Some("~/.omp/agent/skills"),
    },
    Agent {
        key: "openclaw",
        display: "OpenClaw",
        project_dir: "skills",
        global_dir: Some("~/.openclaw/skills"),
    },
    Agent {
        key: "opencode",
        display: "OpenCode",
        project_dir: ".agents/skills",
        global_dir: Some("~/.config/opencode/skills"),
    },
    Agent {
        key: "openhands",
        display: "OpenHands",
        project_dir: ".openhands/skills",
        global_dir: Some("~/.openhands/skills"),
    },
    Agent {
        key: "pi",
        display: "Pi",
        project_dir: ".pi/skills",
        global_dir: Some("~/.pi/agent/skills"),
    },
    Agent {
        key: "pochi",
        display: "Pochi",
        project_dir: ".pochi/skills",
        global_dir: Some("~/.pochi/skills"),
    },
    Agent {
        key: "posit-assistant",
        display: "Posit Assistant",
        project_dir: ".posit/assistant/skills",
        global_dir: Some("~/.posit/assistant/skills"),
    },
    Agent {
        key: "promptscript",
        display: "PromptScript",
        project_dir: ".agents/skills",
        global_dir: None,
    },
    Agent {
        key: "qoder",
        display: "Qoder",
        project_dir: ".qoder/skills",
        global_dir: Some("~/.qoder/skills"),
    },
    Agent {
        key: "qoder-cn",
        display: "Qoder CN",
        project_dir: ".qoder/skills",
        global_dir: Some("~/.qoder-cn/skills"),
    },
    Agent {
        key: "qwen-code",
        display: "Qwen Code",
        project_dir: ".qwen/skills",
        global_dir: Some("~/.qwen/skills"),
    },
    Agent {
        key: "reasonix",
        display: "Reasonix",
        project_dir: ".reasonix/skills",
        global_dir: Some("~/.reasonix/skills"),
    },
    Agent {
        key: "replit",
        display: "Replit",
        project_dir: ".agents/skills",
        global_dir: Some("~/.config/agents/skills"),
    },
    Agent {
        key: "roo",
        display: "Roo Code",
        project_dir: ".roo/skills",
        global_dir: Some("~/.roo/skills"),
    },
    Agent {
        key: "rovodev",
        display: "Rovo Dev",
        project_dir: ".rovodev/skills",
        global_dir: Some("~/.rovodev/skills"),
    },
    Agent {
        key: "sarvam-code",
        display: "Sarvam Code",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
    },
    Agent {
        key: "tabnine-cli",
        display: "Tabnine CLI",
        project_dir: ".tabnine/agent/skills",
        global_dir: Some("~/.tabnine/agent/skills"),
    },
    Agent {
        key: "terramind",
        display: "Terramind",
        project_dir: ".terramind/skills",
        global_dir: Some("~/.terramind/skills"),
    },
    Agent {
        key: "tinycloud",
        display: "Tinycloud",
        project_dir: ".tinycloud/skills",
        global_dir: Some("~/.tinycloud/skills"),
    },
    Agent {
        key: "trae",
        display: "Trae",
        project_dir: ".trae/skills",
        global_dir: Some("~/.trae/skills"),
    },
    Agent {
        key: "trae-cn",
        display: "Trae CN",
        project_dir: ".trae/skills",
        global_dir: Some("~/.trae-cn/skills"),
    },
    Agent {
        key: "universal",
        display: "Universal",
        project_dir: ".agents/skills",
        global_dir: Some("~/.config/agents/skills"),
    },
    Agent {
        key: "warp",
        display: "Warp",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
    },
    Agent {
        key: "windsurf",
        display: "Windsurf",
        project_dir: ".windsurf/skills",
        global_dir: Some("~/.codeium/windsurf/skills"),
    },
    Agent {
        key: "zcode",
        display: "ZCode",
        project_dir: ".zcode/skills",
        global_dir: Some("~/.zcode/skills"),
    },
    Agent {
        key: "zed",
        display: "Zed",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
    },
    Agent {
        key: "zencoder",
        display: "Zencoder",
        project_dir: ".zencoder/skills",
        global_dir: Some("~/.zencoder/skills"),
    },
    Agent {
        key: "zenflow",
        display: "Zenflow",
        project_dir: ".zencoder/skills",
        global_dir: Some("~/.zencoder/skills"),
    },
];

/// Agents in [`AGENTS`] the `skills` CLI has no `--agent` key for.
///
/// Skillshard detects and links their skills itself, but must never pass
/// their keys to the CLI.
const NOT_IN_CLI: &[&str] = &["omp"];

/// Global homes several agents share. The `skills` CLI creates them for any
/// of those agents, so their existence says nothing about which is installed.
const SHARED_HOMES: &[&str] = &["~/.agents", "~/.config/agents"];

/// Directories an agent loads skills from besides its CLI install directory.
struct ExtraDirs {
    key: &'static str,
    project: &'static [&'static str],
    global: &'static [&'static str],
}

/// Only directories inside a project root or the home directory are listed;
/// system-wide ones (Codex's `/etc/codex/skills`) are not a Skillshard scope.
const EXTRA_DIRS: &[ExtraDirs] = &[
    // https://learn.chatgpt.com/docs/build-skills
    ExtraDirs {
        key: "codex",
        project: &[],
        global: &["~/.agents/skills"],
    },
    // https://opencode.ai/docs/skills/
    ExtraDirs {
        key: "opencode",
        project: &[".opencode/skills", ".claude/skills"],
        global: &["~/.agents/skills", "~/.claude/skills"],
    },
    // https://pi.dev/docs/latest/skills
    ExtraDirs {
        key: "pi",
        project: &[".agents/skills"],
        global: &["~/.agents/skills"],
    },
    // https://omp.sh/docs/skills — Claude, Codex and OpenCode user
    // directories are opt-in there, so only their project roots count.
    ExtraDirs {
        key: "omp",
        project: &[
            ".agents/skills",
            ".agent/skills",
            ".claude/skills",
            ".codex/skills",
            ".opencode/skills",
            ".github/skills",
        ],
        global: &["~/.agents/skills", "~/.agent/skills"],
    },
];

impl Agent {
    /// Whether the `skills` CLI accepts this agent's key.
    pub fn in_cli(&self) -> bool {
        !NOT_IN_CLI.contains(&self.key)
    }

    /// Project-relative skills directories read besides [`Agent::project_dir`].
    pub fn extra_project_dirs(&self) -> &'static [&'static str] {
        self.extras().map_or(&[], |e| e.project)
    }

    /// `~`-relative skills directories read besides [`Agent::global_dir`].
    pub fn extra_global_dirs(&self) -> &'static [&'static str] {
        self.extras().map_or(&[], |e| e.global)
    }

    /// Whether the directory holding this agent's global skills directory
    /// belongs to this agent alone. Agents create that directory themselves,
    /// so when it is theirs, it existing is the sign they are installed.
    ///
    /// False for agents with no global directory, or one shared with other
    /// agents: those cannot be detected.
    pub fn has_own_home(&self) -> bool {
        self.global_dir
            .and_then(|dir| dir.strip_suffix("/skills"))
            .is_some_and(|home| !SHARED_HOMES.contains(&home))
    }

    fn extras(&self) -> Option<&'static ExtraDirs> {
        EXTRA_DIRS.iter().find(|e| e.key == self.key)
    }
}

/// Look up an agent by its `--agent` key.
pub fn by_key(key: &str) -> Option<&'static Agent> {
    AGENTS.iter().find(|a| a.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_with_their_own_config_directory_can_be_detected() {
        for key in ["claude-code", "pi", "goose", "github-copilot"] {
            assert!(by_key(key).unwrap().has_own_home(), "{key}");
        }
    }

    #[test]
    fn agents_without_a_home_of_their_own_cannot_be_detected() {
        // ~/.agents/skills is where the CLI puts every universal install.
        assert!(!by_key("cline").unwrap().has_own_home());
        assert!(!by_key("amp").unwrap().has_own_home());
        assert!(AGENTS
            .iter()
            .filter(|a| a.global_dir.is_none())
            .all(|a| !a.has_own_home()));
    }
}
