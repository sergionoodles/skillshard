//! Code editors a skill folder can be opened in.

use crate::paths;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// Editors worth offering, each with the command-line launchers it installs,
/// in order of preference.
const KNOWN: &[(&str, &[&str])] = &[
    ("VS Code", &["code"]),
    ("VSCodium", &["codium"]),
    ("Cursor", &["cursor"]),
    ("Windsurf", &["windsurf"]),
    // Arch and some other distributions package Zed's CLI as `zeditor`.
    ("Zed", &["zed", "zeditor"]),
    ("Sublime Text", &["subl"]),
];

/// An editor found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    pub name: String,
    pub program: PathBuf,
}

impl Editor {
    /// Open `dir` as a folder in the editor.
    ///
    /// Blocking until the launcher exits, which for these CLIs is as soon as
    /// the window is handed off: call it from a background executor.
    pub fn open(&self, dir: &Path) -> io::Result<ExitStatus> {
        Command::new(&self.program)
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    }
}

/// Editors installed on this machine, found through their launchers on `PATH`.
pub fn detect() -> Vec<Editor> {
    std::env::var_os("PATH")
        .map(|path| detect_in(&path))
        .unwrap_or_default()
}

/// Editors with a launcher in the `PATH`-style list `path`.
pub fn detect_in(path: &OsStr) -> Vec<Editor> {
    KNOWN
        .iter()
        .filter_map(|(name, launchers)| {
            let program = launchers.iter().find_map(|l| paths::which_in(l, path))?;
            Some(Editor {
                name: name.to_string(),
                program,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin_dir(tag: &str, launchers: &[&str]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("skillshard-editors-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for launcher in launchers {
            std::fs::write(dir.join(launcher), "").unwrap();
        }
        dir
    }

    #[test]
    fn only_editors_with_a_launcher_on_the_path_are_found() {
        let dir = bin_dir("found", &["zed", "code", "unrelated"]);
        let names: Vec<_> = detect_in(dir.as_os_str())
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["VS Code", "Zed"]);
    }

    #[test]
    fn an_alternative_launcher_name_is_found() {
        let dir = bin_dir("alias", &["zeditor"]);
        assert_eq!(
            detect_in(dir.as_os_str()),
            [Editor {
                name: "Zed".into(),
                program: dir.join("zeditor"),
            }]
        );
    }

    #[test]
    fn nothing_is_found_on_an_empty_path() {
        assert!(detect_in(OsStr::new("")).is_empty());
    }
}
