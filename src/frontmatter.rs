//! Minimal YAML front-matter reader for `SKILL.md`.
//!
//! Skill front matter is a flat map of scalars. Real skills use plain values,
//! quoted values and folded/literal block scalars (`>` and `|`), so those are
//! the three forms handled here; anything else is returned verbatim.

/// The fields Skillshard reads out of a `SKILL.md`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FrontMatter {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// Parse the leading `---` fenced block of a `SKILL.md`.
///
/// Returns an empty result when the file has no front matter.
pub fn parse(source: &str) -> FrontMatter {
    let mut out = FrontMatter::default();
    let Some(block) = fenced_block(source) else {
        return out;
    };

    let lines: Vec<&str> = block.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;

        // Only top-level keys matter; indented lines belong to the previous key.
        if line.starts_with([' ', '\t']) || line.trim().is_empty() {
            continue;
        }
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key != "name" && key != "description" {
            continue;
        }

        let marker = raw.trim();
        let value = if marker.starts_with('>') || marker.starts_with('|') {
            let fold = marker.starts_with('>');
            let (block, consumed) = block_scalar(&lines[i..], fold);
            i += consumed;
            block
        } else {
            unquote(marker)
        };

        match key {
            "name" => out.name = Some(value),
            _ => out.description = Some(value),
        }
    }
    out
}

/// Extract the text between the opening and closing `---` fences.
fn fenced_block(source: &str) -> Option<&str> {
    let body = source
        .strip_prefix("---\n")
        .or_else(|| source.strip_prefix("---\r\n"))?;
    let end = body
        .match_indices("\n---")
        .find(|(idx, _)| {
            let after = &body[idx + 4..];
            after.is_empty() || after.starts_with('\n') || after.starts_with('\r')
        })
        .map(|(idx, _)| idx)?;
    Some(&body[..end])
}

/// Collect an indented block scalar, joining lines when `fold` is set.
///
/// Returns the text and how many lines it consumed.
fn block_scalar(rest: &[&str], fold: bool) -> (String, usize) {
    let mut parts: Vec<String> = Vec::new();
    let mut used = 0;
    for line in rest {
        if !line.starts_with([' ', '\t']) && !line.trim().is_empty() {
            break;
        }
        used += 1;
        parts.push(line.trim().to_string());
    }
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    let joined = if fold {
        parts
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        parts.join("\n")
    };
    (joined, used)
}

/// Strip matching surrounding quotes from a scalar.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && (value.starts_with('"') || value.starts_with('\'')) {
        let quote = bytes[0];
        if bytes[bytes.len() - 1] == quote {
            let inner = &value[1..value.len() - 1];
            return if quote == b'"' {
                inner.replace("\\\"", "\"").replace("\\n", "\n")
            } else {
                inner.replace("''", "'")
            };
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plain_scalars() {
        let fm = parse("---\nname: my-skill\ndescription: Does a thing\n---\n\n# Body\n");
        assert_eq!(fm.name.as_deref(), Some("my-skill"));
        assert_eq!(fm.description.as_deref(), Some("Does a thing"));
    }

    #[test]
    fn folds_block_scalars_into_one_line() {
        // The form used by the real ponytail skills.
        let fm = parse("---\nname: ponytail\ndescription: >\n  Forces the laziest\n  solution that works.\n---\n");
        assert_eq!(
            fm.description.as_deref(),
            Some("Forces the laziest solution that works.")
        );
    }

    #[test]
    fn keeps_literal_block_line_breaks() {
        let fm = parse("---\ndescription: |\n  line one\n  line two\n---\n");
        assert_eq!(fm.description.as_deref(), Some("line one\nline two"));
    }

    #[test]
    fn ignores_other_keys_and_nested_maps() {
        let fm = parse("---\nname: a\nmetadata:\n  internal: true\ndescription: b\n---\n");
        assert_eq!(fm.name.as_deref(), Some("a"));
        assert_eq!(fm.description.as_deref(), Some("b"));
    }

    #[test]
    fn unwraps_quoted_values() {
        let fm = parse("---\nname: \"quoted\"\ndescription: 'single'\n---\n");
        assert_eq!(fm.name.as_deref(), Some("quoted"));
        assert_eq!(fm.description.as_deref(), Some("single"));
    }

    #[test]
    fn missing_front_matter_is_empty() {
        assert_eq!(parse("# Just a heading\n"), FrontMatter::default());
    }
}
