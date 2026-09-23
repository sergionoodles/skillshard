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
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub lines: Vec<DiffLine>,
    pub is_binary: bool,
}

#[derive(Debug, Deserialize)]
struct TreeResponse {
    sha: String,
    #[serde(default)]
    tree: Vec<TreeEntry>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
}

/// Repository trees fetched during one update check, for reuse by previews.
#[derive(Default)]
pub struct TreeCache {
    trees: BTreeMap<(String, String), Result<Arc<TreeResponse>, String>>,
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

fn upstream_tree<'a>(
    entry: &'a LockEntry,
    cache: Option<&TreeCache>,
) -> Result<(&'a str, String, Arc<TreeResponse>), String> {
    upstream_tree_with(entry, cache, fetch_tree)
}

fn upstream_tree_with<'a>(
    entry: &'a LockEntry,
    cache: Option<&TreeCache>,
    mut fetch: impl FnMut(&str, &str) -> Result<TreeResponse, String>,
) -> Result<(&'a str, String, Arc<TreeResponse>), String> {
    let owner_repo = entry.github_owner_repo().ok_or("not a GitHub skill")?;
    let refs: Vec<&str> = entry
        .git_ref
        .as_deref()
        .filter(|git_ref| !git_ref.is_empty())
        .map(|git_ref| vec![git_ref])
        .unwrap_or_else(|| vec!["main", "master"]);
    let mut last_error = String::new();
    for git_ref in refs {
        let cached = cache.and_then(|cache| {
            cache
                .trees
                .get(&(owner_repo.to_string(), git_ref.to_string()))
        });
        let result = match cached {
            Some(result) => result.clone(),
            None => fetch(owner_repo, git_ref).map(Arc::new),
        };
        match result {
            Ok(tree) if !tree.truncated => return Ok((owner_repo, git_ref.to_string(), tree)),
            Ok(_) => return Err("upstream file list is incomplete".into()),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Compare the installed skill folder with the files currently upstream.
pub fn preview(entry: &LockEntry, installed: &Path) -> Result<Vec<FileDiff>, String> {
    preview_with_cache(entry, installed, None)
}

/// Preview using trees already fetched by an update check when available.
pub fn preview_with_cache(
    entry: &LockEntry,
    installed: &Path,
    cache: Option<&TreeCache>,
) -> Result<Vec<FileDiff>, String> {
    let skill_path = entry.skill_path.as_deref().ok_or("skill path is missing")?;
    let (owner_repo, git_ref, tree) = upstream_tree(entry, cache)?;
    let folder = folder_path(skill_path);
    if folder_sha(&tree, skill_path).is_none() {
        return Err("skill is no longer present upstream".into());
    }
    let mut files = local_files(installed)?;
    let mut changes = Vec::new();
    for remote in tree.tree.iter().filter(|item| item.kind == "blob") {
        let relative = if folder.is_empty() {
            remote.path.as_str()
        } else if let Some(relative) = remote.path.strip_prefix(&format!("{folder}/")) {
            relative
        } else {
            continue;
        };
        if !safe_relative_path(relative) {
            return Err(format!("unsafe upstream path: {relative}"));
        }
        let url = format!(
            "https://raw.githubusercontent.com/{owner_repo}/{}/{}",
            encode_path(&git_ref),
            encode_path(&remote.path)
        );
        let upstream = fetch_bytes(&url)?;
        if let Some(change) =
            file_diff(relative.to_string(), files.remove(relative), Some(upstream))
        {
            changes.push(change);
        }
    }
    changes.extend(
        files
            .into_iter()
            .filter_map(|(path, current)| file_diff(path, Some(current), None)),
    );
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(changes)
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let mut response = ureq::get(url)
        .header(
            "User-Agent",
            concat!("skillshard/", env!("CARGO_PKG_VERSION")),
        )
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .call()
        .map_err(|error| error.to_string())?;
    response
        .body_mut()
        .read_to_vec()
        .map_err(|error| error.to_string())
}

fn encode_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

fn local_files(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let path = entry.path();
                let relative = path.strip_prefix(root).map_err(|error| error.to_string())?;
                files.insert(
                    relative.to_string_lossy().replace('\\', "/"),
                    std::fs::read(path).map_err(|error| error.to_string())?,
                );
            }
        }
    }
    Ok(files)
}

fn file_diff(
    path: String,
    current: Option<Vec<u8>>,
    upstream: Option<Vec<u8>>,
) -> Option<FileDiff> {
    if current == upstream {
        return None;
    }
    let is_binary = [&current, &upstream]
        .into_iter()
        .flatten()
        .any(|bytes| bytes.contains(&0) || std::str::from_utf8(bytes).is_err());
    if is_binary {
        return Some(FileDiff {
            path,
            lines: Vec::new(),
            is_binary: true,
        });
    }
    let current = current
        .as_deref()
        .map(|bytes| std::str::from_utf8(bytes).unwrap_or(""))
        .unwrap_or("");
    let upstream = upstream
        .as_deref()
        .map(|bytes| std::str::from_utf8(bytes).unwrap_or(""))
        .unwrap_or("");
    Some(FileDiff {
        path,
        lines: diff_lines(current, upstream),
        is_binary: false,
    })
}

fn diff_lines(current: &str, upstream: &str) -> Vec<DiffLine> {
    let old: Vec<&str> = current.split_inclusive('\n').collect();
    let new: Vec<&str> = upstream.split_inclusive('\n').collect();
    let width = new.len() + 1;
    // ponytail: Large files get a coarse diff; use a linear-space algorithm if they become common.
    if old.len().saturating_mul(new.len()) > 1_000_000 {
        return old
            .iter()
            .map(|line| DiffLine::Removed(display_line(line)))
            .chain(new.iter().map(|line| DiffLine::Added(display_line(line))))
            .collect();
    }
    let mut lengths = vec![0usize; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lengths[i * width + j] = if old[i] == new[j] {
                1 + lengths[(i + 1) * width + j + 1]
            } else {
                lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut lines = Vec::new();
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            lines.push(DiffLine::Context(display_line(old[i])));
            i += 1;
            j += 1;
        } else if j < new.len()
            && (i == old.len() || lengths[i * width + j + 1] > lengths[(i + 1) * width + j])
        {
            lines.push(DiffLine::Added(display_line(new[j])));
            j += 1;
        } else {
            lines.push(DiffLine::Removed(display_line(old[i])));
            i += 1;
        }
    }
    lines
}

fn display_line(line: &str) -> String {
    match line.strip_suffix('\n') {
        Some(text) => text.to_string(),
        None => format!("{line} [no newline]"),
    }
}

/// Check whether `entry` has a newer version upstream.
///
/// Only GitHub-sourced skills with a recorded tree SHA can be checked; for
/// anything else the state is [`UpdateState::NotTracked`], which the UI shows
/// as "check with the CLI" rather than pretending it is current.
pub fn check(entry: &LockEntry) -> UpdateState {
    check_with_cache(entry, &mut TreeCache::default(), &mut fetch_tree)
}

/// Check a set of skills, fetching each repository tree only once per ref.
pub fn check_many(entries: Vec<(String, LockEntry)>) -> (Vec<(String, UpdateState)>, TreeCache) {
    check_many_with(entries, fetch_tree)
}

fn check_many_with(
    entries: Vec<(String, LockEntry)>,
    mut fetch: impl FnMut(&str, &str) -> Result<TreeResponse, String>,
) -> (Vec<(String, UpdateState)>, TreeCache) {
    let mut cache = TreeCache::default();
    let states = entries
        .into_iter()
        .map(|(name, entry)| {
            let state = check_with_cache(&entry, &mut cache, &mut fetch);
            (name, state)
        })
        .collect();
    (states, cache)
}

fn check_with_cache(
    entry: &LockEntry,
    cache: &mut TreeCache,
    fetch: &mut impl FnMut(&str, &str) -> Result<TreeResponse, String>,
) -> UpdateState {
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
    let refs: Vec<&str> = match entry.git_ref.as_deref() {
        Some(git_ref) if !git_ref.is_empty() => vec![git_ref],
        _ => vec!["main", "master"],
    };

    let mut last_error = None;
    for git_ref in refs {
        let tree = cache
            .trees
            .entry((owner_repo.to_string(), git_ref.to_string()))
            .or_insert_with(|| fetch(owner_repo, git_ref).map(Arc::new));
        match tree {
            Ok(tree) => {
                return match folder_sha(tree, skill_path) {
                    Some(upstream) if &upstream == locked_hash => UpdateState::UpToDate,
                    Some(_) => UpdateState::Available,
                    // The folder is gone upstream: the skill was moved or
                    // removed, which an update would have to resolve.
                    None => UpdateState::Failed("no longer present upstream".into()),
                };
            }
            Err(error) => last_error = Some(error.clone()),
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
            truncated: false,
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

    #[test]
    fn checks_shared_repository_trees_once_per_ref() {
        let entry = LockEntry {
            source: "owner/repo".into(),
            source_type: "github".into(),
            source_url: None,
            skill_path: Some("skills/ponytail/SKILL.md".into()),
            git_ref: None,
            hash: Some("cb6e534".into()),
            hash_is_tree_sha: true,
            installed_at: None,
            updated_at: None,
        };
        let mut stale = entry.clone();
        stale.hash = Some("old".into());
        let mut pinned = entry.clone();
        pinned.git_ref = Some("release".into());
        let mut other_repo = entry.clone();
        other_repo.source = "other/repo".into();
        let mut untracked = entry.clone();
        untracked.hash_is_tree_sha = false;

        let mut calls = Vec::new();
        let (states, cache) = check_many_with(
            vec![
                ("current".into(), entry.clone()),
                ("stale".into(), stale),
                ("pinned".into(), pinned),
                ("other".into(), other_repo),
                ("untracked".into(), untracked),
            ],
            |repo, git_ref| {
                calls.push((repo.to_string(), git_ref.to_string()));
                if git_ref == "main" {
                    Err("missing branch".into())
                } else {
                    Ok(tree())
                }
            },
        );

        assert_eq!(
            states,
            vec![
                ("current".into(), UpdateState::UpToDate),
                ("stale".into(), UpdateState::Available),
                ("pinned".into(), UpdateState::UpToDate),
                ("other".into(), UpdateState::UpToDate),
                ("untracked".into(), UpdateState::NotTracked),
            ]
        );
        assert_eq!(
            calls,
            vec![
                ("owner/repo".into(), "main".into()),
                ("owner/repo".into(), "master".into()),
                ("owner/repo".into(), "release".into()),
                ("other/repo".into(), "main".into()),
                ("other/repo".into(), "master".into()),
            ]
        );
        let (_, git_ref, cached_tree) =
            upstream_tree_with(&entry, Some(&cache), |_, _| panic!("unexpected request")).unwrap();
        assert_eq!(git_ref, "master");
        assert_eq!(
            folder_sha(&cached_tree, entry.skill_path.as_deref().unwrap()).as_deref(),
            Some("cb6e534")
        );
    }

    #[test]
    fn preview_lines_keep_context_and_show_insertions_and_removals() {
        assert_eq!(
            diff_lines("first\nold\nlast\n", "first\nnew\nlast\n"),
            vec![
                DiffLine::Context("first".into()),
                DiffLine::Removed("old".into()),
                DiffLine::Added("new".into()),
                DiffLine::Context("last".into()),
            ]
        );
    }

    #[test]
    fn preview_handles_added_removed_and_binary_files() {
        let added = file_diff("new.txt".into(), None, Some(b"hello\n".to_vec())).unwrap();
        assert_eq!(added.lines, vec![DiffLine::Added("hello".into())]);
        let removed = file_diff("gone.txt".into(), Some(b"bye\n".to_vec()), None).unwrap();
        assert_eq!(removed.lines, vec![DiffLine::Removed("bye".into())]);
        let binary = file_diff("image.png".into(), Some(vec![0, 1]), Some(vec![0, 2])).unwrap();
        assert!(binary.is_binary);
        assert!(file_diff("same".into(), Some(b"x".to_vec()), Some(b"x".to_vec())).is_none());
    }

    #[test]
    fn preview_shows_a_changed_final_newline() {
        assert_eq!(
            diff_lines("hello", "hello\n"),
            vec![
                DiffLine::Removed("hello [no newline]".into()),
                DiffLine::Added("hello".into()),
            ]
        );
    }

    #[test]
    fn preview_rejects_a_non_github_source() {
        let entry = LockEntry {
            source: "./local".into(),
            source_type: "local".into(),
            source_url: None,
            skill_path: Some("SKILL.md".into()),
            git_ref: None,
            hash: None,
            hash_is_tree_sha: false,
            installed_at: None,
            updated_at: None,
        };
        assert_eq!(
            preview(&entry, Path::new(".")),
            Err("not a GitHub skill".into())
        );
    }
}
