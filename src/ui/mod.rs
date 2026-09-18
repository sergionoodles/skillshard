//! The desktop interface.

pub mod app;
pub mod install;
pub mod project_settings;
pub mod settings;

pub use app::Skillshard;

use gpui_kit::App;
use std::path::PathBuf;

/// Set up everything the window depends on: the component library, the
/// bundled themes, and preferences loaded from `preferences_path`.
pub fn init(preferences_path: PathBuf, cx: &mut App) {
    gpui_kit::init(cx);
    // The theme files are compiled in and covered by tests, so a failure here
    // is a build defect rather than something a user could fix.
    crate::themes::register(cx).expect("bundled themes are valid");
    crate::preferences::init(preferences_path, cx);
}
