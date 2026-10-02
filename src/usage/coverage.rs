//! Per-source diagnostics replace earlier summaries rather than accumulating scans.

use crate::usage::ingest::HistoryFile;
use crate::usage::{Coverage, Source};
use std::collections::BTreeMap;
use std::path::PathBuf;

const PARTIAL_ATTRIBUTION_REASON: &str =
    "worker activity or inherited context could not be fully validated";

#[derive(Default)]
pub(super) struct ScanCoverage {
    baseline: Vec<Coverage>,
    sources: BTreeMap<(String, PathBuf), Summary>,
    totals: BTreeMap<String, Totals>,
}

#[cfg(test)]
#[path = "coverage_tests.rs"]
mod tests;

#[derive(Default)]
struct Summary {
    diagnostics: usize,
    supported: bool,
    unsupported: bool,
    partial: bool,
    malformed: bool,
    error: Option<String>,
}

#[derive(Default)]
struct Totals {
    diagnostics: usize,
    sources: usize,
    supported: usize,
    unsupported: usize,
    partial: usize,
    malformed: usize,
    errors: usize,
}

impl ScanCoverage {
    pub(super) fn new(baseline: Vec<Coverage>) -> Self {
        Self {
            baseline,
            ..Self::default()
        }
    }

    pub(super) fn update(
        &mut self,
        file: &HistoryFile,
        source: Option<&Source>,
        error: Option<String>,
    ) {
        let summary = Summary {
            diagnostics: source.map_or(0, |source| {
                usize::try_from(source.diagnostics).unwrap_or(usize::MAX)
            }),
            supported: source.is_some_and(|source| source.status == "supported"),
            unsupported: source.is_some_and(|source| source.status == "unsupported_format"),
            partial: source.is_some_and(|source| source.status == "partial_attribution"),
            malformed: source.is_some_and(|source| source.status == "malformed"),
            error,
        };
        let key = (file.adapter.clone(), file.path.clone());
        let totals = self.totals.entry(file.adapter.clone()).or_default();
        if let Some(previous) = self.sources.remove(&key) {
            totals.diagnostics = totals.diagnostics.saturating_sub(previous.diagnostics);
            totals.supported -= usize::from(previous.supported);
            totals.unsupported -= usize::from(previous.unsupported);
            totals.partial -= usize::from(previous.partial);
            totals.malformed -= usize::from(previous.malformed);
            totals.errors -= usize::from(previous.error.is_some());
        } else {
            totals.sources += 1;
        }
        totals.diagnostics = totals.diagnostics.saturating_add(summary.diagnostics);
        totals.supported += usize::from(summary.supported);
        totals.unsupported += usize::from(summary.unsupported);
        totals.partial += usize::from(summary.partial);
        totals.malformed += usize::from(summary.malformed);
        totals.errors += usize::from(summary.error.is_some());
        self.sources.insert(key, summary);
    }

    pub(super) fn snapshot(&self) -> Vec<Coverage> {
        self.baseline
            .iter()
            .map(|baseline| {
                let Some(totals) = self.totals.get(&baseline.agent) else {
                    return baseline.clone();
                };
                let mut item = baseline.clone();
                item.diagnostics = item
                    .diagnostics
                    .saturating_add(totals.diagnostics)
                    .saturating_add(totals.errors);
                if totals.errors > 0 {
                    let error = self
                        .sources
                        .iter()
                        .find_map(|((agent, _), summary)| {
                            (agent == &item.agent)
                                .then_some(summary.error.as_deref())
                                .flatten()
                        })
                        .unwrap_or("a history source could not be imported");
                    item.status = format!("Partial coverage: {error}");
                    return item;
                }
                if baseline.diagnostics > 0
                    || baseline.status.starts_with("Partial coverage")
                    || baseline.status.starts_with("History unavailable")
                {
                    return item;
                }
                if let Some(status) = describe_source_status(totals, baseline.sources) {
                    item.status = status;
                }
                item
            })
            .collect()
    }
}

fn describe_source_status(totals: &Totals, discovered_sources: usize) -> Option<String> {
    let source_count = discovered_sources.max(totals.sources);
    if totals.unsupported > 0 && totals.unsupported == source_count {
        return Some("Unsupported format".into());
    }
    if totals.unsupported == 0
        && totals.partial == 0
        && totals.malformed == 0
        && totals.diagnostics == 0
    {
        return None;
    }
    let counts = describe_source_counts(totals, source_count);
    let prefix = if totals.unsupported == 0 && totals.partial > 0 {
        "Partial attribution"
    } else {
        "Partial coverage"
    };
    let summary = format!("{prefix}: {counts} sources");
    if totals.partial > 0 {
        return Some(format!("{summary}; {PARTIAL_ATTRIBUTION_REASON}"));
    }
    if totals.unsupported == 0 && totals.malformed == 0 {
        return Some(format!(
            "{summary}; parser diagnostics: {}",
            totals.diagnostics
        ));
    }
    Some(summary)
}

fn describe_source_counts(totals: &Totals, source_count: usize) -> String {
    let pending =
        source_count - totals.supported - totals.unsupported - totals.partial - totals.malformed;
    [
        (totals.supported, "supported"),
        (totals.unsupported, "unsupported"),
        (totals.malformed, "malformed"),
        (totals.partial, "with partial attribution"),
        (pending, "awaiting import"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect::<Vec<_>>()
    .join(", ")
}
