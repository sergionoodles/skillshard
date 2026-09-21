//! End-to-end interface tests.
//!
//! These drive the real window headlessly: render a frame, click actual
//! controls, and check the effect on disk. The fixture is a temporary project
//! tree, so the tests never touch the user's own skills.

use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{px, size, AppContext, TestAppContext};
use skillshard::agents::by_key;
use skillshard::model::{Scope, UpdateState};
use skillshard::tokens::estimate;
use skillshard::ui::Skillshard;
use std::path::{Path, PathBuf};

/// Initialise the app with a fresh preferences file, so tests never read or
/// write the user's own settings.
fn init(cx: &mut TestAppContext) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "skillshard-ui-prefs-{}-{n}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    cx.update(|cx| skillshard::ui::init(path, cx));
}

/// A throwaway project tree containing `names` as canonical skills.
fn fixture(tag: &str, names: &[&str]) -> Scope {
    let root = std::env::temp_dir().join(format!("skillshard-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let scope = Scope::Project(root);
    for name in names {
        let dir = scope.canonical_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Description of {name}\n---\n"),
        )
        .unwrap();
    }
    scope
}

/// Link a skill into an agent's own directory.
fn link(scope: &Scope, agent_dir: &str, name: &str) {
    let Scope::Project(root) = scope else {
        unreachable!()
    };
    let dir = root.join(agent_dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(scope.canonical_dir().join(name), dir.join(name)).unwrap();
}

fn canonical(scope: &Scope, name: &str) -> PathBuf {
    scope.canonical_dir().join(name)
}

fn disabled(scope: &Scope, name: &str) -> PathBuf {
    scope.disabled_dir().join(name)
}

#[gpui_kit::test]
fn lists_every_skill_in_the_active_scope(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("list", &["alpha", "beta"]);

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("row-alpha").is_some(),
            "alpha should be listed"
        );
        assert!(
            window.try_find("row-beta").is_some(),
            "beta should be listed"
        );
        // Both start enabled, so their switches are on.
        assert_eq!(window.find("toggle-alpha").checked(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_switch_disables_a_skill_on_disk_and_enables_it_again(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("toggle", &["alpha"]);
    link(&scope, ".claude/skills", "alpha");

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    let claude_link = match &scope {
        Scope::Project(root) => root.join(".claude/skills/alpha"),
        _ => unreachable!(),
    };

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(canonical(&scope, "alpha").is_dir());

        window.click("toggle-alpha", cx);
        window.render_frame(cx);

        // Disabling parks the files and takes them out of the agent's reach,
        // without uninstalling anything.
        assert!(
            !canonical(&scope, "alpha").exists(),
            "canonical copy is moved"
        );
        assert!(disabled(&scope, "alpha").join("SKILL.md").is_file());
        assert!(!Path::new(&claude_link).exists(), "agent link is removed");
        assert_eq!(window.find("toggle-alpha").checked(), Some(false));

        window.click("toggle-alpha", cx);
        window.render_frame(cx);

        // Re-enabling restores both the files and the agent that had it.
        assert!(canonical(&scope, "alpha").is_dir());
        assert!(!disabled(&scope, "alpha").exists());
        assert!(
            std::fs::symlink_metadata(&claude_link).is_ok(),
            "the agent link comes back"
        );
        assert_eq!(window.find("toggle-alpha").checked(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn selecting_a_skill_reveals_its_agent_checkboxes(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("detail", &["alpha"]);
    link(&scope, ".claude/skills", "alpha");

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // Nothing is selected yet, so no agent controls are on screen.
        assert!(window.try_find("agent-claude-code").is_none());

        window.click("row-alpha", cx);
        window.render_frame(cx);

        let claude = window.find("agent-claude-code");
        assert_eq!(claude.checked(), Some(true), "Claude Code has the skill");
    })
    .unwrap();
}

#[gpui_kit::test]
fn unchecking_an_agent_removes_only_that_agents_link(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("agents", &["alpha"]);
    link(&scope, ".claude/skills", "alpha");
    link(&scope, ".windsurf/skills", "alpha");

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    let Scope::Project(root) = scope.clone() else {
        unreachable!()
    };

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);

        window.click("agent-claude-code", cx);
        window.render_frame(cx);

        assert!(!root.join(".claude/skills/alpha").exists(), "unlinked");
        assert!(
            std::fs::symlink_metadata(root.join(".windsurf/skills/alpha")).is_ok(),
            "other agents are untouched"
        );
        // The skill itself is still installed.
        assert!(canonical(&scope, "alpha").is_dir());
        assert_eq!(window.find("agent-claude-code").checked(), Some(false));
    })
    .unwrap();
}

#[gpui_kit::test]
fn agents_reading_the_canonical_dir_show_a_locked_checkbox(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("shared", &["alpha"]);
    link(&scope, ".claude/skills", "alpha");
    let handle = open_with_agents(
        vec![scope.clone()],
        &["claude-code", "codex", "cursor", "opencode", "pi"],
        cx,
    );

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);

        assert_eq!(window.find("agent-shared").checked(), Some(true));
        // Many agents read .agents/skills; beyond the first few they collapse.
        assert!(window.try_find("agent-shared-more").is_some());
        // It leads the list, ahead of the agents that can be toggled.
        assert!(
            window.find("agent-shared").bounds().origin.y
                < window.find("agent-claude-code").bounds().origin.y
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_copy_in_a_shared_readers_own_directory_is_listed_for_that_agent(cx: &mut TestAppContext) {
    init(cx);
    // Pi reads .agents/skills too, but this skill lives only in .pi/skills.
    let scope = fixture("own-copy", &[]);
    let Scope::Project(root) = scope.clone() else {
        unreachable!()
    };
    let dir = root.join(".pi/skills/solo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: solo\ndescription: d\n---\n",
    )
    .unwrap();

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-solo", cx);
        window.render_frame(cx);

        assert_eq!(window.find("agent-pi").checked(), Some(true));
        // Nothing is in .agents/skills, so no agent sees it through there.
        assert!(window.try_find("agent-shared").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_install_dialog_opens_with_every_option_on_screen(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("install", &["alpha"]);
    link(&scope, ".claude/skills", "alpha");

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("install", cx);
        window.render_frame(cx);

        // Every choice the CLI would prompt for is present up front, so the
        // command can run without an interactive terminal.
        assert!(window.try_find("source").is_some(), "source field");
        assert!(window.try_find("skills").is_some(), "skill selection");
        assert!(window.try_find("scope").is_some(), "global/project switch");
        assert!(window.try_find("copy").is_some(), "symlink/copy switch");
        assert!(window.try_find("run-review").is_some(), "security review");
        // Installing is refused until a source is given: the dialog stays put
        // rather than launching an empty command.
        window.click("confirm", cx);
        window.render_frame(cx);
        assert!(window.try_find("source").is_some(), "dialog stays open");

        window.click("cancel", cx);
    })
    .unwrap();

    // Closing is driven by an event from the dialog to the main window, which
    // is delivered on the next effect cycle.
    cx.run_until_parked();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("source").is_none(), "dialog closes");
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_three_panes_are_laid_out_side_by_side(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("layout", &["alpha"]);
    link(&scope, ".claude/skills", "alpha");

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);

        let sidebar = window.find("scope-0").bounds();
        let row = window.find("row-alpha").bounds();
        let agent = window.find("agent-claude-code").bounds();
        let search = window.find("search").bounds();

        // Every pane occupies real space.
        assert!(sidebar.size.width > px(0.), "sidebar has width");
        assert!(row.size.width > px(0.), "list has width");
        assert!(agent.size.width > px(0.), "detail has width");

        // Left to right: sidebar, then the list, then the detail pane.
        assert!(
            row.left() >= sidebar.right(),
            "list sits right of the sidebar"
        );
        assert!(agent.left() >= row.right(), "detail sits right of the list");

        // The list starts below the header rather than under it.
        assert!(row.top() >= search.bottom(), "list is below the header");

        // Nothing spills outside the window.
        assert!(agent.right() <= px(1180.), "detail fits the window");
    })
    .unwrap();
}

/// Open the window and keep a handle on the view, so tests can push state in.
fn open(
    scope: &Scope,
    width: f32,
    cx: &mut TestAppContext,
) -> (gpui_kit::WindowHandle<Root>, gpui_kit::Entity<Skillshard>) {
    let scope = scope.clone();
    let mut view = None;
    let handle = cx.open_window(size(px(width), px(760.)), |window, cx| {
        let created = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        view = Some(created.clone());
        Root::new(created, window, cx)
    });
    (handle, view.unwrap())
}

#[gpui_kit::test]
fn the_on_off_switch_sits_at_the_end_of_the_row(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("switch-side", &["alpha"]);
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let row = window.find("row-alpha").bounds();
        let switch = window.find("toggle-alpha").bounds();

        // Trailing edge, not leading: the switch is in the right-hand half and
        // hugs the end of the row.
        assert!(
            switch.left() > row.left() + row.size.width / 2.,
            "switch should be on the right"
        );
        assert!(switch.right() <= row.right(), "switch stays inside the row");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_wide_list_labels_the_update_and_a_narrow_one_uses_an_arrow(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("update-badge", &["alpha"]);

    // Wide window: there is room for the worded badge.
    let (handle, view) = open(&scope, 1180., cx);
    cx.update(|cx| {
        view.update(cx, |this, cx| {
            this.apply_update_states(vec![("alpha".into(), UpdateState::Available)], cx);
        })
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("update-tag-alpha").is_some(),
            "worded badge"
        );
        assert!(window.try_find("update-icon-alpha").is_none());
    })
    .unwrap();

    // Narrow window: the badge collapses to an arrow.
    let (narrow_handle, narrow_view) = open(&scope, 640., cx);
    cx.update(|cx| {
        narrow_view.update(cx, |this, cx| {
            this.apply_update_states(vec![("alpha".into(), UpdateState::Available)], cx);
        })
    });
    cx.update_window(narrow_handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("update-icon-alpha").is_some(),
            "arrow marker"
        );
        assert!(window.try_find("update-tag-alpha").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn long_names_and_descriptions_stay_inside_the_row(cx: &mut TestAppContext) {
    init(cx);
    let root = std::env::temp_dir().join(format!("skillshard-ui-trunc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let scope = Scope::Project(root);
    let dir = scope.canonical_dir().join("alpha");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {}\ndescription: {}\n---\n",
            "an-extremely-long-skill-name-that-would-otherwise-push-the-row-wide".repeat(2),
            "A very long description ".repeat(40)
        ),
    )
    .unwrap();

    let (handle, _view) = open(&scope, 900., cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let row = window.find("row-alpha").bounds();
        // However long the text, the row keeps to the window and the switch
        // is still reachable at its end.
        assert!(row.right() <= px(900.), "row does not overflow the window");
        let switch = window.find("toggle-alpha").bounds();
        assert!(switch.right() <= row.right(), "switch stays visible");
        assert!(switch.size.width > px(0.), "switch is not squeezed away");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_long_description_scrolls_on_its_own_and_the_controls_stay_on_screen(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("long-detail", &[]);
    let dir = scope.canonical_dir().join("alpha");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: alpha\ndescription: {}\n---\n",
            "A very long description ".repeat(200)
        ),
    )
    .unwrap();
    link(&scope, ".claude/skills", "alpha");

    // The detail pane's fixed lower half — metadata, context cost, actions,
    // agents — needs this much before the description starts giving up height.
    let height = 620.;
    let handle = cx.open_window(size(px(1180.), px(height)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);

        let description = window.find("detail-description").bounds();
        let uninstall = window.find("remove").bounds();
        let agent = window.find("agent-claude-code").bounds();
        assert!(
            uninstall.top() >= description.bottom(),
            "buttons sit below the description"
        );
        assert!(
            agent.bottom() <= px(height),
            "agent checkboxes stay inside the window"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_skill_from_a_remote_repo_links_to_it(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("repo-link", &["alpha"]);
    let Scope::Project(root) = scope.clone() else {
        unreachable!()
    };
    std::fs::write(
        root.join("skills-lock.json"),
        r#"{"skills":{"alpha":{"source":"owner/repo","sourceType":"github","sourceUrl":"https://github.com/owner/repo.git"}}}"#,
    )
    .unwrap();
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);
        window.click("source-link", cx);
    })
    .unwrap();

    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/owner/repo")
    );
}

#[gpui_kit::test]
fn a_local_skill_has_no_repo_link(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("no-repo-link", &["alpha"]);
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);
        assert!(window.try_find("source-link").is_none());
        assert!(window.try_find("location-link").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_badge_collapses_when_the_list_is_narrow_even_with_nothing_selected(cx: &mut TestAppContext) {
    // Regression: the detail pane is always on screen, but the width check
    // only counted it once a skill was selected, overestimating the list.
    init(cx);
    let scope = fixture("badge-unselected", &["alpha"]);
    let (handle, view) = open(&scope, 900., cx);
    cx.update(|cx| {
        view.update(cx, |this, cx| {
            this.apply_update_states(vec![("alpha".into(), UpdateState::Available)], cx);
        })
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let list = window.find("row-alpha").bounds().size.width;
        assert!(
            list < px(460.),
            "fixture must leave a narrow list, got {list:?}"
        );
        assert!(
            window.try_find("update-icon-alpha").is_some(),
            "arrow marker"
        );
        assert!(window.try_find("update-tag-alpha").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn changing_preferences_re_themes_the_open_window(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("retheme", &["alpha"]);
    let (_handle, _view) = open(&scope, 1180., cx);

    cx.update(|cx| {
        skillshard::preferences::update(cx, |p| {
            p.appearance = skillshard::preferences::Appearance::Dark;
            p.dark_theme = "Tokyo Night".into();
        })
    });
    cx.run_until_parked();

    cx.update(|cx| {
        let theme = gpui_kit::component::Theme::global(cx);
        assert!(theme.is_dark());
        assert_eq!(theme.theme_name().as_ref(), "Tokyo Night");
    });
}

#[gpui_kit::test]
fn the_settings_modal_opens_from_the_header_and_closes(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("settings", &["alpha"]);
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-settings", cx);
        window.render_frame(cx);
        assert!(window.try_find("close-settings").is_some(), "modal is open");
        window.click("close-settings", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("close-settings").is_none(), "modal closed");
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_install_dialog_starts_from_the_configured_defaults(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("defaults", &["alpha"]);
    cx.update(|cx| {
        skillshard::preferences::update(cx, |p| {
            p.install_global = false;
            p.install_copy = true;
        })
    });
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("install", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("scope").checked(),
            Some(false),
            "project by default"
        );
        assert_eq!(window.find("copy").checked(), Some(true), "copy by default");
    })
    .unwrap();
}

#[gpui_kit::test]
fn local_repository_skills_are_listed_and_fill_the_form(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("local-install", &["alpha"]);
    let repo = std::env::temp_dir().join(format!("skillshard-ui-repo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    let skill_dir = repo.join("skills/pdf-tools");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: pdf-tools\ndescription: Split PDFs\n---\n",
    )
    .unwrap();
    cx.update(|cx| {
        skillshard::preferences::update(cx, |p| p.local_repositories = vec![repo.clone()])
    });
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("install", cx);
    })
    .unwrap();
    // Discovery runs in the background.
    cx.run_until_parked();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("local-0", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("source").value(),
            Some(skill_dir.display().to_string().as_str()),
            "picking a local skill installs from its directory"
        );
    })
    .unwrap();
}

/// A second fixture project, saved in preferences so it appears in the
/// sidebar after the first.
fn saved_project(tag: &str, skill: &str, cx: &mut TestAppContext) -> PathBuf {
    let Scope::Project(root) = fixture(tag, &[skill]) else {
        unreachable!()
    };
    cx.update(|cx| {
        skillshard::preferences::update(cx, |p| {
            p.project_mut(&root);
        })
    });
    root
}

#[gpui_kit::test]
fn sidebar_entries_are_left_aligned(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("align", &["alpha"]);
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let row = window.find("scope-0").bounds();
        let label = window.find("scope-0-label").bounds();
        // Icon plus padding, not half the row: centred text would start
        // well over 100px in on a 300px sidebar.
        assert!(
            label.left() - row.left() < px(40.),
            "label starts {:?} into the row",
            label.left() - row.left()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn saved_projects_are_restored_in_the_sidebar(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("restore-a", &["alpha"]);
    saved_project("restore-b", "beta", cx);
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("scope-1", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("row-beta").is_some(),
            "the saved project opens"
        );
    })
    .unwrap();
}

/// Open the window on `scopes`, with `installed` standing in for the agents
/// detected on this machine.
fn open_with_agents(
    scopes: Vec<Scope>,
    installed: &[&str],
    cx: &mut TestAppContext,
) -> gpui_kit::WindowHandle<Root> {
    let installed: Vec<_> = installed.iter().map(|k| by_key(k).unwrap()).collect();
    cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx
            .new(|cx| Skillshard::with_scopes(scopes, window, cx).with_installed_agents(installed));
        Root::new(view, window, cx)
    })
}

/// Open `button`'s menu for a skill in one project, with a second project
/// saved, and check it lists global, a divider, then both projects.
fn assert_scope_menu(button: &'static str, cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture(&format!("{button}-from"), &["alpha"]);
    let other = saved_project(&format!("{button}-to"), "beta", cx);
    let handle = open_with_agents(vec![scope.clone(), Scope::Global], &[], cx);
    let label = |path: &Path| path.file_name().unwrap().to_string_lossy().into_owned();
    let Scope::Project(root) = &scope else {
        unreachable!()
    };

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);
        window.click(button, cx);
        window.render_frame(cx);

        let menu = window.within("popup-menu");
        assert_eq!(menu.find(0usize).label(), Some("Global"));
        // Global is set apart by a divider, so the projects start after it.
        assert!(menu.find(1usize).label().is_none());
        assert_eq!(menu.find(2usize).label(), Some(label(root).as_str()));
        assert_eq!(menu.find(3usize).label(), Some(label(&other).as_str()));
    })
    .unwrap();
}

#[gpui_kit::test]
fn move_to_offers_global_first_then_every_project(cx: &mut TestAppContext) {
    assert_scope_menu("move-to", cx);
}

#[gpui_kit::test]
fn copy_to_offers_global_first_then_every_project(cx: &mut TestAppContext) {
    assert_scope_menu("copy-to", cx);
}

#[gpui_kit::test]
fn sync_is_gone_and_uninstall_is_called_remove(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("actions", &["alpha"]);
    let handle = open_with_agents(vec![scope.clone()], &[], cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);
        assert!(window.try_find("sync").is_none());
        assert_eq!(window.find("remove").label(), Some("Remove"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn copying_to_a_project_goes_through_the_menu_item(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("copy-from", &["alpha"]);
    let other = saved_project("copy-to", "beta", cx);
    let handle = open_with_agents(vec![scope.clone(), Scope::Global], &[], cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);
        window.click("copy-to", cx);
        window.render_frame(cx);
        window.within("popup-menu").click(3usize, cx);
        window.render_frame(cx);

        // The fixture has no lock file, so the copy stops before running the
        // CLI — with a message naming the action that was picked.
        assert_eq!(
            window.find("status-text").label(),
            Some("alpha has no recorded source to copy it from")
        );
        assert!(canonical(&scope, "alpha").is_dir());
        assert!(!other.join(".agents/skills/alpha").exists());
    })
    .unwrap();
}

#[gpui_kit::test]
fn only_installed_agents_are_listed(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("installed", &["alpha"]);
    let Scope::Project(root) = &scope else {
        unreachable!()
    };
    // `skills/` is OpenClaw's project directory, but a project having one
    // does not mean OpenClaw is installed.
    std::fs::create_dir_all(root.join("skills")).unwrap();
    let handle = open_with_agents(vec![scope.clone()], &["claude-code", "codex"], cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);

        assert_eq!(window.find("agent-claude-code").checked(), Some(false));
        assert!(window.try_find("agent-openclaw").is_none());
        // Codex is the only installed agent reading .agents/skills, so the
        // shared row does not pad itself out with the rest.
        assert!(window.try_find("agent-shared").is_some());
        assert!(window.try_find("agent-shared-more").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_agent_holding_a_skill_is_listed_even_when_not_detected(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("undetected", &["alpha"]);
    link(&scope, ".windsurf/skills", "alpha");
    let handle = open_with_agents(vec![scope.clone()], &[], cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);
        // Its link is on disk, so there is something to untick.
        assert_eq!(window.find("agent-windsurf").checked(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn project_settings_save_the_icon_and_colour(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("appearance-a", &["alpha"]);
    let root = saved_project("appearance-b", "beta", cx);
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("project-settings-1", cx);
        window.render_frame(cx);
        // The settings button must not also select the project row it sits in.
        assert!(
            window.try_find("row-alpha").is_some(),
            "selection unchanged"
        );

        window.click("icon-rocket", cx);
        window.click("color-blue", cx);
    })
    .unwrap();

    cx.update(|cx| {
        let prefs = skillshard::preferences::get(cx);
        let project = prefs.project(&root).expect("project is saved");
        assert_eq!(project.icon.as_deref(), Some("rocket"));
        assert_eq!(project.color.as_deref(), Some("blue"));
    });
}

#[gpui_kit::test]
fn removing_the_open_project_falls_back_and_keeps_its_files(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("remove-a", &["alpha"]);
    let root = saved_project("remove-b", "beta", cx);
    let (handle, _view) = open(&scope, 1180., cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("scope-1", cx);
        window.render_frame(cx);
        assert!(window.try_find("row-beta").is_some());
        window.click("project-settings-1", cx);
        window.render_frame(cx);
        window.click("remove-project", cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("scope-1").is_none(),
            "gone from the sidebar"
        );
        assert!(
            window.try_find("row-alpha").is_some(),
            "back on the first scope"
        );
        assert!(window.try_find("remove-project").is_none(), "modal closed");
    })
    .unwrap();
    cx.update(|cx| assert!(skillshard::preferences::get(cx).project(&root).is_none()));
    assert!(
        root.join(".agents/skills/beta/SKILL.md").is_file(),
        "removing a project never deletes its files"
    );
}

#[gpui_kit::test]
fn the_detail_pane_breaks_a_skill_s_context_cost_into_three_figures(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("cost", &[]);
    let dir = scope.canonical_dir().join("alpha");
    std::fs::create_dir_all(dir.join("references")).unwrap();
    // Sized so each figure lands in a different bucket, and so the bundled
    // file dwarfs the body it sits next to.
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: alpha\ndescription: {}\n---\n\n{}",
            "word ".repeat(20),
            "body ".repeat(100)
        ),
    )
    .unwrap();
    std::fs::write(dir.join("references/extra.md"), "detail ".repeat(1000)).unwrap();
    link(&scope, ".claude/skills", "alpha");

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-alpha", cx);
        window.render_frame(cx);

        // Name plus description, body, and references, at roughly 3.7
        // characters per token.
        assert_eq!(
            window.find("cost-always").label(),
            Some("Always ≈ 29 tokens")
        );
        assert_eq!(
            window.find("cost-trigger").label(),
            Some("Trigger ≈ 136 tokens")
        );
        assert_eq!(
            window.find("cost-bundled").label(),
            Some("Bundled ≈ 1.8k tokens")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn each_list_row_carries_the_compact_cost_figures(cx: &mut TestAppContext) {
    init(cx);
    let scope = fixture("row-cost", &["alpha"]);
    link(&scope, ".claude/skills", "alpha");

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let always = estimate("alpha") + estimate("Description of alpha");
        // The fixture skill has no body and no bundled files, so only the
        // first figure has anything to report.
        assert_eq!(
            window.find("cost-alpha").label(),
            Some(format!("{always} always, — on trigger, — bundled").as_str())
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_sidebar_totals_the_skills_an_agent_actually_loads(cx: &mut TestAppContext) {
    init(cx);
    // `linked` and `canonical` both reach an agent — the second through the
    // agents that read `.agents/skills` directly. `parked` is disabled, so it
    // is in no system prompt and must not be counted.
    let scope = fixture("always-total", &["linked", "canonical", "parked"]);
    link(&scope, ".claude/skills", "linked");
    std::fs::create_dir_all(scope.disabled_dir()).unwrap();
    std::fs::rename(canonical(&scope, "parked"), disabled(&scope, "parked")).unwrap();

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let expected = estimate("linked")
            + estimate("Description of linked")
            + estimate("canonical")
            + estimate("Description of canonical");
        assert_eq!(
            window.find("always-total").label(),
            Some(format!("{expected} tokens always loaded").as_str())
        );
    })
    .unwrap();
}
