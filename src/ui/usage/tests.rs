use super::chart::bar_height;
use super::labels::{identity_label, timestamp_label};
use super::{UsageEvent, UsageScope, UsageView};
use crate::preferences::TrackingDays;
use crate::usage::{Counts, Metric, Query, Ranking, RoleFilter, Snapshot, Status};
use gpui_kit::component::input::InputState;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{px, size, AnyWindowHandle, AppContext, Entity, TestAppContext};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[test]
fn install_context_displays_scope_and_source_without_serialized_identity() {
    assert_eq!(
        identity_label(r#"["global","example",null]"#),
        "Global install"
    );
    assert_eq!(
        identity_label(r#"["/work/project","example",["owner/repository","skills/example"]]"#),
        "Project: /work/project · owner/repository · skills/example"
    );
    assert_eq!(
        identity_label(r#"["global","example",["owner/repository",null]]"#),
        "Global install · owner/repository"
    );
}

#[test]
fn copy_context_displays_agent_and_full_path() {
    let identity = serde_json::to_string(&(
        r#"["global","example",null]"#,
        "claude-code",
        "/work/copies/example",
    ))
    .unwrap();
    assert_eq!(
        identity_label(&identity),
        "Global install · Claude Code copy: /work/copies/example"
    );
}

#[test]
fn malformed_context_has_readable_fallback() {
    for invalid in ["not JSON", "[]", r#"["broken base","codex","/copy"]"#] {
        assert_eq!(identity_label(invalid), "Install context unavailable");
    }
}

#[test]
fn bars_scale_to_the_busiest_day_and_small_days_stay_visible() {
    assert_eq!(bar_height(0, 0), 0.);
    assert_eq!(bar_height(0, 4), 0.);
    assert_eq!(bar_height(4, 4), bar_height(8, 8));
    assert_eq!(bar_height(2, 4), bar_height(4, 4) / 2.);
    assert!(bar_height(1, 10_000) > 0.);
    assert_eq!(timestamp_label(None), "Unavailable");
    assert_eq!(timestamp_label(Some(i64::MAX)), "Unavailable");
}

fn ranking(skill_id: i64, name: &str, activations: u64) -> Ranking {
    Ranking {
        skill_id,
        name: name.into(),
        identity: r#"["global","x",null]"#.into(),
        counts: Counts {
            activations,
            ..Counts::default()
        },
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        counts: Counts {
            activations: 7,
            ..Counts::default()
        },
        rankings: vec![ranking(1, "alpha", 2), ranking(2, "beta", 5)],
        ..Snapshot::default()
    }
}

/// A usage view in its own window, recording every query it asks for.
fn open(
    cx: &mut TestAppContext,
    tag: &str,
) -> (AnyWindowHandle, Entity<UsageView>, Rc<RefCell<Vec<Query>>>) {
    let preferences = std::env::temp_dir().join(format!(
        "skillshard-usage-{tag}-{}.json",
        std::process::id()
    ));
    cx.update(|cx| crate::ui::init(preferences, cx));
    let mut view = None;
    let handle = cx.open_window(size(px(1180.), px(1600.)), |window, cx| {
        let search = cx.new(|cx| InputState::new(window, cx));
        let usage = cx.new(|cx| {
            UsageView::new(
                snapshot(),
                Status::default(),
                TrackingDays::Fifteen,
                search,
                cx,
            )
        });
        view = Some(usage.clone());
        Root::new(usage, window, cx)
    });
    let view = view.unwrap();
    let queries = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let queries = queries.clone();
        cx.subscribe(&view, move |_, event, _| {
            if let UsageEvent::QueryChanged(query) = event {
                queries.borrow_mut().push(query.clone());
            }
        })
        .detach();
    });
    (handle.into(), view, queries)
}

#[gpui_kit::test]
fn the_most_used_skill_leads_the_view(cx: &mut TestAppContext) {
    let (handle, _, _) = open(cx, "top");

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("usage-top-skill-name").label(), Some("beta"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn sidebar_filters_publish_queries(cx: &mut TestAppContext) {
    let (handle, view, queries) = open(cx, "filters");

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("usage-role-subagents", cx);
        window.render_frame(cx);
        window.click("usage-metric-conversations", cx);
        window.render_frame(cx);
        window.click(("usage-range", 7usize), cx);
    })
    .unwrap();
    cx.run_until_parked();

    let queries = queries.borrow();
    assert_eq!(queries.len(), 3);
    assert_eq!(queries[0].role, RoleFilter::Subagents);
    assert_eq!(queries[1].metric, Metric::Conversations);
    assert_eq!(queries[2].range_days, Some(TrackingDays::Seven));
    assert!(queries[2].since.is_some());
    drop(queries);
    cx.update(|cx| {
        let range = view.read(cx).query().range_days;
        assert_eq!(range, Some(TrackingDays::Seven));
    });
}

#[gpui_kit::test]
fn ranges_beyond_the_kept_history_cannot_be_picked(cx: &mut TestAppContext) {
    let (handle, view, queries) = open(cx, "range");

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(("usage-range", 30usize), cx);
    })
    .unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.update_tracking_days(TrackingDays::Seven, cx)
        })
    });

    let queries = queries.borrow();
    assert_eq!(queries.len(), 1, "only the shrunk history re-queries");
    assert_eq!(queries[0].range_days, Some(TrackingDays::Seven));
}

#[gpui_kit::test]
fn picking_skills_narrows_the_query_and_show_all_clears_it(cx: &mut TestAppContext) {
    let (handle, _, queries) = open(cx, "picker");

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(("usage-skill", 1u64), cx);
        window.render_frame(cx);
        window.click(("usage-rank", 2u64), cx);
        window.render_frame(cx);
        assert_eq!(window.find(("usage-skill", 2u64)).checked(), Some(true));
        window.click("usage-clear-skills", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let picked: Vec<Vec<i64>> = queries
        .borrow()
        .iter()
        .map(|q| q.skill_ids.clone())
        .collect();
    assert_eq!(picked, vec![vec![1], vec![1, 2], vec![]]);
}

#[gpui_kit::test]
fn a_new_scope_re_queries_for_that_project_only(cx: &mut TestAppContext) {
    let (_, view, queries) = open(cx, "scope");
    let project = PathBuf::from("/work/project");
    let scope = UsageScope {
        project: Some(project.clone()),
        label: "project".into(),
    };

    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.set_scope(scope.clone(), Vec::new(), cx);
            view.set_scope(scope, Vec::new(), cx);
        })
    });

    let queries = queries.borrow();
    assert_eq!(queries.len(), 1, "an unchanged scope does not re-query");
    assert_eq!(queries[0].project, Some(project));
}
