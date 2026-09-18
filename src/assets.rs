//! Asset source: the app's own brand files, then GPUI Kit's icons.
//!
//! The kit's default `Assets` bundle holds only the icons its own components
//! use; `AllAssets` is the full Lucide catalogue (~730 KB), which is what lets
//! the app use any icon by path.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

/// The logo is two single-colour halves, because GPUI tints an SVG with one
/// colour; stacking them gives the two-tone mark.
pub const LOGO_LEFT: &str = "brand/shard-left.svg";
pub const LOGO_RIGHT: &str = "brand/shard-right.svg";

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let own: Option<&'static [u8]> = match path {
            LOGO_LEFT => Some(include_bytes!("../assets/brand/shard-left.svg")),
            LOGO_RIGHT => Some(include_bytes!("../assets/brand/shard-right.svg")),
            _ => None,
        };
        match own {
            Some(bytes) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit::assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::AllAssets.list(path)
    }
}

#[cfg(test)]
mod tests {
    use super::Assets;
    use gpui_kit::AssetSource;
    use std::path::Path;

    /// Every `icons/….svg` path written in the source.
    fn referenced_icons(dir: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                referenced_icons(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (start, _) in text.match_indices("\"icons/") {
                    let rest = &text[start + 1..];
                    if let Some(end) = rest.find(".svg\"") {
                        let icon = &rest[..end + 4];
                        // `format!` templates are covered by the project
                        // icon test, which checks each name it can produce.
                        if !icon.contains('{') {
                            found.push(icon.to_string());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_icon_path_used_in_the_source_is_served() {
        // Regression: icons outside the kit's default bundle loaded as
        // nothing and rendered blank, which headless tests cannot see.
        let mut icons = Vec::new();
        referenced_icons(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut icons,
        );
        assert!(
            icons.len() >= 5,
            "expected to find icon paths, got {icons:?}"
        );
        for icon in icons {
            let loaded = Assets.load(&icon).unwrap_or(None);
            assert!(loaded.is_some(), "{icon} is not served");
        }
    }
}
