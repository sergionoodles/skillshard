//! The project settings modal: pick an icon and colour for a project's
//! sidebar entry, or take the project out of the sidebar.

use crate::paths;
use crate::preferences::{self, Project};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

/// Icons offered for projects; the first is the default. The globe is left
/// out because it stands for the global scope.
pub const ICONS: &[&str] = &[
    "folder",
    "folder-code",
    "folder-git-2",
    "code",
    "code-xml",
    "terminal",
    "braces",
    "git-branch",
    "github",
    "bug",
    "rocket",
    "package",
    "box",
    "boxes",
    "layers",
    "blocks",
    "puzzle",
    "component",
    "server",
    "database",
    "cloud",
    "cpu",
    "hard-drive",
    "network",
    "laptop",
    "monitor",
    "smartphone",
    "bot",
    "brain",
    "brain-circuit",
    "sparkles",
    "wand-sparkles",
    "zap",
    "lightbulb",
    "target",
    "flask-conical",
    "atom",
    "dna",
    "microscope",
    "telescope",
    "book-open",
    "notebook-pen",
    "graduation-cap",
    "pen-tool",
    "palette",
    "brush",
    "camera",
    "image",
    "film",
    "music",
    "headphones",
    "gamepad-2",
    "dice-5",
    "trophy",
    "crown",
    "gem",
    "star",
    "heart",
    "flame",
    "leaf",
    "sprout",
    "trees",
    "flower-2",
    "mountain",
    "sun",
    "moon",
    "feather",
    "compass",
    "map",
    "plane",
    "car",
    "bike",
    "anchor",
    "briefcase",
    "building-2",
    "chart-line",
    "wallet",
    "shopping-cart",
    "store",
    "house",
    "shield",
    "lock",
    "key",
    "wrench",
    "hammer",
    "cog",
    "calendar",
    "mail",
    "message-square",
    "coffee",
    "pizza",
    "cat",
    "dog",
    "bird",
    "fish",
    "rabbit",
    "ghost",
];

/// Palette colours offered for projects, by GPUI Kit colour name.
pub const COLORS: &[&str] = &[
    "red", "orange", "amber", "green", "emerald", "teal", "sky", "blue", "indigo", "violet",
    "pink", "rose",
];

/// The icon for a project, falling back to a folder for unknown names (a
/// hand-edited settings file) so the sidebar never shows a blank.
pub fn icon(project: Option<&Project>) -> Icon {
    let name = project
        .and_then(|p| p.icon.as_deref())
        .filter(|name| ICONS.contains(name))
        .unwrap_or(ICONS[0]);
    Icon::empty().path(format!("icons/{name}.svg"))
}

/// The resolved colour for a project, if it has one.
///
/// A lighter shade on dark themes and a darker one on light themes keeps the
/// same named colour legible against either background.
pub fn color(project: Option<&Project>, cx: &App) -> Option<Hsla> {
    let name = project?.color.as_deref()?;
    let color = ColorName::try_from(name).ok()?;
    Some(color.scale(if cx.theme().is_dark() { 400 } else { 600 }))
}

pub enum ProjectSettingsEvent {
    Close,
    /// Take the project out of the sidebar. Nothing on disk is touched.
    Remove(PathBuf),
}

impl EventEmitter<ProjectSettingsEvent> for ProjectSettingsDialog {}

pub struct ProjectSettingsDialog {
    path: PathBuf,
}

impl ProjectSettingsDialog {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        // Choices are written straight to preferences; redraw when they land.
        cx.observe_global::<preferences::Store>(|_, cx| cx.notify())
            .detach();
        Self { path }
    }

    fn set(&self, cx: &mut App, change: impl FnOnce(&mut Project)) {
        let path = self.path.clone();
        preferences::update(cx, |p| change(p.project_mut(&path)));
    }
}

impl Render for ProjectSettingsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project = preferences::get(cx).project(&self.path).cloned();
        let current_icon = project
            .as_ref()
            .and_then(|p| p.icon.clone())
            .unwrap_or_else(|| ICONS[0].to_string());
        let current_color = project.as_ref().and_then(|p| p.color.clone());
        let tint = color(project.as_ref(), cx).unwrap_or(cx.theme().foreground);
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| paths::shorten(&self.path));

        let icon_tiles = ICONS
            .iter()
            .map(|icon_name| {
                let selected = current_icon == *icon_name;
                let picked = icon_name.to_string();
                div()
                    .id(SharedString::from(format!("icon-{icon_name}")))
                    .size(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(if selected {
                        cx.theme().ring
                    } else {
                        cx.theme().transparent
                    })
                    .when(selected, |this| this.bg(cx.theme().accent))
                    .hover(|this| this.bg(cx.theme().list_hover))
                    .child(
                        Icon::empty()
                            .path(format!("icons/{icon_name}.svg"))
                            .with_size(px(20.))
                            .text_color(tint),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let picked = picked.clone();
                        this.set(cx, |p| p.icon = Some(picked))
                    }))
                    .test_support()
            })
            .collect::<Vec<_>>();

        let swatch = |id: String, fill: Option<Hsla>, selected: bool, cx: &App| {
            div()
                .id(SharedString::from(id))
                .size(px(22.))
                .rounded_full()
                .border_2()
                .border_color(if selected {
                    cx.theme().foreground
                } else {
                    cx.theme().transparent
                })
                .p(px(2.))
                .child(div().size_full().rounded_full().map(|this| match fill {
                    Some(fill) => this.bg(fill),
                    // "Default" reads as an empty ring.
                    None => this.border_1().border_color(cx.theme().muted_foreground),
                }))
        };
        let is_dark = cx.theme().is_dark();
        let mut color_swatches =
            vec![
                swatch("color-default".into(), None, current_color.is_none(), cx)
                    .on_click(cx.listener(|this, _, _, cx| this.set(cx, |p| p.color = None)))
                    .test_support()
                    .into_any_element(),
            ];
        for color_name in COLORS {
            let fill = ColorName::try_from(*color_name)
                .ok()
                .map(|c| c.scale(if is_dark { 400 } else { 600 }));
            let picked = color_name.to_string();
            color_swatches.push(
                swatch(
                    format!("color-{color_name}"),
                    fill,
                    current_color.as_deref() == Some(*color_name),
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let picked = picked.clone();
                    this.set(cx, |p| p.color = Some(picked))
                }))
                .test_support()
                .into_any_element(),
            );
        }

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.45))
            .child(
                v_flex()
                    .id("project-settings-dialog")
                    .w(px(520.))
                    .p_5()
                    .gap_4()
                    .rounded(cx.theme().radius_lg)
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(icon(project.as_ref()).large().text_color(tint))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().font_semibold().truncate().child(name))
                                    .child(
                                        div()
                                            .text_xs()
                                            .truncate()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(paths::shorten(&self.path)),
                                    ),
                            )
                            .child(
                                Button::new("close-project-settings")
                                    .small()
                                    .ghost()
                                    .icon(Icon::empty().path("icons/x.svg"))
                                    .tooltip("Close")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(ProjectSettingsEvent::Close)
                                    })),
                            ),
                    )
                    .child(field_label("Icon", cx))
                    .child(
                        div()
                            .id("icon-grid")
                            // The full set is taller than a small window;
                            // scroll it rather than push the buttons off.
                            .max_h(px(280.))
                            .overflow_y_scroll()
                            .child(h_flex().flex_wrap().gap_1().children(icon_tiles)),
                    )
                    .child(field_label("Colour", cx))
                    .child(h_flex().flex_wrap().gap_1().children(color_swatches))
                    .child(div().h_px().bg(cx.theme().border))
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                Button::new("remove-project")
                                    .small()
                                    .danger()
                                    .label("Remove project")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.emit(ProjectSettingsEvent::Remove(this.path.clone()))
                                    })),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Removes it from skillshard only; nothing on disk is deleted."),
                            ),
                    ),
            )
    }
}

fn field_label(text: &str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_uppercase())
}

#[cfg(test)]
mod tests {
    // Not `super::*`: see the note in `ui::install`'s tests.
    use super::{COLORS, ICONS};
    use gpui_kit::component::ColorName;
    use gpui_kit::AssetSource;

    #[test]
    fn every_offered_icon_is_in_the_bundled_set() {
        for name in ICONS {
            let path = format!("icons/{name}.svg");
            let loaded = crate::assets::Assets.load(&path).unwrap();
            assert!(loaded.is_some(), "{path} is not bundled");
        }
    }

    #[test]
    fn every_offered_colour_is_a_palette_name() {
        for name in COLORS {
            assert!(
                ColorName::try_from(*name).is_ok(),
                "{name} is not a palette colour"
            );
        }
    }
}
