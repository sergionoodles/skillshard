//! skillshard — manage AI coding agent skills.

use gpui_kit::*;
use skillshard::ui::Skillshard;

fn main() {
    let app = gpui_kit::application().with_assets(skillshard::assets::Assets);

    app.run(move |cx| {
        skillshard::ui::init(skillshard::preferences::default_path(), cx);

        // Computed before the spawn: display metrics need a synchronous App.
        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);

        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("skillshard".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| Skillshard::new(window, cx));
                    cx.new(|cx| component::Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });
}
