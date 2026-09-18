//! User preferences, persisted to `$XDG_CONFIG_HOME/skillshard/settings.json`.
//!
//! Held as a GPUI global so the settings modal can read and write them from
//! plain `&App` closures, and so any view can observe changes.

use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Light or dark, or whatever the system is using.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// A project shown in the sidebar, and how it looks there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub path: PathBuf,
    /// Icon name from the bundled Lucide set; `None` shows a folder.
    #[serde(default)]
    pub icon: Option<String>,
    /// Palette colour name, e.g. `"blue"`; `None` uses the text colour.
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// Fields missing from an older file take their defaults instead of failing.
#[serde(default)]
pub struct Preferences {
    pub appearance: Appearance,
    pub light_theme: String,
    pub dark_theme: String,
    /// Default scope in the install dialog.
    pub install_global: bool,
    /// Agents pre-selected in the install dialog; empty means every agent
    /// already in use.
    pub install_agents: Vec<String>,
    pub install_copy: bool,
    /// Folders searched for skills that live on this machine.
    pub local_repositories: Vec<PathBuf>,
    pub check_updates_on_startup: bool,
    /// Projects kept in the sidebar between runs.
    pub projects: Vec<Project>,
}

impl Preferences {
    /// The entry for the project at `path`, added if it is not there yet.
    pub fn project_mut(&mut self, path: &Path) -> &mut Project {
        let index = match self.projects.iter().position(|p| p.path == path) {
            Some(index) => index,
            None => {
                self.projects.push(Project {
                    path: path.to_path_buf(),
                    icon: None,
                    color: None,
                });
                self.projects.len() - 1
            }
        };
        &mut self.projects[index]
    }

    pub fn project(&self, path: &Path) -> Option<&Project> {
        self.projects.iter().find(|p| p.path == path)
    }

    pub fn remove_project(&mut self, path: &Path) {
        self.projects.retain(|p| p.path != path);
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: Appearance::System,
            light_theme: "Catppuccin Latte".into(),
            dark_theme: "Catppuccin Mocha".into(),
            install_global: true,
            install_agents: Vec::new(),
            install_copy: false,
            local_repositories: Vec::new(),
            check_updates_on_startup: true,
            projects: Vec::new(),
        }
    }
}

/// Where preferences are stored.
pub fn default_path() -> PathBuf {
    crate::paths::expand("~/.config/skillshard/settings.json")
}

/// Read preferences from `path`. A missing file is not an error: it means
/// nothing has been changed from the defaults yet.
pub fn load(path: &Path) -> Result<Preferences, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("could not read {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

pub fn save(prefs: &Preferences, path: &Path) -> Result<(), String> {
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(prefs)?)
    };
    write().map_err(|e| format!("could not save {}: {e}", path.display()))
}

/// The preferences global: current values, where they live, and the last
/// problem reading or writing them, for the main window to report.
pub struct Store {
    pub values: Preferences,
    pub path: PathBuf,
    pub error: Option<String>,
}

impl Global for Store {}

/// Load preferences from `path` into the global store.
///
/// An unreadable file falls back to defaults, with the reason kept in
/// `Store::error` rather than dropped.
pub fn init(path: PathBuf, cx: &mut App) {
    let (values, error) = match load(&path) {
        Ok(values) => (values, None),
        Err(e) => (Preferences::default(), Some(e)),
    };
    cx.set_global(Store {
        values,
        path,
        error,
    });
}

pub fn get(cx: &App) -> &Preferences {
    &cx.global::<Store>().values
}

/// Change preferences and write them to disk.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Preferences)) {
    let store = cx.global_mut::<Store>();
    change(&mut store.values);
    store.error = save(&store.values, &store.path).err();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("skillshard-prefs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested/settings.json")
    }

    #[test]
    fn round_trips_through_disk() {
        let path = temp_file("roundtrip");
        let prefs = Preferences {
            appearance: Appearance::Dark,
            dark_theme: "Tokyo Night".into(),
            install_agents: vec!["claude-code".into()],
            local_repositories: vec![PathBuf::from("/src/skills")],
            projects: vec![Project {
                path: PathBuf::from("/work/app"),
                icon: Some("rocket".into()),
                color: Some("blue".into()),
            }],
            ..Preferences::default()
        };
        save(&prefs, &path).unwrap();
        assert_eq!(load(&path).unwrap(), prefs);
    }

    #[test]
    fn project_entries_are_created_once_and_can_be_removed() {
        let mut prefs = Preferences::default();
        let path = Path::new("/work/app");
        prefs.project_mut(path).icon = Some("rocket".into());
        prefs.project_mut(path).color = Some("blue".into());
        assert_eq!(prefs.projects.len(), 1, "the second call reuses the entry");
        assert_eq!(prefs.project(path).unwrap().icon.as_deref(), Some("rocket"));

        prefs.remove_project(path);
        assert!(prefs.project(path).is_none());
    }

    #[test]
    fn a_missing_file_means_defaults() {
        assert_eq!(load(&temp_file("missing")).unwrap(), Preferences::default());
    }

    #[test]
    fn fields_absent_from_an_older_file_take_their_defaults() {
        let path = temp_file("partial");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"appearance":"light"}"#).unwrap();
        let prefs = load(&path).unwrap();
        assert_eq!(prefs.appearance, Appearance::Light);
        assert_eq!(prefs.dark_theme, "Catppuccin Mocha");
    }

    #[test]
    fn a_corrupt_file_is_reported_not_ignored() {
        let path = temp_file("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let err = load(&path).unwrap_err();
        assert!(err.contains("settings.json"), "{err}");
    }
}
