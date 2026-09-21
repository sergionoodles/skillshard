//! Read-only update checking against the GitHub trees API.
//!
//! The `skills` CLI has no "is anything outdated?" command — `skills check` is
//! an alias for `update`, which performs the update. So that Skillshard can
//! *show* update status without changing anything, it repeats the CLI's own
//! comparison: the global lock records the GitHub tree SHA of each skill's
//! folder, and a differing SHA upstream means the skill has moved on.
//!
//! Applying an update is still delegated to `skills update`.

use crate::model::{LockEntry, UpdateState};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct TreeResponse {
    sha: String,
    #[serde(default)]
    tree: Vec<TreeEntry>,
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
}

/// Strip a trailing `SKILL.md` to get the skill's folder path within the repo.
///
/// Mirrors the CLI's `getSkillFolderHashFromTree`.
fn folder_path(skill_path: &str) -> String {
    let normalized = skill_path.replace('\\', "/");
    let lower = normalized.to_lowercase();
    let trimmed = if lower.ends_with("/skill.md") {
        &normalized[..normalized.len() - 9]
    } else if lower.ends_with("skill.md") {
        &normalized[..normalized.len() - 8]
    } else {
        &normalized[..]
    };
    trimmed.trim_end_matches('/').to_string()
}

/// Find the tree SHA for a skill folder within a repository tree.
fn folder_sha(tree: &TreeResponse, skill_path: &str) -> Option<String> {
    let folder = folder_path(skill_path);
    // A skill at the repository root is the whole tree.
    if folder.is_empty() {
        return Some(tree.sha.clone());
    }
    tree.tree
        .iter()
        .find(|e| e.kind == "tree" && e.path == folder)
        .map(|e| e.sha.clone())
}

/// Fetch a repository tree for one ref.
fn fetch_tree(owner_repo: &str, git_ref: &str) -> Result<TreeResponse, String> {
    let url = format!("https://api.github.com/repos/{owner_repo}/git/trees/{git_ref}?recursive=1");
    let body = crate::registry::get(&url)?;
    serde_json::from_str(&body).map_err(|e| format!("unexpected tree response: {e}"))
}

/// Check whether `entry` has a newer version upstream.
///
/// Only GitHub-sourced skills with a recorded tree SHA can be checked; for
/// anything else the state is [`UpdateState::NotTracked`], which the UI shows
/// as "check with the CLI" rather than pretending it is current.
pub fn check(entry: &LockEntry) -> UpdateState {
    let Some(owner_repo) = entry.github_owner_repo() else {
        return UpdateState::NotTracked;
    };
    let (Some(locked_hash), Some(skill_path)) = (&entry.hash, &entry.skill_path) else {
        return UpdateState::NotTracked;
    };
    if !entry.hash_is_tree_sha {
        // Project locks store a content hash of local files, which cannot be
        // compared against a GitHub tree SHA.
        return UpdateState::NotTracked;
    }

    // Honour a pinned ref, otherwise try the usual default branches.
    let refs: Vec<String> = match &entry.git_ref {
        Some(r) if !r.is_empty() => vec![r.clone()],
        _ => vec!["main".into(), "master".into()],
    };

    let mut last_error = None;
    for git_ref in refs {
        match fetch_tree(owner_repo, &git_ref) {
            Ok(tree) => {
                return match folder_sha(&tree, skill_path) {
                    Some(upstream) if &upstream == locked_hash => UpdateState::UpToDate,
                    Some(_) => UpdateState::Available,
                    // The folder is gone upstream: the skill was moved or
                    // removed, which an update would have to resolve.
                    None => UpdateState::Failed("no longer present upstream".into()),
                };
            }
            Err(e) => last_error = Some(e),
        }
    }
    UpdateState::Failed(last_error.unwrap_or_else(|| "could not reach GitHub".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> TreeResponse {
        TreeResponse {
            sha: "roottree".into(),
            tree: vec![
                TreeEntry {
                    path: "skills".into(),
                    kind: "tree".into(),
                    sha: "skillsdir".into(),
                },
                TreeEntry {
                    path: "skills/ponytail".into(),
                    kind: "tree".into(),
                    sha: "cb6e534".into(),
                },
                TreeEntry {
                    path: "skills/ponytail/SKILL.md".into(),
                    kind: "blob".into(),
                    sha: "blobsha".into(),
                },
            ],
        }
    }

    #[test]
    fn strips_the_skill_file_to_get_the_folder() {
        assert_eq!(folder_path("skills/ponytail/SKILL.md"), "skills/ponytail");
        assert_eq!(folder_path("skills/ponytail/"), "skills/ponytail");
        assert_eq!(folder_path("SKILL.md"), "");
    }

    #[test]
    fn finds_the_folder_sha_not_the_file_sha() {
        assert_eq!(
            folder_sha(&tree(), "skills/ponytail/SKILL.md").as_deref(),
            Some("cb6e534")
        );
    }

    #[test]
    fn a_root_level_skill_uses_the_whole_tree() {
        assert_eq!(folder_sha(&tree(), "SKILL.md").as_deref(), Some("roottree"));
    }

    #[test]
    fn a_missing_folder_has_no_sha() {
        assert_eq!(folder_sha(&tree(), "skills/gone/SKILL.md"), None);
    }

    #[test]
    fn non_github_and_content_hashed_sources_are_not_tracked() {
        let local = LockEntry {
            source: "./local".into(),
            source_type: "local".into(),
            source_url: None,
            skill_path: Some("SKILL.md".into()),
            git_ref: None,
            hash: Some("abc".into()),
            hash_is_tree_sha: false,
            installed_at: None,
            updated_at: None,
        };
        assert_eq!(check(&local), UpdateState::NotTracked);

        // A GitHub source whose hash is a content hash (project lock) cannot
        // be compared against a tree SHA either.
        let project = LockEntry {
            source: "owner/repo".into(),
            source_type: "github".into(),
            ..local.clone()
        };
        assert_eq!(check(&project), UpdateState::NotTracked);
    }
}
