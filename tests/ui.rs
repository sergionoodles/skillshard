//! End-to-end interface tests.
//!
//! These drive the real window headlessly: render a frame, click actual
//! controls, and check the effect on disk. The fixture is a temporary project
//! tree, so the tests never touch the user's own skills.

use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{px, size, AppContext, TestAppContext};
use skillshard::model::{Scope, UpdateState};
use skillshard::ui::Skillshard;
use std::path::{Path, PathBuf};

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
    cx.update(gpui_kit::init);
    let scope = fixture("list", &["alpha", "beta"]);

    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| Skillshard::with_scopes(vec![scope.clone()], window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("row-alpha").is_some(), "alpha should be listed");
        assert!(window.try_find("row-beta").is_some(), "beta should be listed");
        // Both start enabled, so their switches are on.
        assert_eq!(window.find("toggle-alpha").checked(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_switch_disables_a_skill_on_disk_and_enables_it_again(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
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
        assert!(!canonical(&scope, "alpha").exists(), "canonical copy is moved");
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
    cx.update(gpui_kit::init);
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
    cx.update(gpui_kit::init);
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
fn the_install_dialog_opens_with_every_option_on_screen(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
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
    cx.update(gpui_kit::init);
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
        assert!(row.left() >= sidebar.right(), "list sits right of the sidebar");
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
) -> (
    gpui_kit::WindowHandle<Root>,
    gpui_kit::Entity<Skillshard>,
) {
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
    cx.update(gpui_kit::init);
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
    cx.update(gpui_kit::init);
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
        assert!(window.try_find("update-tag-alpha").is_some(), "worded badge");
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
        assert!(window.try_find("update-icon-alpha").is_some(), "arrow marker");
        assert!(window.try_find("update-tag-alpha").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn long_names_and_descriptions_stay_inside_the_row(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
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
