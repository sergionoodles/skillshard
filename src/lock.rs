//! Readers for the two lock files the `skills` CLI maintains.
//!
//! * Global: `$XDG_STATE_HOME/skills/.skill-lock.json`, else
//!   `~/.agents/.skill-lock.json`. Version 3, keyed by `skillFolderHash`
//!   (a GitHub tree SHA).
//! * Project: `<root>/skills-lock.json`. Version 1, keyed by `computedHash`
//!   (a SHA-256 over the skill's files).
//!
//! Skillshard only reads these; the CLI stays the sole writer.

use crate::model::{LockEntry, Scope};
use crate::paths;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlobalLockFile {
    #[serde(default)]
    skills: BTreeMap<String, GlobalLockEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlobalLockEntry {
    source: String,
    source_type: String,
    source_url: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    skill_path: Option<String>,
    skill_folder_hash: Option<String>,
    installed_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectLockFile {
    #[serde(default)]
    skills: BTreeMap<String, ProjectLockEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectLockEntry {
    source: String,
    source_type: String,
    source_url: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    skill_path: Option<String>,
    computed_hash: Option<String>,
}

/// Path of the global lock file, honouring `$XDG_STATE_HOME`.
pub fn global_lock_path() -> PathBuf {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("skills/.skill-lock.json"),
        _ => paths::home().join(".agents/.skill-lock.json"),
    }
}

/// Path of a project's lock file.
pub fn project_lock_path(root: &std::path::Path) -> PathBuf {
    root.join("skills-lock.json")
}

/// Read the lock entries for `scope`, keyed by skill name.
///
/// A missing or malformed lock file yields an empty map: the filesystem, not
/// the lock, is the source of truth for what is installed.
pub fn read(scope: &Scope) -> BTreeMap<String, LockEntry> {
    match scope {
        Scope::Global => read_global(&global_lock_path()),
        Scope::Project(root) => read_project(&project_lock_path(root)),
    }
}

fn read_global(path: &std::path::Path) -> BTreeMap<String, LockEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(file) = serde_json::from_str::<GlobalLockFile>(&text) else {
        return BTreeMap::new();
    };
    file.skills
        .into_iter()
        .map(|(name, e)| {
            (
                name,
                LockEntry {
                    source: e.source,
                    source_type: e.source_type,
                    source_url: e.source_url,
                    skill_path: e.skill_path,
                    git_ref: e.git_ref,
                    hash: e.skill_folder_hash,
                    hash_is_tree_sha: true,
                    installed_at: e.installed_at,
                    updated_at: e.updated_at,
                },
            )
        })
        .collect()
}

fn read_project(path: &std::path::Path) -> BTreeMap<String, LockEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(file) = serde_json::from_str::<ProjectLockFile>(&text) else {
        return BTreeMap::new();
    };
    file.skills
        .into_iter()
        .map(|(name, e)| {
            (
                name,
                LockEntry {
                    source: e.source,
                    source_type: e.source_type,
                    source_url: e.source_url,
                    skill_path: e.skill_path,
                    git_ref: e.git_ref,
                    hash: e.computed_hash,
                    hash_is_tree_sha: false,
                    installed_at: None,
                    updated_at: None,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_v3_global_lock() {
        let dir = tempdir();
        let path = dir.join("lock.json");
        std::fs::write(
            &path,
            r#"{"version":3,"skills":{"ponytail":{"source":"dietrichgebert/ponytail",
               "sourceType":"github","sourceUrl":"https://github.com/dietrichgebert/ponytail.git",
               "skillPath":"skills/ponytail/SKILL.md","skillFolderHash":"cb6e534",
               "installedAt":"2026-09-14T12:20:50.816Z","updatedAt":"2026-09-16T15:40:46.653Z"}}}"#,
        )
        .unwrap();

        let entries = read_global(&path);
        let entry = &entries["ponytail"];
        assert_eq!(entry.source, "dietrichgebert/ponytail");
        assert_eq!(entry.hash.as_deref(), Some("cb6e534"));
        assert!(entry.hash_is_tree_sha);
        assert_eq!(entry.github_owner_repo(), Some("dietrichgebert/ponytail"));
    }

    #[test]
    fn reads_a_project_lock_content_hash() {
        let dir = tempdir();
        let path = dir.join("skills-lock.json");
        std::fs::write(
            &path,
            r#"{"version":1,"skills":{"my-skill":{"source":"./local","sourceType":"local",
               "computedHash":"abc123"}}}"#,
        )
        .unwrap();

        let entry = &read_project(&path)["my-skill"];
        assert_eq!(entry.hash.as_deref(), Some("abc123"));
        assert!(!entry.hash_is_tree_sha);
        // Only GitHub sources can be checked against the trees API.
        assert_eq!(entry.github_owner_repo(), None);
    }

    #[test]
    fn a_missing_or_corrupt_lock_reads_as_empty() {
        let dir = tempdir();
        assert!(read_global(&dir.join("absent.json")).is_empty());
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "not json at all").unwrap();
        assert!(read_global(&bad).is_empty());
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "skillshard-lock-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
