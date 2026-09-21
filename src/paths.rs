//! Home-relative path resolution, matching the `skills` CLI's own rules.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The user's home directory.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// `$XDG_CONFIG_HOME`, falling back to `~/.config`.
fn config_home() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(".config"),
    }
}

/// Expand a `~`-relative agent directory into an absolute path.
///
/// Honours the same environment overrides the CLI reads, so Skillshard and
/// `npx skills` always agree on where an agent keeps its skills.
pub fn expand(tilde_path: &str) -> PathBuf {
    let rest = tilde_path.strip_prefix("~/").unwrap_or(tilde_path);

    // Agent-specific home overrides, checked before the generic rules.
    for (prefix, env) in [
        (".claude/", "CLAUDE_CONFIG_DIR"),
        (".codex/", "CODEX_HOME"),
        (".vibe/", "VIBE_HOME"),
        (".hermes/", "HERMES_HOME"),
        (".autohand/", "AUTOHAND_HOME"),
        (".grok/", "GROK_HOME"),
    ] {
        if let Some(tail) = rest.strip_prefix(prefix) {
            if let Some(base) = std::env::var_os(env).filter(|v| !v.is_empty()) {
                return PathBuf::from(base).join(tail);
            }
        }
    }

    if let Some(tail) = rest.strip_prefix(".config/") {
        return config_home().join(tail);
    }

    home().join(rest)
}

/// Render an absolute path back into `~`-relative form for display.
pub fn shorten(path: &Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Locate an executable on `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, &std::env::var_os("PATH")?)
}

/// Locate an executable in a `PATH`-style list of directories.
pub fn which_in(name: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}
