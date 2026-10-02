use super::*;
use std::collections::BTreeMap;

fn directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "skillshard-discovery-{label}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn verified_default_locations_honor_overrides_and_preserve_relative_database_paths() {
    let home = directory("locations");
    let environment = BTreeMap::from([
        ("CODEX_HOME", home.join("codex")),
        ("CLAUDE_CONFIG_DIR", home.join("claude")),
        ("PI_CODING_AGENT_SESSION_DIR", home.join("custom-sessions")),
        ("PI_CONFIG_DIR", PathBuf::from("/custom-omp")),
        ("XDG_DATA_HOME", home.join("data")),
        ("OPENCODE_DB", PathBuf::from("opencode-test.db")),
    ]);
    let roots = default_roots_for(&home, "linux", |name| environment.get(name).cloned());
    assert!(roots
        .iter()
        .any(|root| root.adapter == "codex" && root.path == home.join("codex/archived_sessions")));
    assert!(roots
        .iter()
        .any(|root| root.adapter == "claude-code" && root.path == home.join("claude/projects")));
    assert!(roots
        .iter()
        .any(|root| root.adapter == "omp" && root.path == home.join("custom-omp/agent/sessions")));
    assert!(roots
        .iter()
        .any(|root| root.adapter == "pi" && root.path == home.join("custom-sessions")));
    assert!(roots
        .iter()
        .any(|root| root.adapter == "omp" && root.path == home.join("data/omp/profiles")));
    assert!(roots.iter().any(|root| root.adapter == "opencode"
        && root.path == home.join("data/opencode/opencode-test.db")));
}

#[test]
fn shared_pi_and_omp_directory_does_not_invent_agent_attribution() {
    let path = directory("shared");
    fs::write(path.join("session.jsonl"), "{}\n").unwrap();
    let roots = ["pi", "omp"].map(|adapter| HistoryRoot {
        adapter: adapter.into(),
        path: path.clone(),
    });
    let discovery = discover(&roots, &AtomicBool::new(false));
    assert!(discovery.files.is_empty());
    assert!(discovery
        .coverage
        .iter()
        .filter(|coverage| matches!(coverage.agent.as_str(), "pi" | "omp"))
        .all(|coverage| coverage.status.contains("ambiguous agent attribution")));
}

#[test]
fn database_channels_and_profile_sessions_are_discovered_without_unrelated_files() {
    let path = directory("formats");
    fs::write(path.join("opencode.db"), "synthetic database placeholder").unwrap();
    fs::write(
        path.join("opencode-preview.db"),
        "synthetic database placeholder",
    )
    .unwrap();
    fs::write(path.join("unrelated.db"), "excluded").unwrap();
    let profiles = path.join("profiles");
    fs::create_dir_all(profiles.join("named/agent/sessions/tasks")).unwrap();
    fs::write(
        profiles.join("named/agent/sessions/tasks/worker.jsonl"),
        "{}\n",
    )
    .unwrap();
    fs::write(profiles.join("named/unrelated.jsonl"), "excluded").unwrap();
    let opencode = discover(
        &[HistoryRoot {
            adapter: "opencode".into(),
            path: path.clone(),
        }],
        &AtomicBool::new(false),
    );
    assert_eq!(opencode.files.len(), 2);
    let omp = discover(
        &[HistoryRoot {
            adapter: "omp".into(),
            path: profiles,
        }],
        &AtomicBool::new(false),
    );
    assert_eq!(omp.files.len(), 1);
}

#[test]
fn a_later_supported_root_does_not_hide_an_earlier_discovery_error() {
    let path = directory("errors");
    let invalid = path.join("not-a-directory");
    fs::write(&invalid, "synthetic").unwrap();
    let valid = path.join("valid");
    fs::create_dir_all(&valid).unwrap();
    fs::write(valid.join("session.jsonl"), "{}\n").unwrap();
    let roots = [invalid, valid].map(|path| HistoryRoot {
        adapter: "codex".into(),
        path,
    });
    let discovery = discover(&roots, &AtomicBool::new(false));
    assert_eq!(discovery.files.len(), 1);
    let coverage = discovery
        .coverage
        .iter()
        .find(|coverage| coverage.agent == "codex")
        .unwrap();
    assert_eq!(coverage.diagnostics, 1);
    assert!(coverage.status.starts_with("History unavailable"));
}
