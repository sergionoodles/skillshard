//! The bundled themes, loaded through the real GPUI Kit registry.

use gpui_kit::component::{Theme, ThemeRegistry};
use gpui_kit::{div, px, size, AppContext, IntoElement, Render, TestAppContext, Window};
use skillshard::preferences::{self, Appearance};
use skillshard::themes;

struct Blank;
impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        div()
    }
}

fn setup(tag: &str, cx: &mut TestAppContext) {
    let path = std::env::temp_dir().join(format!(
        "skillshard-themes-{tag}-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    cx.update(|cx| {
        gpui_kit::init(cx);
        themes::register(cx).unwrap();
        preferences::init(path, cx);
    });
}

#[gpui_kit::test]
fn every_offered_theme_exists_with_the_right_mode(cx: &mut TestAppContext) {
    setup("offered", cx);
    cx.update(|cx| {
        let registry = ThemeRegistry::global(cx).themes();
        for name in themes::DARK {
            let theme = registry
                .get(*name)
                .unwrap_or_else(|| panic!("{name} missing"));
            assert!(theme.mode.is_dark(), "{name} should be dark");
        }
        for name in themes::LIGHT {
            let theme = registry
                .get(*name)
                .unwrap_or_else(|| panic!("{name} missing"));
            assert!(!theme.mode.is_dark(), "{name} should be light");
        }
        assert_eq!(themes::DARK.len(), 4);
        assert_eq!(themes::LIGHT.len(), 4);
    });
}

#[gpui_kit::test]
fn a_forced_appearance_applies_the_matching_theme(cx: &mut TestAppContext) {
    setup("forced", cx);
    let window = cx.open_window(size(px(200.), px(200.)), |_, _| Blank);

    cx.update_window(window.into(), |_, window, cx| {
        preferences::update(cx, |p| {
            p.appearance = Appearance::Dark;
            p.dark_theme = "Tokyo Night".into();
        });
        themes::apply(window, cx);
        assert!(Theme::global(cx).is_dark());
        assert_eq!(Theme::global(cx).theme_name().as_ref(), "Tokyo Night");
        assert_eq!(Theme::global(cx).font_size, px(14.));

        preferences::update(cx, |p| p.appearance = Appearance::Light);
        themes::apply(window, cx);
        assert!(!Theme::global(cx).is_dark());
        assert_eq!(Theme::global(cx).theme_name().as_ref(), "Catppuccin Latte");
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_unknown_theme_name_falls_back_to_the_kit_default(cx: &mut TestAppContext) {
    setup("unknown", cx);
    let window = cx.open_window(size(px(200.), px(200.)), |_, _| Blank);

    cx.update_window(window.into(), |_, window, cx| {
        preferences::update(cx, |p| {
            p.appearance = Appearance::Dark;
            p.dark_theme = "No Such Theme".into();
        });
        themes::apply(window, cx);
        let default = ThemeRegistry::global(cx).default_dark_theme().name.clone();
        assert_eq!(Theme::global(cx).theme_name(), &default);
    })
    .unwrap();
}
