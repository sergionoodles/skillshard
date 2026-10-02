use super::Skillshard;
use crate::model::Scope;
use crate::usage::ServiceState;
use gpui_kit::component::Root;
use gpui_kit::{px, size, AppContext, TestAppContext};
use std::sync::atomic::Ordering;

#[gpui_kit::test]
fn stale_query_errors_cannot_replace_a_newer_usage_view(cx: &mut TestAppContext) {
    let directory =
        std::env::temp_dir().join(format!("skillshard-stale-query-{}", std::process::id()));
    cx.update(|cx| {
        crate::ui::init_with_usage(
            directory.join("preferences.json"),
            Err("Isolated test has no collector".into()),
            Vec::new(),
            cx,
        );
    });
    let mut view = None;
    cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let app = cx.new(|cx| Skillshard::with_scopes(vec![Scope::Project(directory)], window, cx));
        view = Some(app.clone());
        Root::new(app, window, cx)
    });
    cx.update(|cx| {
        view.unwrap().update(cx, |app, cx| {
            app.usage.control_revision.store(2, Ordering::Release);
            app.usage.query_revision.store(2, Ordering::Release);
            {
                let mut view = app.usage.view.lock().unwrap();
                view.status.state = ServiceState::Running;
                view.status.error = None;
                view.snapshot.counts.activations = 3;
            }
            for (query, control) in [(1, 2), (2, 1)] {
                app.finish_usage_query(Err("Stale query failed".into()), query, control, cx);
                let view = app.usage.current_view();
                assert_eq!(view.status.state, ServiceState::Running);
                assert!(view.status.error.is_none());
                assert_eq!(view.snapshot.counts.activations, 3);
            }
            app.finish_usage_query(Err("Current query failed".into()), 2, 2, cx);
            let view = app.usage.current_view();
            assert_eq!(view.status.state, ServiceState::Failed);
            assert_eq!(view.status.error.as_deref(), Some("Current query failed"));
        });
    });
}
