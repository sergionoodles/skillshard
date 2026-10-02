//! Local observations of skill instruction loading, separate from installed skills.

pub mod adapters;
mod generations;
pub mod ingest;
pub mod opencode_ingest;
pub mod service;
pub mod storage;

#[cfg(test)]
mod ingest_family_tests;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type UsageResult<T> = Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    Main,
    Subagent,
    #[default]
    Unknown,
}

impl WorkerRole {
    pub fn key(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Subagent => "subagent",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
    #[default]
    Unknown,
}

impl Outcome {
    pub fn key(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    SkillTool,
    FileRead,
    ShellRead,
    Attachment,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub agent: String,
    pub native_id: String,
    pub parent_native_id: Option<String>,
    pub role: WorkerRole,
    pub worker_name: Option<String>,
    pub model: Option<String>,
    pub project: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activation {
    pub session: Session,
    pub worker_id: Option<String>,
    pub native_id: String,
    pub occurred_at: Option<i64>,
    pub working_directory: Option<PathBuf>,
    pub evidence: Evidence,
    pub reference: String,
    pub outcome: Outcome,
}

/// Only normalized metadata crosses the ingestion boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Session(Session),
    SessionPath {
        session: Session,
        path: PathBuf,
    },
    SessionEntry {
        session: Session,
        entry_id: String,
        inherits_parent: bool,
        occurred_at: Option<i64>,
    },
    Activation(Activation),
    ToolResult {
        agent: String,
        session_id: String,
        worker_id: Option<String>,
        call_id: String,
        outcome: Outcome,
        occurred_at: Option<i64>,
    },
    Link {
        agent: String,
        parent_id: String,
        child_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoleFilter {
    #[default]
    All,
    Main,
    Subagents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Metric {
    #[default]
    Activations,
    Sessions,
    Conversations,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub agent: Option<String>,
    pub project: Option<PathBuf>,
    pub role: RoleFilter,
    pub metric: Metric,
    pub since: Option<i64>,
    pub range_days: Option<crate::preferences::TrackingDays>,
    /// Restricts counts and daily buckets; empty means every skill. Rankings
    /// ignore it so a skill picker can always offer every used skill.
    pub skill_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub activations: u64,
    pub sessions: u64,
    pub conversations: u64,
    pub subagent_activations: u64,
    pub main_activations: u64,
    pub unconfirmed: u64,
    pub last_used: Option<i64>,
    pub provisional_conversations: bool,
}

impl Counts {
    pub fn value(&self, metric: Metric) -> u64 {
        match metric {
            Metric::Activations => self.activations,
            Metric::Sessions => self.sessions,
            Metric::Conversations => self.conversations,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ranking {
    pub skill_id: i64,
    pub name: String,
    pub identity: String,
    pub counts: Counts,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bucket {
    pub label: String,
    pub value: u64,
}

/// One skill's share of a local calendar day.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillBucket {
    pub label: String,
    pub skill_id: i64,
    pub value: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub counts: Counts,
    pub rankings: Vec<Ranking>,
    pub daily: Vec<Bucket>,
    /// Daily buckets split by skill, only computed for `Query::skill_ids`.
    pub daily_by_skill: Vec<SkillBucket>,
    pub agents: Vec<String>,
    pub projects: Vec<PathBuf>,
    pub unresolved: u64,
    pub unknown_time: u64,
    pub skill_ids: std::collections::HashMap<String, i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServiceState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Rebuilding,
    RebuildPaused,
    Failed,
}

#[derive(Debug, Clone, Default)]
pub struct Coverage {
    pub agent: String,
    pub status: String,
    pub sources: usize,
    pub diagnostics: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub state: ServiceState,
    pub generation: u64,
    pub imported_records: u64,
    pub coverage: Vec<Coverage>,
    pub error: Option<String>,
}

/// Persistent source cursor; parser_state contains metadata, never chat text.
#[derive(Debug, Clone, Default)]
pub struct Source {
    pub adapter: String,
    pub path: String,
    pub generation: i64,
    pub size: u64,
    pub modified: String,
    pub identity: String,
    pub offset: u64,
    pub fingerprint: String,
    pub parser_version: u32,
    pub cutoff: i64,
    pub parser_state: String,
    pub diagnostics: u64,
    pub unknown_time: u64,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct Observation {
    pub locator: String,
    pub record: Record,
}

#[cfg(test)]
mod crash_tests;
#[cfg(test)]
mod ingest_tests;
#[cfg(test)]
mod service_tests;
