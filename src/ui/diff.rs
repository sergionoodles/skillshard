//! Read-only preview of an available skill update.

use crate::model::LockEntry;
use crate::updates::{self, DiffLine, FileDiff};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

pub enum DiffEvent {
    Close,
}

impl EventEmitter<DiffEvent> for DiffDialog {}

pub struct DiffDialog {
    name: String,
    files: Option<Result<Vec<FileDiff>, String>>,
    selected: usize,
}

impl DiffDialog {
    pub fn new(name: String, entry: LockEntry, installed: PathBuf, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updates::preview(&entry, &installed) })
                .await;
            this.update(cx, |this, cx| {
                this.files = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            name,
            files: None,
            selected: 0,
        }
    }

    fn render_files(&self, files: &[FileDiff], cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("diff-files")
            .w(px(230.))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(cx.theme().border)
            .children(
                files
                    .iter()
                    .enumerate()
                    .map(|(index, file)| {
                        let selected = index == self.selected;
                        Button::new(format!("diff-file-{index}"))
                            .ghost()
                            .selected(selected)
                            .w_full()
                            .label(file.path.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = index;
                                cx.notify();
                            }))
                    })
                    .collect::<Vec<_>>(),
            )
    }

    fn render_diff(&self, file: &FileDiff, cx: &App) -> impl IntoElement {
        let mut old_number = 0;
        let mut new_number = 0;
        let rows = file
            .lines
            .iter()
            .map(|line| {
                let (marker, number, color, background) = match line {
                    DiffLine::Context(_) => {
                        old_number += 1;
                        new_number += 1;
                        (
                            " ",
                            format!("{old_number:>4} {new_number:>4}"),
                            cx.theme().muted_foreground,
                            cx.theme().transparent,
                        )
                    }
                    DiffLine::Added(_) => {
                        new_number += 1;
                        (
                            "+",
                            format!("     {new_number:>4}"),
                            cx.theme().success,
                            cx.theme().success.opacity(0.1),
                        )
                    }
                    DiffLine::Removed(_) => {
                        old_number += 1;
                        (
                            "−",
                            format!("{old_number:>4}     "),
                            cx.theme().danger,
                            cx.theme().danger.opacity(0.1),
                        )
                    }
                };
                let content = match line {
                    DiffLine::Context(text) | DiffLine::Added(text) | DiffLine::Removed(text) => {
                        text.clone()
                    }
                };
                h_flex()
                    .w_full()
                    .min_w_0()
                    .text_sm()
                    .text_color(color)
                    .bg(background)
                    .child(
                        div()
                            .w(px(90.))
                            .flex_shrink_0()
                            .child(format!("{number} {marker}")),
                    )
                    .child(div().min_w_0().child(content))
            })
            .collect::<Vec<_>>();

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .w_full()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .font_medium()
                    .child(file.path.clone()),
            )
            .child(
                v_flex()
                    .id("diff-lines")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .font_family("monospace")
                    .when(file.is_binary, |this| this.child("Binary file changed"))
                    .when(!file.is_binary && file.lines.is_empty(), |this| {
                        this.child("Empty file changed")
                    })
                    .children(rows),
            )
    }
}

impl Render for DiffDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = (f32::from(window.viewport_size().width) - 32.).min(1100.);
        let height = (f32::from(window.viewport_size().height) - 32.).min(720.);
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
                    .id("diff-dialog")
                    .w(px(width))
                    .h(px(height))
                    .overflow_hidden()
                    .rounded(cx.theme().radius_lg)
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .px_4()
                            .py_3()
                            .items_center()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .child(
                                        div()
                                            .font_semibold()
                                            .child(format!("Changes in {}", self.name)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Installed files → available version"),
                                    ),
                            )
                            .child(
                                Button::new("close-diff")
                                    .small()
                                    .ghost()
                                    .label("Close")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DiffEvent::Close))),
                            ),
                    )
                    .child(match &self.files {
                        None => h_flex()
                            .flex_1()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .child(Spinner::new().small())
                            .child("Loading changes…")
                            .into_any_element(),
                        Some(Err(error)) => div()
                            .p_4()
                            .child(format!("Could not load changes: {error}"))
                            .into_any_element(),
                        Some(Ok(files)) if files.is_empty() => div()
                            .p_4()
                            .child("No file changes found in the installed copy.")
                            .into_any_element(),
                        Some(Ok(files)) => h_flex()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_files(files, cx))
                            .child(self.render_diff(&files[self.selected], cx))
                            .into_any_element(),
                    }),
            )
    }
}
