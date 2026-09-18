//! Skills that live in folders on this machine ("local repositories").
//!
//! Each configured root is searched for `SKILL.md` files. Found skills are
//! installed by handing their directory to `skills add`, exactly like a remote
//! source.

use crate::frontmatter;
use std::path::{Path, PathBuf};

/// How far below a root to look. Deep enough for `repo/skills/<group>/<skill>`,
/// shallow enough that pointing at a home directory stays quick.
const MAX_DEPTH: usize = 5;

/// Directories that never hold skills but can be enormous.
const SKIPPED: &[&str] = &["node_modules", "target", "dist", "build", "vendor"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSkill {
    pub name: String,
    pub description: String,
    /// The skill directory, passed to `skills add`.
    pub path: PathBuf,
    /// The configured root it was found under.
    pub repository: PathBuf,
}

/// Find every skill under `roots`.
///
/// A root that cannot be read is reported rather than skipped silently; the
/// user configured it and should hear that it is not working.
pub fn discover(roots: &[PathBuf]) -> (Vec<LocalSkill>, Vec<String>) {
    let mut skills = Vec::new();
    let mut errors = Vec::new();
    for root in roots {
        if let Err(e) = std::fs::read_dir(root) {
            errors.push(format!("{}: {e}", root.display()));
            continue;
        }
        walk(root, root, 0, &mut skills);
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    (skills, errors)
}

fn walk(dir: &Path, root: &Path, depth: usize, out: &mut Vec<LocalSkill>) {
    let skill_file = dir.join("SKILL.md");
    if skill_file.is_file() {
        let fm = std::fs::read_to_string(&skill_file)
            .map(|text| frontmatter::parse(&text))
            .unwrap_or_default();
        let dir_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(LocalSkill {
            name: fm.name.unwrap_or(dir_name),
            description: fm.description.unwrap_or_default(),
            path: dir.to_path_buf(),
            repository: root.to_path_buf(),
        });
        // A skill's own subfolders are its resources, not further skills.
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    // Unreadable subfolders are not skills we could install anyway.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIPPED.contains(&name.as_ref()) {
            continue;
        }
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&entry.path(), root, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("skillshard-local-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn skill(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: about {name}\n---\n"),
        )
        .unwrap();
    }

    fn names(skills: &[LocalSkill]) -> Vec<&str> {
        skills.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn finds_skills_at_any_depth_within_the_limit() {
        let root = root("nested");
        skill(&root.join("top"), "top");
        skill(&root.join("skills/group/deep"), "deep");

        let (skills, errors) = discover(std::slice::from_ref(&root));
        assert!(errors.is_empty());
        assert_eq!(names(&skills), ["deep", "top"]);
        assert_eq!(skills[0].path, root.join("skills/group/deep"));
        assert_eq!(skills[0].repository, root);
        assert_eq!(skills[0].description, "about deep");
    }

    #[test]
    fn skips_hidden_and_build_directories() {
        let root = root("skipped");
        skill(&root.join(".git/hooks-skill"), "hidden");
        skill(&root.join("node_modules/pkg"), "dependency");
        skill(&root.join("real"), "real");

        assert_eq!(names(&discover(&[root]).0), ["real"]);
    }

    #[test]
    fn does_not_treat_a_skills_own_folders_as_skills() {
        let root = root("resources");
        skill(&root.join("outer"), "outer");
        skill(&root.join("outer/examples/inner"), "inner");

        assert_eq!(names(&discover(&[root]).0), ["outer"]);
    }

    #[test]
    fn stops_at_the_depth_limit() {
        let root = root("depth");
        // Separate branches, so neither is hidden by being inside the other.
        skill(&root.join("a/b/c/d/e"), "just-fits");
        skill(&root.join("x/b/c/d/e/too-deep"), "too-deep");

        assert_eq!(names(&discover(&[root]).0), ["just-fits"]);
    }

    #[test]
    fn reports_a_missing_root() {
        let missing = std::env::temp_dir().join("skillshard-local-does-not-exist");
        let (skills, errors) = discover(&[missing]);
        assert!(skills.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("skillshard-local-does-not-exist"));
    }
}
