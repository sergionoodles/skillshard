use super::ScanCoverage;
use crate::usage::ingest::HistoryFile;
use crate::usage::{Coverage, Source};

const SUPPORTED: &str = "Supported; observed evidence only";

fn coverage(status: &str, sources: usize, diagnostics: usize) -> ScanCoverage {
    ScanCoverage::new(vec![Coverage {
        agent: "claude-code".into(),
        status: status.into(),
        sources,
        diagnostics,
    }])
}

fn update_source(coverage: &mut ScanCoverage, id: usize, status: &str, diagnostics: u64) {
    let source = Source {
        status: status.into(),
        diagnostics,
        ..Source::default()
    };
    coverage.update(&history_file(id), Some(&source), None);
}

fn history_file(id: usize) -> HistoryFile {
    HistoryFile {
        adapter: "claude-code".into(),
        path: format!("/synthetic/history/{id}.jsonl").into(),
    }
}

#[test]
fn mixed_supported_unsupported_and_malformed_sources_report_partial_coverage_counts() {
    let mut coverage = coverage(SUPPORTED, 122, 0);
    for id in 0..88 {
        update_source(&mut coverage, id, "supported", 0);
    }
    update_source(&mut coverage, 88, "unsupported_format", 1);
    for id in 89..122 {
        update_source(&mut coverage, id, "malformed", 2);
    }
    let snapshot = coverage.snapshot();
    assert_eq!(
        snapshot[0].status,
        "Partial coverage: 88 supported, 1 unsupported, 33 malformed sources"
    );
    assert_eq!(snapshot[0].sources, 122);
    assert_eq!(snapshot[0].diagnostics, 67);
}

#[test]
fn wholly_unsupported_sources_keep_the_unsupported_label() {
    let mut coverage = coverage(SUPPORTED, 2, 0);
    update_source(&mut coverage, 0, "unsupported_format", 1);
    update_source(&mut coverage, 1, "unsupported_format", 1);
    assert_eq!(coverage.snapshot()[0].status, "Unsupported format");
}

#[test]
fn pending_or_partially_attributed_sources_are_not_labeled_wholly_unsupported() {
    let mut coverage = coverage(SUPPORTED, 2, 0);
    update_source(&mut coverage, 0, "unsupported_format", 1);
    coverage.update(&history_file(1), None, None);
    assert_eq!(
        coverage.snapshot()[0].status,
        "Partial coverage: 1 unsupported, 1 awaiting import sources"
    );
    update_source(&mut coverage, 1, "partial_attribution", 1);
    let snapshot = coverage.snapshot();
    assert_eq!(
        snapshot[0].status,
        "Partial coverage: 1 unsupported, 1 with partial attribution sources; worker activity or inherited context could not be fully validated"
    );
    assert_eq!(snapshot[0].diagnostics, 2);
}

#[test]
fn known_sources_not_yet_processed_prevent_an_adapter_wide_unsupported_label() {
    let mut coverage = coverage(SUPPORTED, 125, 0);
    update_source(&mut coverage, 0, "unsupported_format", 1);
    assert_eq!(
        coverage.snapshot()[0].status,
        "Partial coverage: 1 unsupported, 124 awaiting import sources"
    );
}

#[test]
fn malformed_and_diagnostics_only_sources_include_supported_source_counts() {
    let mut malformed = coverage(SUPPORTED, 21, 0);
    for id in 0..20 {
        update_source(&mut malformed, id, "supported", 0);
    }
    update_source(&mut malformed, 20, "malformed", 3);
    assert_eq!(
        malformed.snapshot()[0].status,
        "Partial coverage: 20 supported, 1 malformed sources"
    );
    let mut diagnostics = coverage(SUPPORTED, 2, 0);
    update_source(&mut diagnostics, 0, "supported", 3);
    update_source(&mut diagnostics, 1, "supported", 0);
    assert_eq!(
        diagnostics.snapshot()[0].status,
        "Partial coverage: 2 supported sources; parser diagnostics: 3"
    );
}

#[test]
fn partial_attribution_keeps_its_reason_alongside_source_counts() {
    let mut coverage = coverage(SUPPORTED, 2, 0);
    update_source(&mut coverage, 0, "supported", 0);
    update_source(&mut coverage, 1, "partial_attribution", 1);
    assert_eq!(
        coverage.snapshot()[0].status,
        "Partial attribution: 1 supported, 1 with partial attribution sources; worker activity or inherited context could not be fully validated"
    );
}

#[test]
fn replacing_a_source_replaces_counts_and_clears_stale_unsupported_status() {
    let mut coverage = coverage(SUPPORTED, 2, 0);
    update_source(&mut coverage, 0, "supported", 0);
    update_source(&mut coverage, 1, "unsupported_format", 1);
    assert_eq!(
        coverage.snapshot()[0].status,
        "Partial coverage: 1 supported, 1 unsupported sources"
    );
    update_source(&mut coverage, 1, "supported", 0);
    let snapshot = coverage.snapshot();
    assert_eq!(snapshot[0].status, SUPPORTED);
    assert_eq!(snapshot[0].diagnostics, 0);
}

#[test]
fn baseline_warnings_and_import_errors_keep_their_existing_priority() {
    for (status, diagnostics) in [
        ("Partial coverage: discovery limit reached", 0),
        ("History unavailable: permission denied", 0),
        ("Baseline parser warning", 1),
    ] {
        let mut coverage = coverage(status, 1, diagnostics);
        update_source(&mut coverage, 0, "unsupported_format", 1);
        assert_eq!(coverage.snapshot()[0].status, status);
        coverage.update(
            &history_file(0),
            None,
            Some("synthetic import failed".into()),
        );
        let snapshot = coverage.snapshot();
        assert_eq!(
            snapshot[0].status,
            "Partial coverage: synthetic import failed"
        );
        assert_eq!(snapshot[0].diagnostics, diagnostics + 1);
    }
}
