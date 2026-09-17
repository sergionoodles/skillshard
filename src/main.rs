//! Skillshard — manage AI coding agent skills.

use gpui_kit::*;
use skillshard::ui::Skillshard;

fn main() {
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);

    app.run(move |cx| {
        gpui_kit::init(cx);

        // Computed before the spawn: display metrics need a synchronous App.
        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);

        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Skillshard".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    // Adopt the system light/dark setting, and keep following
                    // it if the user changes it while the app is open.
                    component::Theme::sync_system_appearance(Some(window), cx);
                    window
                        .observe_window_appearance(|window, cx| {
                            component::Theme::sync_system_appearance(Some(window), cx);
                        })
                        .detach();

                    let view = cx.new(|cx| Skillshard::new(window, cx));
                    cx.new(|cx| component::Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });
}
