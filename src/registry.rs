//! Read-only clients for the two skills.sh services the app consults.
//!
//! * `https://skills.sh/api/search` — the skill directory, used to browse and
//!   pick a source in the install dialog.
//! * `https://add-skill.vercel.sh/audit` — the partner security review the
//!   `skills` CLI shows during `add`. Skillshard fetches the same data so the
//!   verdict is on screen *before* the user commits to installing.
//!
//! Both calls are blocking; run them on a background executor.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

const SEARCH_URL: &str = "https://skills.sh/api/search";
const AUDIT_URL: &str = "https://add-skill.vercel.sh/audit";
const USER_AGENT: &str = concat!("skillshard/", env!("CARGO_PKG_VERSION"));

/// One result from the skill directory.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Skill name, as installed.
    pub name: String,
    /// `owner/repo` to hand to `skills add`.
    pub source: String,
    #[serde(default)]
    pub skill_id: Option<String>,
    /// Lifetime install count, used to order results.
    #[serde(default)]
    pub installs: u64,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    skills: Vec<SearchHit>,
}

/// Search the public skill directory.
pub fn search(query: &str, limit: u32) -> Result<Vec<SearchHit>, String> {
    let url = format!("{SEARCH_URL}?q={}&limit={limit}", urlencode(query));
    let body = get(&url)?;
    serde_json::from_str::<SearchResponse>(&body)
        .map(|r| r.skills)
        .map_err(|e| format!("could not read search results: {e}"))
}

/// Lifetime installs of one skill, according to skills.sh.
///
/// There is no per-skill endpoint, so this searches by name and picks the hit
/// from the right source: popular names exist in many repositories.
pub fn fetch_install_count(source: &str, name: &str) -> Result<Option<u64>, String> {
    Ok(install_count(&search(name, 50)?, source, name))
}

/// The install count for `name` from `source` among search results.
pub fn install_count(hits: &[SearchHit], source: &str, name: &str) -> Option<u64> {
    let source = source.trim_end_matches(".git");
    hits.iter()
        .find(|hit| {
            hit.source.eq_ignore_ascii_case(source)
                && (hit.name == name || hit.skill_id.as_deref() == Some(name))
        })
        .map(|hit| hit.installs)
}

/// Compact display form: `950`, `60.6k`, `722k`, `1.2M`.
pub fn format_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=99_999 => trim_decimal(n as f64 / 1_000., "k"),
        100_000..=999_999 => format!("{}k", n / 1_000),
        _ => trim_decimal(n as f64 / 1_000_000., "M"),
    }
}

/// One decimal place, dropping a trailing `.0`.
fn trim_decimal(value: f64, unit: &str) -> String {
    let text = format!("{:.1}", (value * 10.).floor() / 10.);
    format!("{}{unit}", text.trim_end_matches(".0"))
}

/// How risky one partner judged a skill to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Risk {
    Safe,
    Low,
    Medium,
    High,
    Critical,
    #[default]
    Unknown,
}

impl Risk {
    fn parse(value: &str) -> Risk {
        match value {
            "safe" => Risk::Safe,
            "low" => Risk::Low,
            "medium" => Risk::Medium,
            "high" => Risk::High,
            "critical" => Risk::Critical,
            _ => Risk::Unknown,
        }
    }

    /// Label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Risk::Safe => "safe",
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
            Risk::Critical => "critical",
            Risk::Unknown => "unrated",
        }
    }

    /// Whether this verdict deserves a warning before installing.
    pub fn is_concerning(self) -> bool {
        matches!(self, Risk::Medium | Risk::High | Risk::Critical)
    }
}

/// One partner's verdict.
#[derive(Debug, Clone, Deserialize)]
struct PartnerAudit {
    #[serde(default)]
    risk: Option<String>,
    #[serde(default)]
    alerts: Option<u32>,
    #[serde(default)]
    score: Option<f64>,
}

/// The full security review for one skill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillAudit {
    /// Per-partner verdicts, e.g. `socket`, `snyk`, `ath`, `zeroleaks`.
    pub partners: Vec<(String, Risk)>,
    /// Total alerts reported across partners.
    pub alerts: u32,
}

impl SkillAudit {
    /// The most severe verdict any partner returned.
    ///
    /// Reviews disagree — a skill can be `safe` to one partner and `critical`
    /// to another — so the UI leads with the worst case.
    pub fn worst(&self) -> Risk {
        self.partners
            .iter()
            .map(|(_, r)| *r)
            .filter(|r| *r != Risk::Unknown)
            .max()
            .unwrap_or(Risk::Unknown)
    }

    /// Whether anything here should be shown as a warning.
    pub fn is_concerning(&self) -> bool {
        self.worst().is_concerning() || self.alerts > 0
    }
}

/// Fetch the security review for skills from one source.
///
/// `source` is `owner/repo`; `names` are skill names, which are slugified to
/// match the service's keys.
pub fn audit(source: &str, names: &[String]) -> Result<BTreeMap<String, SkillAudit>, String> {
    if names.is_empty() {
        return Ok(BTreeMap::new());
    }
    let slugs: Vec<String> = names.iter().map(|n| to_slug(n)).collect();
    let url = format!(
        "{AUDIT_URL}?source={}&skills={}",
        urlencode(source),
        urlencode(&slugs.join(","))
    );
    let body = get(&url)?;
    let raw: BTreeMap<String, BTreeMap<String, PartnerAudit>> =
        serde_json::from_str(&body).map_err(|e| format!("could not read audit: {e}"))?;

    Ok(raw
        .into_iter()
        .map(|(skill, partners)| {
            let mut audit = SkillAudit::default();
            for (partner, data) in partners {
                let risk = data.risk.as_deref().map(Risk::parse).unwrap_or_default();
                audit.alerts += data.alerts.unwrap_or(0);
                let _ = data.score;
                audit.partners.push((partner, risk));
            }
            audit.partners.sort();
            (skill, audit)
        })
        .collect())
}

/// Convert a skill name into the slug the audit service is keyed by.
///
/// Mirrors the CLI's `toSkillSlug` exactly; a mismatch silently loses the
/// review for that skill.
pub fn to_slug(name: &str) -> String {
    let lowered = name.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    for ch in lowered.chars() {
        match ch {
            ' ' | '_' => out.push('-'),
            c if c.is_ascii_alphanumeric() || c == '-' => out.push(c),
            _ => {}
        }
    }
    // Collapse runs of dashes, then trim them from both ends.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_dash = false;
    for ch in out.chars() {
        if ch == '-' {
            if !prev_dash {
                collapsed.push(ch);
            }
            prev_dash = true;
        } else {
            collapsed.push(ch);
            prev_dash = false;
        }
    }
    collapsed.trim_matches('-').to_string()
}

/// Percent-encode a query parameter value.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Perform a GET and return the body as text.
pub(crate) fn get(url: &str) -> Result<String, String> {
    let mut response = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .call()
        .map_err(|e| format!("{e}"))?;
    response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_match_the_cli_rules() {
        assert_eq!(to_slug("Convex Best Practices"), "convex-best-practices");
        assert_eq!(to_slug("my_skill"), "my-skill");
        assert_eq!(to_slug("--Weird--Name!!--"), "weird-name");
        assert_eq!(to_slug("vercel-optimize"), "vercel-optimize");
    }

    #[test]
    fn worst_verdict_wins_across_partners() {
        // Real data: the audit service returned safe/critical/low for the same
        // skill, so the UI must lead with critical rather than an average.
        let audit = SkillAudit {
            partners: vec![
                ("ath".into(), Risk::Safe),
                ("snyk".into(), Risk::Low),
                ("socket".into(), Risk::Critical),
            ],
            alerts: 0,
        };
        assert_eq!(audit.worst(), Risk::Critical);
        assert!(audit.is_concerning());
    }

    #[test]
    fn an_all_safe_review_is_not_concerning() {
        let audit = SkillAudit {
            partners: vec![("ath".into(), Risk::Safe), ("socket".into(), Risk::Safe)],
            alerts: 0,
        };
        assert_eq!(audit.worst(), Risk::Safe);
        assert!(!audit.is_concerning());
    }

    #[test]
    fn unrated_skills_report_unknown() {
        assert_eq!(SkillAudit::default().worst(), Risk::Unknown);
    }

    fn hit(name: &str, source: &str, installs: u64) -> SearchHit {
        SearchHit {
            name: name.into(),
            source: source.into(),
            skill_id: Some(name.into()),
            installs,
        }
    }

    #[test]
    fn install_count_matches_the_source_not_just_the_name() {
        // Popular names are published by many repositories.
        let hits = [
            hit("frontend-design", "someone-else/skills", 12),
            hit("frontend-design", "anthropics/skills", 90_000),
        ];
        assert_eq!(
            install_count(&hits, "anthropics/skills", "frontend-design"),
            Some(90_000)
        );
        assert_eq!(
            install_count(&hits, "anthropics/skills.git", "frontend-design"),
            Some(90_000)
        );
        assert_eq!(
            install_count(&hits, "unknown/repo", "frontend-design"),
            None
        );
    }

    #[test]
    fn formats_counts_compactly() {
        assert_eq!(format_count(950), "950");
        assert_eq!(format_count(1_000), "1k");
        assert_eq!(format_count(60_640), "60.6k");
        // Rounds down, so a count never reads higher than it is.
        assert_eq!(format_count(99_999), "99.9k");
        assert_eq!(format_count(722_445), "722k");
        assert_eq!(format_count(1_250_000), "1.2M");
    }

    #[test]
    fn encodes_query_parameters() {
        assert_eq!(urlencode("owner/repo"), "owner%2Frepo");
        assert_eq!(urlencode("a,b"), "a%2Cb");
    }
}
