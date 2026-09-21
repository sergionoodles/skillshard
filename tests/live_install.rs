//! A real install through the `skills` CLI, into a throwaway project.
//!
//! This is the check that matters most for the install flow: the arguments
//! Skillshard builds must drive the CLI from start to finish without it
//! stopping to ask anything. Ignored by default because it downloads:
//!     cargo test --test live_install -- --ignored --nocapture

use skillshard::model::Scope;
use skillshard::scan;
use skillshard::skills_cli::{self, InstallRequest, Launcher};

#[test]
#[ignore = "requires network and npx"]
fn installs_a_real_skill_without_any_prompt() {
    let root = std::env::temp_dir().join(format!("skillshard-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let scope = Scope::Project(root.clone());

    let request = InstallRequest {
        source: "vercel-labs/agent-skills".into(),
        scope: scope.clone(),
        skills: vec!["deploy-to-vercel".into()],
        agents: vec!["claude-code".into()],
        copy: false,
    };

    let launcher = Launcher::detect();
    let args = request.args();
    println!("running: {args:?}");

    let outcome = skills_cli::run(&launcher, &args, &root).expect("failed to spawn the CLI");
    println!("stderr:\n{}", outcome.stderr);
    println!("stdout:\n{}", outcome.stdout);
    assert!(outcome.success, "install failed");

    // The JSON contract: exactly one array on stdout, whatever noise went to stderr.
    let results = outcome
        .install_results()
        .expect("stdout should be a JSON array");
    assert!(
        results.iter().any(|r| r.status == "installed"),
        "expected an installed skill, got {results:?}"
    );

    // And Skillshard sees it where the CLI put it.
    let skills = scan::scan(&scope);
    let installed = skills
        .iter()
        .find(|s| s.name == "deploy-to-vercel")
        .expect("scanner should find the new skill");
    assert!(!installed.description.is_empty());
    assert!(
        installed.is_enabled_for(skillshard::agents::by_key("claude-code").unwrap()),
        "should be linked into Claude Code"
    );

    let _ = std::fs::remove_dir_all(&root);
}
