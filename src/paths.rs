//! Home-relative path resolution, matching the `skills` CLI's own rules.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Platform-specific application data resolution, separated from the environment
/// so tests do not change process-wide variables.
pub fn usage_data_dir_for(
    platform: &str,
    home: Option<&Path>,
    data_home: Option<&Path>,
    local_app_data: Option<&Path>,
) -> Result<PathBuf, String> {
    let absolute = |path: Option<&Path>| {
        path.filter(|path| path.is_absolute())
            .map(Path::to_path_buf)
    };
    let base = match platform {
        "windows" => absolute(local_app_data),
        "macos" => absolute(home).map(|home| home.join("Library/Application Support")),
        _ => absolute(data_home).or_else(|| absolute(home).map(|home| home.join(".local/share"))),
    };
    base.map(|base| base.join("skillshard")).ok_or_else(|| {
        "could not resolve an absolute application data directory for skill usage".into()
    })
}

pub fn usage_data_dir() -> Result<PathBuf, String> {
    let home = home();
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    usage_data_dir_for(
        std::env::consts::OS,
        Some(&home),
        data_home.as_deref(),
        local_app_data.as_deref(),
    )
}

pub fn usage_database_path() -> Result<PathBuf, String> {
    usage_data_dir().map(|directory| directory.join("usage.sqlite3"))
}

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

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn usage_data_respects_platform_locations_and_absolute_overrides() {
        let home = std::env::temp_dir().join("skillshard-path-test");
        let data_home = home.join("data");
        let local_app_data = home.join("local");
        assert_eq!(
            usage_data_dir_for("linux", Some(&home), None, None).unwrap(),
            home.join(".local/share/skillshard")
        );
        assert_eq!(
            usage_data_dir_for("linux", Some(&home), Some(&data_home), None).unwrap(),
            data_home.join("skillshard")
        );
        assert_eq!(
            usage_data_dir_for("macos", Some(&home), None, None).unwrap(),
            home.join("Library/Application Support/skillshard")
        );
        assert_eq!(
            usage_data_dir_for("windows", None, None, Some(&local_app_data)).unwrap(),
            local_app_data.join("skillshard")
        );
        assert!(usage_data_dir_for("linux", None, Some(Path::new("relative")), None).is_err());
        assert!(usage_data_dir_for("windows", Some(&home), None, None).is_err());
    }
}
