//! Git repositories the user added as skill sources, typically private ones.
//!
//! skills.sh only indexes public repositories, so a custom source is browsed by
//! shallow-cloning it with the user's own git setup — credential helpers and
//! SSH keys work exactly as they do in a terminal — and reading its `SKILL.md`
//! files. Installing still hands the source to `skills add`.

use crate::local_repos;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Where `owner/repo` shorthands live, matching the `skills` CLI.
const GITHUB: &str = "https://github.com";

/// Distinguishes clones started by the same process.
static CLONES: AtomicUsize = AtomicUsize::new(0);

/// A skill found in a custom source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSkill {
    pub name: String,
    pub description: String,
    /// The skill's folder within the repository, `/`-separated; empty for a
    /// skill at the root.
    pub folder: String,
}

/// The URL git should clone for `source`.
///
/// Accepts `owner/repo`, any URL git understands (`https://`, `ssh://`,
/// `git@host:path`), or a local path to a repository.
pub fn clone_url(source: &str) -> Result<String, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("enter a repository".into());
    }

    let is_url = source.contains("://") || source.starts_with("git@");
    if is_url || Path::new(source).is_absolute() {
        return Ok(source.to_string());
    }

    if is_owner_repo(source) {
        return Ok(format!("{GITHUB}/{source}.git"));
    }

    Err(format!(
        "“{source}” is not owner/repo or a git URL (https://…, ssh://… or git@host:path)"
    ))
}

fn is_owner_repo(source: &str) -> bool {
    let segments: Vec<&str> = source.split('/').collect();
    let is_segment = |segment: &&str| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    segments.len() == 2 && segments.iter().all(is_segment)
}

/// Every skill in `source`, by shallow-cloning it into a temporary folder.
///
/// Blocking and networked: call it from a background executor.
pub fn list_skills(source: &str) -> Result<Vec<SourceSkill>, String> {
    let url = clone_url(source)?;
    let clone = std::env::temp_dir().join(format!(
        "skillshard-source-{}-{}",
        std::process::id(),
        CLONES.fetch_add(1, Ordering::Relaxed)
    ));

    let listed = shallow_clone(&url, &clone).and_then(|()| skills_in(&clone));
    // git can leave a partial folder behind when a clone fails.
    if clone.exists() {
        std::fs::remove_dir_all(&clone)
            .map_err(|e| format!("could not remove {}: {e}", clone.display()))?;
    }
    listed
}

fn shallow_clone(url: &str, destination: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args(["clone", "--depth", "1", "--quiet", "--no-tags", url])
        .arg(destination)
        // There is no terminal to type a password into; fail with git's own
        // message instead of hanging.
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("could not clone {url}: {}", stderr.trim()))
}

fn skills_in(clone: &Path) -> Result<Vec<SourceSkill>, String> {
    let (skills, errors) = local_repos::discover(&[clone.to_path_buf()]);
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(skills
        .into_iter()
        .map(|skill| SourceSkill {
            folder: relative_folder(&skill.path, clone),
            name: skill.name,
            description: skill.description,
        })
        .collect())
}

fn relative_folder(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(PathBuf::from)
        .unwrap_or_default()
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_repo_is_cloned_from_github() {
        assert_eq!(
            clone_url(" acme/private-skills ").unwrap(),
            "https://github.com/acme/private-skills.git"
        );
    }

    #[test]
    fn urls_and_absolute_paths_are_cloned_as_given() {
        for source in [
            "https://gitlab.com/acme/skills.git",
            "ssh://git@example.com/acme/skills.git",
            "git@github.com:acme/skills.git",
            "/srv/git/skills.git",
        ] {
            assert_eq!(clone_url(source).unwrap(), source);
        }
    }

    #[test]
    fn anything_else_is_rejected() {
        for source in ["", "   ", "acme", "acme/skills/extra", "acme/sk ills"] {
            assert!(clone_url(source).is_err(), "{source:?} should be rejected");
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?} failed: {status:?}");
    }

    /// A committed repository with a skill in each of `folders`.
    fn repository(tag: &str, folders: &[&str]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("skillshard-custom-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for folder in folders {
            let name = folder.rsplit('/').next().unwrap();
            std::fs::create_dir_all(dir.join(folder)).unwrap();
            std::fs::write(
                dir.join(folder).join("SKILL.md"),
                format!("---\nname: {name}\ndescription: about {name}\n---\n"),
            )
            .unwrap();
        }
        git(&dir, &["init", "--quiet"]);
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "--quiet", "-m", "skills"]);
        dir
    }

    #[test]
    fn lists_the_skills_in_a_cloned_repository() {
        let repo = repository("listed", &["skills/pdf-tools", "git-helper"]);

        let skills = list_skills(&repo.display().to_string()).unwrap();

        assert_eq!(
            skills,
            [
                SourceSkill {
                    name: "git-helper".into(),
                    description: "about git-helper".into(),
                    folder: "git-helper".into(),
                },
                SourceSkill {
                    name: "pdf-tools".into(),
                    description: "about pdf-tools".into(),
                    folder: "skills/pdf-tools".into(),
                },
            ]
        );
    }

    #[test]
    fn a_failed_clone_reports_gits_reason() {
        let missing = std::env::temp_dir().join("skillshard-custom-does-not-exist");

        let error = list_skills(&missing.display().to_string()).unwrap_err();

        assert!(error.starts_with("could not clone"), "{error}");
        assert!(error.contains("does not exist"), "{error}");
    }
}
