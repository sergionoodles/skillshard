//! Skillshard's own per-scope state.
//!
//! The `skills` CLI has no notion of a disabled skill, so the record of what
//! was turned off — and which agents had it before — lives here, beside the
//! disabled files themselves.

use crate::model::Scope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a skill looked like before it was disabled, so it can be restored.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisabledRecord {
    /// `--agent` keys that loaded the skill when it was disabled.
    #[serde(default)]
    pub agents: Vec<String>,
}

/// Persisted state for one scope.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopeState {
    #[serde(default)]
    pub disabled: BTreeMap<String, DisabledRecord>,
}

/// Read the state for `scope`, defaulting to empty.
pub fn load(scope: &Scope) -> ScopeState {
    std::fs::read_to_string(scope.state_file())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Write the state for `scope`, creating the directory if needed.
pub fn save(scope: &Scope, state: &ScopeState) -> std::io::Result<()> {
    let path = scope.state_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(state)?;
    std::fs::write(path, text)
}
