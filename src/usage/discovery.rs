//! Bounded history discovery and verified agent data locations.

use crate::usage::Coverage;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_SOURCES: usize = 50_000;
const MAX_DISCOVERY_ENTRIES: usize = 200_000;
const MAX_DIRECTORY_DEPTH: usize = 20;

#[derive(Debug, Clone)]
pub struct HistoryRoot {
    pub adapter: String,
    pub path: PathBuf,
}

impl HistoryRoot {
    pub fn defaults() -> Vec<Self> {
        default_roots_for(
            &crate::paths::home(),
            std::env::consts::OS,
            environment_path,
        )
    }
}

fn default_roots_for(
    home: &std::path::Path,
    platform: &str,
    environment: impl Fn(&str) -> Option<PathBuf>,
) -> Vec<HistoryRoot> {
    let codex = environment("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
    let claude = environment("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
    let pi_agent = environment("PI_CODING_AGENT_DIR").unwrap_or_else(|| home.join(".pi/agent"));
    let session_override = environment("PI_CODING_AGENT_SESSION_DIR");
    let pi_sessions = session_override
        .clone()
        .unwrap_or_else(|| pi_agent.join("sessions"));
    let configuration_name = environment("PI_CONFIG_DIR").unwrap_or_else(|| PathBuf::from(".omp"));
    let configuration_tail = configuration_name
        .components()
        .filter(|component| {
            !matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Prefix(_)
            )
        })
        .collect::<PathBuf>();
    let omp_configuration = home.join(configuration_tail);
    let mut roots = vec![
        HistoryRoot {
            adapter: "codex".into(),
            path: codex.join("sessions"),
        },
        HistoryRoot {
            adapter: "codex".into(),
            path: codex.join("archived_sessions"),
        },
        HistoryRoot {
            adapter: "claude-code".into(),
            path: claude.join("projects"),
        },
        HistoryRoot {
            adapter: "pi".into(),
            path: pi_sessions,
        },
        HistoryRoot {
            adapter: "omp".into(),
            path: omp_configuration.join("agent/sessions"),
        },
        HistoryRoot {
            adapter: "omp".into(),
            path: omp_configuration.join("profiles"),
        },
    ];
    for path in [
        session_override,
        environment("PI_CODING_AGENT_DIR").map(|path| path.join("sessions")),
    ]
    .into_iter()
    .flatten()
    {
        roots.push(HistoryRoot {
            adapter: "omp".into(),
            path,
        });
    }
    let data_home = environment("XDG_DATA_HOME");
    if matches!(platform, "linux" | "macos") {
        if let Some(data_home) = &data_home {
            for tail in ["omp/sessions", "omp/profiles"] {
                roots.push(HistoryRoot {
                    adapter: "omp".into(),
                    path: data_home.join(tail),
                });
            }
        }
    }
    let opencode_data = data_home
        .unwrap_or_else(|| home.join(".local/share"))
        .join("opencode");
    let opencode = environment("OPENCODE_DB").map_or_else(
        || opencode_data.clone(),
        |path| {
            if path.is_absolute() {
                path
            } else {
                opencode_data.join(path)
            }
        },
    );
    roots.push(HistoryRoot {
        adapter: "opencode".into(),
        path: opencode,
    });
    roots
}

fn environment_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Debug, Clone)]
pub struct HistoryFile {
    pub adapter: String,
    pub path: PathBuf,
}

pub struct Discovery {
    pub files: VecDeque<HistoryFile>,
    pub coverage: Vec<Coverage>,
}

pub fn discover(roots: &[HistoryRoot], cancelled: &AtomicBool) -> Discovery {
    let mut files = VecDeque::new();
    let mut coverage = Vec::new();
    for root in roots {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let mut item = Coverage {
            agent: root.adapter.clone(),
            status: "No history found".into(),
            ..Coverage::default()
        };
        let overlaps_another_agent = roots.iter().any(|other| {
            other.adapter != root.adapter
                && (root.path.starts_with(&other.path) || other.path.starts_with(&root.path))
        });
        if overlaps_another_agent {
            item.status =
                "Partial coverage: shared history directory has ambiguous agent attribution".into();
            item.diagnostics = 1;
        } else {
            discover_root(root, &mut files, &mut item, cancelled);
        }
        if let Some(existing) = coverage
            .iter_mut()
            .find(|item: &&mut Coverage| item.agent == root.adapter)
        {
            let has_prior_error = existing.diagnostics > 0
                || existing.status.starts_with("Partial coverage")
                || existing.status.starts_with("History unavailable");
            existing.sources += item.sources;
            existing.diagnostics += item.diagnostics;
            if item.status != "No history found"
                && (!has_prior_error
                    || item.diagnostics > 0
                    || item.status.starts_with("Partial coverage"))
            {
                existing.status = item.status;
            }
        } else {
            coverage.push(item);
        }
    }
    for agent in ["cursor", "gemini-cli", "droid", "amp"] {
        coverage.push(Coverage {
            agent: agent.into(),
            status: "Unsupported format — awaiting validated fixtures".into(),
            ..Coverage::default()
        });
    }
    Discovery { files, coverage }
}

fn discover_root(
    root: &HistoryRoot,
    files: &mut VecDeque<HistoryFile>,
    coverage: &mut Coverage,
    cancelled: &AtomicBool,
) {
    if root.adapter == "opencode" && root.path.is_file() {
        files.push_back(HistoryFile {
            adapter: root.adapter.clone(),
            path: root.path.clone(),
        });
        coverage.sources = 1;
        coverage.status = "Supported; observed evidence only".into();
        return;
    }
    let mut directories = vec![(root.path.clone(), 0)];
    let mut entries = 0;
    while let Some((directory, depth)) = directories.pop() {
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        let listing = match fs::read_dir(&directory) {
            Ok(listing) => listing,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                coverage.status = format!("History unavailable: {error}");
                coverage.diagnostics += 1;
                continue;
            }
        };
        for entry in listing {
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            entries += 1;
            if entries > MAX_DISCOVERY_ENTRIES || files.len() >= MAX_SOURCES {
                coverage.status = "Partial coverage: discovery limit reached".into();
                return;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    coverage.status = format!("Partial coverage: {error}");
                    coverage.diagnostics += 1;
                    continue;
                }
            };
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    coverage.status = format!("Partial coverage: {error}");
                    coverage.diagnostics += 1;
                    continue;
                }
            };
            if kind.is_dir() && depth < MAX_DIRECTORY_DEPTH {
                directories.push((entry.path(), depth + 1));
            }
            if kind.is_dir() && depth >= MAX_DIRECTORY_DEPTH {
                coverage.status = "Partial coverage: directory discovery limit reached".into();
                coverage.diagnostics += 1;
            }
            if kind.is_file() && is_history_file(root, &entry.path()) {
                files.push_back(HistoryFile {
                    adapter: root.adapter.clone(),
                    path: entry.path(),
                });
                coverage.sources += 1;
                if coverage.diagnostics == 0 {
                    coverage.status = "Supported; observed evidence only".into();
                }
            }
        }
    }
}

fn is_history_file(root: &HistoryRoot, path: &std::path::Path) -> bool {
    if root.adapter == "opencode" {
        return path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name == "opencode.db" || (name.starts_with("opencode-") && name.ends_with(".db"))
            });
    }
    let is_jsonl = path
        .extension()
        .is_some_and(|extension| extension == "jsonl");
    if root.adapter == "omp" && root.path.file_name().is_some_and(|name| name == "profiles") {
        return is_jsonl
            && path
                .components()
                .any(|component| component.as_os_str() == "sessions");
    }
    is_jsonl
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
