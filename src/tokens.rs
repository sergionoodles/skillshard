//! Estimates what a skill costs an agent in context tokens.
//!
//! A skill is paid for in three separate places, and conflating them hides
//! the thing worth knowing:
//!
//! * the front-matter description sits in the system prompt of every session
//!   the skill is enabled for, whether or not it is ever used;
//! * the `SKILL.md` body is read only once the skill triggers;
//! * anything else in the skill directory is progressive disclosure — loaded
//!   only if the agent decides to open it, so it is a ceiling, not a cost.
//!
//! Claude's tokenizer is not public, so these are estimates. The alternative
//! tokenizers on crates.io are OpenAI's BPE, which undercounts Claude by
//! 15-20% on prose and considerably more on code — worse than the ratio below
//! while also being a dependency.

use crate::frontmatter;
use std::path::Path;

/// Characters per token for the markdown skills are written in.
///
/// Measured against Claude's tokenizer on English prose with light markup.
/// Code- and YAML-dense skills pack more tokens per character, so their real
/// cost runs above this estimate.
const CHARS_PER_TOKEN: f32 = 3.7;

/// How deep to walk a skill directory when totalling its bundled resources.
///
/// Skills nest a `references/` or `scripts/` directory at most; the limit is
/// really here so a symlink cycle cannot spin forever.
const MAX_DEPTH: usize = 8;

/// Full scale of the description bar: a description this long crowds the
/// system prompt of every session, used or not.
pub const DESCRIPTION_BUDGET: u32 = 500;
/// Full scale of the body bar.
pub const BODY_BUDGET: u32 = 5_000;
/// Full scale of the bundled-resources bar.
pub const RESOURCES_BUDGET: u32 = 20_000;
/// Full scale for a scope's combined always-on cost.
///
/// Nothing enforces this — it is the point at which a skill collection is
/// eating a noticeable slice of every session's system prompt.
pub const SCOPE_ALWAYS_BUDGET: u32 = 10_000;

/// What one skill costs, in estimated tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenCost {
    /// Front-matter name and description — paid in every session.
    pub description: u32,
    /// The `SKILL.md` body — paid when the skill triggers.
    pub body: u32,
    /// Every other file in the skill directory — paid only if read.
    pub resources: u32,
}

impl TokenCost {
    /// Whether anything was measured at all.
    ///
    /// A skill recorded in the lock file but missing from disk has no cost to
    /// show.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Estimate the token count of `text`.
pub fn estimate(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    (text.chars().count() as f32 / CHARS_PER_TOKEN).ceil() as u32
}

/// Measure the skill directory at `path`.
///
/// Returns an empty cost when the directory has no readable `SKILL.md`.
pub fn measure(path: &Path) -> TokenCost {
    let Ok(source) = std::fs::read_to_string(path.join("SKILL.md")) else {
        return TokenCost::default();
    };
    let front = frontmatter::parse(&source);

    // The agent sees the name alongside the description, so both are part of
    // what the skill costs just by being enabled.
    let name = front.name.as_deref().unwrap_or_default();
    let description = front.description.as_deref().unwrap_or_default();

    TokenCost {
        description: estimate(name) + estimate(description),
        body: estimate(frontmatter::body(&source)),
        resources: resources(path, 0),
    }
}

/// Total the text files in `dir`, excluding the top-level `SKILL.md`.
///
/// Files that are not valid UTF-8 are skipped: an agent does not read a
/// binary into its context.
fn resources(dir: &Path, depth: usize) -> u32 {
    if depth >= MAX_DEPTH {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || (depth == 0 && name == "SKILL.md") {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            total += resources(&path, depth + 1);
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            total += estimate(&text);
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("skillshard-tokens-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_three_costs_are_counted_separately() {
        let dir = fixture("split");
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: demo\ndescription: A short description.\n---\n\nThe body of the skill.\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::write(dir.join("references/extra.md"), "Extra reference material.").unwrap();

        let cost = measure(&dir);
        assert_eq!(
            cost.description,
            estimate("demo") + estimate("A short description.")
        );
        assert_eq!(cost.body, estimate("The body of the skill.\n"));
        assert_eq!(cost.resources, estimate("Extra reference material."));
    }

    /// The whole point of the resources figure: a skill whose bytes are mostly
    /// in files loaded on demand must not have them counted as trigger cost.
    #[test]
    fn bundled_files_are_not_charged_to_the_body() {
        let dir = fixture("disclosure");
        std::fs::write(dir.join("SKILL.md"), "---\ndescription: d\n---\n\nShort.\n").unwrap();
        std::fs::write(dir.join("long.md"), "x".repeat(10_000)).unwrap();

        let cost = measure(&dir);
        assert!(cost.body < 10);
        assert!(cost.resources > 2_000);
    }

    #[test]
    fn a_directory_without_a_skill_file_costs_nothing() {
        let dir = fixture("missing");
        assert!(measure(&dir).is_empty());
    }

    #[test]
    fn a_skill_with_no_front_matter_still_reports_a_body() {
        let dir = fixture("bare");
        std::fs::write(dir.join("SKILL.md"), "Just a body, no front matter.\n").unwrap();

        let cost = measure(&dir);
        assert_eq!(cost.description, 0);
        assert_eq!(cost.body, estimate("Just a body, no front matter.\n"));
    }
}
