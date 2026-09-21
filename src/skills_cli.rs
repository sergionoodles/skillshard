//! Invocation of the `skills` CLI (<https://skills.sh>).
//!
//! Skillshard never implements install, update or uninstall logic itself. It
//! builds a fully-specified, non-interactive command line — scope, agents and
//! skill names all chosen up front in the UI — and runs it. Every command
//! carries `-y` so the CLI never stops to ask a question it has no terminal
//! to ask in.

use crate::model::Scope;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

/// How to invoke the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher {
    /// A `skills` binary already on `PATH`.
    Binary(String),
    /// `npx --yes skills@<version>`, the documented entry point.
    Npx { spec: String },
}

impl Launcher {
    /// Prefer an installed binary, otherwise fall back to `npx`.
    pub fn detect() -> Self {
        if which("skills").is_some() {
            Launcher::Binary("skills".into())
        } else {
            Launcher::Npx {
                spec: "skills@latest".into(),
            }
        }
    }

    /// Program name and the arguments that must precede the subcommand.
    fn program(&self) -> (String, Vec<String>) {
        match self {
            Launcher::Binary(bin) => (bin.clone(), Vec::new()),
            Launcher::Npx { spec } => ("npx".into(), vec!["--yes".into(), spec.clone()]),
        }
    }
}

/// Locate an executable on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Everything the user chooses in the install dialog before anything runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRequest {
    /// `owner/repo`, a URL, or a local path.
    pub source: String,
    pub scope: Scope,
    /// Skill names to install; empty means every skill in the source.
    pub skills: Vec<String>,
    /// `--agent` keys to install to; empty means every detected agent.
    pub agents: Vec<String>,
    /// Copy files instead of symlinking.
    pub copy: bool,
}

impl InstallRequest {
    /// The exact `skills` arguments for this request.
    ///
    /// `--json` implies machine-readable output and requires `-y`, which is
    /// always present: the app never drives an interactive session.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["add".to_string(), self.source.clone()];
        push_multi(&mut args, "-s", &self.skills);
        push_multi(&mut args, "-a", &self.agents);
        if self.scope.is_global() {
            args.push("-g".into());
        }
        if self.copy {
            args.push("--copy".into());
        }
        args.push("-y".into());
        args.push("--json".into());
        args
    }
}

/// Emit `flag value` pairs, falling back to `*` (all) when nothing is chosen.
fn push_multi(args: &mut Vec<String>, flag: &str, values: &[String]) {
    if values.is_empty() {
        args.push(flag.into());
        args.push("*".into());
        return;
    }
    // One flag per value: the CLI accumulates repeats, and this keeps names
    // containing spaces unambiguous.
    for value in values {
        args.push(flag.into());
        args.push(value.clone());
    }
}

/// Arguments to uninstall `skill` from `scope`.
///
/// With no agents given the CLI cleans up every agent link for the skill.
pub fn remove_args(skill: &str, scope: &Scope, agents: &[String]) -> Vec<String> {
    let mut args = vec!["remove".to_string(), skill.to_string()];
    if !agents.is_empty() {
        push_multi(&mut args, "-a", agents);
    }
    if scope.is_global() {
        args.push("-g".into());
    }
    args.push("-y".into());
    args
}

/// Arguments to update `skills` in `scope`; an empty list updates everything.
pub fn update_args(skills: &[String], scope: &Scope) -> Vec<String> {
    let mut args = vec!["update".to_string()];
    args.extend(skills.iter().cloned());
    args.push(if scope.is_global() { "-g" } else { "-p" }.into());
    args.push("-y".into());
    args
}

/// The result of one CLI run.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// The command as typed, for display in the activity log.
    pub command: String,
    pub success: bool,
    pub stdout: String,
    /// Human-readable progress and errors; the CLI keeps these off stdout.
    pub stderr: String,
}

/// One skill's entry in `skills add --json` output.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub name: Option<String>,
    /// `installed`, `skipped` or `failed`.
    pub status: String,
    pub source: Option<String>,
    pub path: Option<String>,
    pub scope: Option<String>,
    #[serde(default)]
    pub agents: Vec<String>,
    pub error: Option<String>,
    pub reason: Option<String>,
}

impl Outcome {
    /// Parse `--json` install output, which is a single array on stdout.
    pub fn install_results(&self) -> Option<Vec<InstallResult>> {
        serde_json::from_str(self.stdout.trim()).ok()
    }
}

/// Run a `skills` subcommand to completion.
///
/// Blocking: call it from a background executor, not the UI thread.
pub fn run(
    launcher: &Launcher,
    args: &[String],
    cwd: &std::path::Path,
) -> std::io::Result<Outcome> {
    let (program, prefix) = launcher.program();
    let full: Vec<String> = prefix.iter().chain(args.iter()).cloned().collect();

    let output = Command::new(&program)
        .args(&full)
        .current_dir(cwd)
        // The CLI changes shape when it thinks a coding agent is driving it.
        // Skillshard is a GUI, so present as a plain non-interactive caller.
        .env_remove("CLAUDE_CODE")
        .env("CI", "1")
        .output()?;

    Ok(Outcome {
        command: format!("{program} {}", full.join(" ")),
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Directory a command for `scope` should run in.
///
/// Project commands must run inside the project so the CLI writes to the right
/// tree; global commands are run from home to keep them out of any project.
pub fn working_dir(scope: &Scope) -> PathBuf {
    match scope {
        Scope::Global => crate::paths::home(),
        Scope::Project(root) => root.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Scope {
        Scope::Project(PathBuf::from("/tmp/demo"))
    }

    #[test]
    fn install_is_fully_specified_and_non_interactive() {
        let request = InstallRequest {
            source: "vercel-labs/agent-skills".into(),
            scope: Scope::Global,
            skills: vec!["vercel-optimize".into(), "deploy-to-vercel".into()],
            agents: vec!["claude-code".into(), "codex".into()],
            copy: false,
        };
        assert_eq!(
            request.args(),
            [
                "add",
                "vercel-labs/agent-skills",
                "-s",
                "vercel-optimize",
                "-s",
                "deploy-to-vercel",
                "-a",
                "claude-code",
                "-a",
                "codex",
                "-g",
                "-y",
                "--json",
            ]
        );
    }

    #[test]
    fn empty_selections_mean_everything() {
        let request = InstallRequest {
            source: "owner/repo".into(),
            scope: project(),
            skills: vec![],
            agents: vec![],
            copy: true,
        };
        let args = request.args();
        assert!(args.windows(2).any(|w| w == ["-s", "*"]));
        assert!(args.windows(2).any(|w| w == ["-a", "*"]));
        assert!(args.contains(&"--copy".to_string()));
        // Project scope must not pass -g.
        assert!(!args.contains(&"-g".to_string()));
    }

    #[test]
    fn remove_and_update_target_the_right_scope() {
        assert_eq!(
            remove_args("my-skill", &Scope::Global, &[]),
            ["remove", "my-skill", "-g", "-y"]
        );
        assert_eq!(
            remove_args("my-skill", &project(), &["cursor".into()]),
            ["remove", "my-skill", "-a", "cursor", "-y"]
        );
        assert_eq!(
            update_args(&["a".into()], &Scope::Global),
            ["update", "a", "-g", "-y"]
        );
        assert_eq!(update_args(&[], &project()), ["update", "-p", "-y"]);
    }

    #[test]
    fn parses_the_json_install_report() {
        let outcome = Outcome {
            command: String::new(),
            success: true,
            stdout: r#"[{"name":"a","status":"installed","scope":"global",
                        "agents":["Claude Code"],"path":"/x"},
                       {"name":"b","status":"failed","error":"boom"}]"#
                .into(),
            stderr: "progress noise lives here".into(),
        };
        let results = outcome.install_results().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].status, "installed");
        assert_eq!(results[1].error.as_deref(), Some("boom"));
    }
}
