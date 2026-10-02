//! Custom sources inside Settings: git repositories, often private, offered as
//! one-click shortcuts in the install dialog.

use super::{section, SettingsDialog};
use crate::preferences::{self, CustomSource};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::setting::SettingItem;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// The form for adding a custom source.
pub(super) struct SourceForm {
    source: Entity<InputState>,
    name: Entity<InputState>,
    /// Why the last attempt to add was refused.
    error: Option<String>,
}

impl SourceForm {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            source: cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or a git URL")),
            name: cx.new(|cx| InputState::new(window, cx).placeholder("Name (optional)")),
            error: None,
        }
    }
}

impl SettingsDialog {
    fn add_custom_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.source_form.source.read(cx).value().to_string();
        let name = self.source_form.name.read(cx).value().to_string();
        let mut result = Ok(());
        preferences::update(cx, |p| result = p.add_custom_source(&source, &name));

        self.source_form.error = result.err();
        if self.source_form.error.is_none() {
            for input in [&self.source_form.source, &self.source_form.name] {
                input.update(cx, |state, cx| state.set_value("", window, cx));
            }
        }
        cx.notify();
    }

    pub(super) fn custom_sources_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let view = cx.entity().downgrade();
        SettingItem::render(move |_, _, cx| {
            let Some(view) = view.upgrade() else {
                return div().into_any_element();
            };
            section(
                "Custom sources",
                "Git repositories, such as private ones, shown as shortcuts when \
                 installing. They are cloned with your own git credentials.",
                render_custom_sources(&view, cx),
                cx,
            )
            .into_any_element()
        })
    }
}

fn render_custom_sources(view: &Entity<SettingsDialog>, cx: &App) -> impl IntoElement {
    let sources = preferences::get(cx).custom_sources.clone();
    let form = &view.read(cx).source_form;
    let add = view.downgrade();

    v_flex()
        .gap_1()
        .when(sources.is_empty(), |this| {
            this.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No custom sources yet."),
            )
        })
        .children(
            sources
                .iter()
                .enumerate()
                .map(|(index, source)| render_custom_source(index, source, cx))
                .collect::<Vec<_>>(),
        )
        .child(
            h_flex()
                .pt_1()
                .gap_2()
                .child(Input::new(&form.source).id("new-source").small().flex_1())
                .child(
                    Input::new(&form.name)
                        .id("new-source-name")
                        .small()
                        .w(px(160.)),
                )
                .child(
                    Button::new("add-source")
                        .small()
                        .icon(Icon::empty().path("icons/plus.svg"))
                        .label("Add")
                        .on_click(move |_, window, cx| {
                            add.update(cx, |this, cx| this.add_custom_source(window, cx))
                                .ok();
                        }),
                ),
        )
        .when_some(form.error.clone(), |this, error| {
            this.child(
                div()
                    .id("source-error")
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(error)
                    .test_support(),
            )
        })
}

fn render_custom_source(index: usize, source: &CustomSource, cx: &App) -> impl IntoElement {
    let key = source.source.clone();
    h_flex()
        .gap_2()
        .items_center()
        .child(
            Icon::empty()
                .path("icons/folder-git-2.svg")
                .small()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().truncate().child(source.label().to_string()))
                .when(source.name.is_some(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(source.source.clone()),
                    )
                }),
        )
        .child(
            Button::new(("remove-source", index))
                .xsmall()
                .ghost()
                .icon(Icon::empty().path("icons/x.svg"))
                .tooltip("Remove")
                .on_click(move |_, _, cx| {
                    preferences::update(cx, |p| p.custom_sources.retain(|s| s.source != key))
                }),
        )
}
