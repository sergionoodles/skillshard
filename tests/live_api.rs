//! Checks against the real skills.sh and GitHub services.
//!
//! Ignored by default so the normal test run stays offline and fast:
//!     cargo test --test live_api -- --ignored
use skillshard::{lock, model::Scope, registry, updates};

#[test]
#[ignore = "requires network"]
fn searches_the_skill_directory() {
    let hits = registry::search("react", 3).expect("search failed");
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|h| !h.source.is_empty()));
}

#[test]
#[ignore = "requires network"]
fn fetches_a_real_security_review() {
    let audit = registry::audit(
        "vercel-labs/agent-skills",
        &[
            "vercel-optimize".to_string(),
            "deploy-to-vercel".to_string(),
        ],
    )
    .expect("audit failed");
    let optimize = audit.get("vercel-optimize").expect("missing skill");
    assert!(!optimize.partners.is_empty(), "expected partner verdicts");
}

#[test]
#[ignore = "requires network"]
fn checks_update_state_for_locally_installed_skills() {
    for (name, entry) in lock::read(&Scope::Global) {
        if entry.github_owner_repo().is_none() {
            continue;
        }
        println!("{name:<20} {:?}", updates::check(&entry));
    }
}

#[test]
#[ignore = "requires network"]
fn detects_a_stale_hash_as_an_available_update() {
    // Same real skill, but with the lock pinned to a hash that is not current.
    let mut entry = lock::read(&Scope::Global)
        .remove("ponytail")
        .expect("ponytail must be installed for this check");
    assert_eq!(
        updates::check(&entry),
        skillshard::model::UpdateState::UpToDate
    );

    entry.hash = Some("0000000000000000000000000000000000000000".into());
    assert_eq!(
        updates::check(&entry),
        skillshard::model::UpdateState::Available
    );
}

#[test]
#[ignore = "requires network"]
fn finds_install_counts_for_locally_installed_skills() {
    let mut found = 0;
    for (name, entry) in lock::read(&Scope::Global) {
        let count = registry::fetch_install_count(&entry.source, &name).expect("search failed");
        println!("{name:<20} {:?}", count.map(registry::format_count));
        found += count.is_some() as usize;
    }
    assert!(
        found > 0,
        "expected at least one installed skill to be listed"
    );
}
