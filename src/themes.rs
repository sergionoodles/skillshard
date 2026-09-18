//! Colour themes.
//!
//! The JSON files in `assets/themes` are vendored unmodified from GPUI Kit's
//! theme collection (<https://github.com/longbridge/gpui-kit/tree/main/themes>,
//! Apache-2.0). They are embedded so the app needs nothing on disk.

use crate::preferences::{self, Appearance};
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{px, App, Window};

const THEME_FILES: &[&str] = &[
    include_str!("../assets/themes/ayu.json"),
    include_str!("../assets/themes/catppuccin.json"),
    include_str!("../assets/themes/gruvbox.json"),
    include_str!("../assets/themes/solarized.json"),
    include_str!("../assets/themes/tokyonight.json"),
];

/// Themes offered for dark mode.
pub const DARK: &[&str] = &[
    "Catppuccin Mocha",
    "Tokyo Night",
    "Gruvbox Dark",
    "Ayu Dark",
];

/// Themes offered for light mode.
pub const LIGHT: &[&str] = &[
    "Catppuccin Latte",
    "Gruvbox Light",
    "Ayu Light",
    "Solarized Light",
];

/// Theme files set no font size, and the kit's 16px default reads large in a
/// dense list, so the app sets its own after every theme change.
const BASE_FONT_SIZE: f32 = 14.;

/// Add the bundled themes to the registry. Call once, after `gpui_kit::init`.
pub fn register(cx: &mut App) -> Result<(), String> {
    let registry = ThemeRegistry::global_mut(cx);
    for file in THEME_FILES {
        registry
            .load_themes_from_str(file)
            .map_err(|e| format!("bundled theme failed to load: {e}"))?;
    }
    Ok(())
}

/// Apply the themes and appearance from the current preferences.
///
/// With [`Appearance::System`] the window's own appearance picks between the
/// light and the dark theme.
pub fn apply(window: &mut Window, cx: &mut App) {
    let prefs = preferences::get(cx).clone();
    let registry = ThemeRegistry::global(cx);
    // A name that no longer exists (a hand-edited file, a renamed theme)
    // falls back to the kit default rather than leaving the old theme on.
    let light = registry
        .themes()
        .get(prefs.light_theme.as_str())
        .unwrap_or(registry.default_light_theme())
        .clone();
    let dark = registry
        .themes()
        .get(prefs.dark_theme.as_str())
        .unwrap_or(registry.default_dark_theme())
        .clone();

    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;

    let mode = match prefs.appearance {
        Appearance::System => ThemeMode::from(window.appearance()),
        Appearance::Light => ThemeMode::Light,
        Appearance::Dark => ThemeMode::Dark,
    };
    Theme::change(mode, Some(window), cx);
    Theme::global_mut(cx).font_size = px(BASE_FONT_SIZE);
    Theme::sync_base(cx);
}
