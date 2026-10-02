//! The desktop interface.

pub mod app;
pub mod diff;
pub mod install;
pub mod project_settings;
pub mod settings;
pub mod usage;

pub use app::Skillshard;

use gpui_kit::{App, Global};
use std::path::PathBuf;

/// Set up everything the window depends on: the component library, the
/// bundled themes, and preferences loaded from `preferences_path`.
pub fn init(preferences_path: PathBuf, cx: &mut App) {
    init_with_usage(
        preferences_path,
        crate::paths::usage_data_dir(),
        crate::usage::ingest::HistoryRoot::defaults(),
        cx,
    );
}

pub struct UsageEnvironment {
    pub directory: Result<PathBuf, String>,
    pub roots: Vec<crate::usage::ingest::HistoryRoot>,
}

impl Global for UsageEnvironment {}

/// Initialize with explicit local sources, also used by isolated headless tests.
pub fn init_with_usage(
    preferences_path: PathBuf,
    directory: Result<PathBuf, String>,
    roots: Vec<crate::usage::ingest::HistoryRoot>,
    cx: &mut App,
) {
    gpui_kit::init(cx);
    // The theme files are compiled in and covered by tests, so a failure here
    // is a build defect rather than something a user could fix.
    crate::themes::register(cx).expect("bundled themes are valid");
    crate::preferences::init(preferences_path, cx);
    cx.set_global(UsageEnvironment { directory, roots });
}
