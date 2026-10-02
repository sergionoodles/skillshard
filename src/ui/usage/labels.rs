//! Human-readable text for usage values: install contexts, times, states and
//! metric names.

use crate::usage::{Metric, ServiceState};
use chrono::{DateTime, Local, NaiveDate, Utc};
use std::path::PathBuf;

const CONTEXT_UNAVAILABLE: &str = "Install context unavailable";
const GLOBAL_SCOPE: &str = "global";

type InstallIdentity = (String, String, Option<(String, Option<String>)>);

/// Where a ranked skill is installed, from its stored identity.
pub fn identity_label(identity: &str) -> String {
    if let Ok(install) = serde_json::from_str::<InstallIdentity>(identity) {
        return format_install_context(install);
    }
    let Ok((base, agent, path)) = serde_json::from_str::<(String, String, PathBuf)>(identity)
    else {
        return CONTEXT_UNAVAILABLE.into();
    };
    let Ok(install) = serde_json::from_str::<InstallIdentity>(&base) else {
        return CONTEXT_UNAVAILABLE.into();
    };
    let agent_label = crate::agents::by_key(&agent).map_or(agent.as_str(), |agent| agent.display);
    format!(
        "{} · {agent_label} copy: {}",
        format_install_context(install),
        path.display()
    )
}

fn format_install_context((scope, _, source): InstallIdentity) -> String {
    let context = if scope == GLOBAL_SCOPE {
        "Global install".to_string()
    } else {
        format!("Project: {scope}")
    };
    match source {
        Some((repository, Some(path))) if !path.is_empty() => {
            format!("{context} · {repository} · {path}")
        }
        Some((repository, _)) => format!("{context} · {repository}"),
        None => context,
    }
}

/// An agent's display name, or its key when it is not one Skillshard knows.
pub fn agent_label(key: &str) -> String {
    crate::agents::by_key(key).map_or(key.to_string(), |agent| agent.display.to_string())
}

pub fn timestamp_label(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map(|time| {
            time.with_timezone(&Local)
                .format("%b %d, %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "Unavailable".into())
}

/// `Mon, Sep 28`.
pub fn day_label(date: NaiveDate) -> String {
    date.format("%a, %b %-d").to_string()
}

/// `Sep 28`, for chart axes.
pub fn short_day_label(date: NaiveDate) -> String {
    date.format("%b %-d").to_string()
}

pub fn state_label(state: ServiceState) -> &'static str {
    match state {
        ServiceState::Stopped => "Stopped",
        ServiceState::Starting => "Starting import",
        ServiceState::Running => "Tracking locally",
        ServiceState::Stopping => "Stopping",
        ServiceState::Rebuilding => "Rebuilding local history",
        ServiceState::RebuildPaused => "Rebuild paused",
        ServiceState::Failed => "Tracking needs attention",
    }
}

pub fn metric_title(metric: Metric) -> &'static str {
    match metric {
        Metric::Activations => "Activations",
        Metric::Sessions => "Sessions",
        Metric::Conversations => "Conversations",
    }
}

/// `1 activation`, `3 sessions`.
pub fn metric_count(metric: Metric, value: u64) -> String {
    let noun = match (metric, value) {
        (Metric::Activations, 1) => "activation",
        (Metric::Activations, _) => "activations",
        (Metric::Sessions, 1) => "session",
        (Metric::Sessions, _) => "sessions",
        (Metric::Conversations, 1) => "conversation",
        (Metric::Conversations, _) => "conversations",
    };
    format!("{value} {noun}")
}

/// How the top skill's value relates to the period total, worded so that
/// overlapping sessions never read as a slice of a whole.
pub fn share_label(metric: Metric, percent: u64, selected: bool) -> String {
    let pool = if selected {
        "the selected skills"
    } else {
        "skills"
    };
    match (metric, selected) {
        (Metric::Activations, true) => format!("{percent}% of the selected skills' activations"),
        (Metric::Activations, false) => format!("{percent}% of all activations"),
        (Metric::Sessions, _) => format!("Used in {percent}% of sessions with {pool}"),
        (Metric::Conversations, _) => format!("Used in {percent}% of conversations with {pool}"),
    }
}
