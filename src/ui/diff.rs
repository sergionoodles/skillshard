//! Read-only preview of an available skill update.

use crate::model::LockEntry;
use crate::updates::{self, DiffLine, FileDiff};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tree::{tree, TreeItem, TreeState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;

pub enum DiffEvent {
    Close,
    Update,
}

impl EventEmitter<DiffEvent> for DiffDialog {}

pub struct DiffDialog {
    name: String,
    files: Option<Result<Vec<FileDiff>, String>>,
    tree: Entity<TreeState>,
}

impl DiffDialog {
    pub fn new(
        name: String,
        entry: LockEntry,
        installed: PathBuf,
        cache: Option<Arc<updates::TreeCache>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        cx.observe(&tree, |_, _, cx| cx.notify()).detach();
        let tree_for_load = tree.clone();
        cx.spawn(async move |this, cx| {
            let result =
                cx.background_executor()
                    .spawn(async move {
                        updates::preview_with_cache(&entry, &installed, cache.as_deref())
                    })
                    .await;
            this.update(cx, |this, cx| {
                if let Ok(files) = &result {
                    tree_for_load.update(cx, |tree, cx| {
                        tree.set_items(file_tree(files), cx);
                        if let Some(first) = files.first() {
                            tree.set_selected_item(
                                Some(&TreeItem::new(&first.path, &first.path)),
                                cx,
                            );
                        }
                    });
                }
                this.files = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            name,
            files: None,
            tree,
        }
    }

    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("diff-files")
            .w(px(230.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(tree(&self.tree, |_, entry, _, _, _| {
                let marker = if entry.is_folder() {
                    if entry.is_expanded() {
                        "▾"
                    } else {
                        "▸"
                    }
                } else {
                    " "
                };
                ListItem::new(format!("diff-file-{}", entry.item().id))
                    .w_full()
                    .justify_start()
                    .text_xs()
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_1()
                            .pl(px(entry.depth() as f32 * 14. + 8.))
                            .child(div().w(px(12.)).flex_shrink_0().child(marker))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(entry.item().label.clone()),
                            ),
                    )
            }))
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
                    .font_family(cx.theme().mono_font_family.clone())
                    .when(file.is_binary, |this| this.child("Binary file changed"))
                    .when(!file.is_binary && file.lines.is_empty(), |this| {
                        this.child("Empty file changed")
                    })
                    .children(rows),
            )
    }
}

fn file_tree(files: &[FileDiff]) -> Vec<TreeItem> {
    let mut roots = Vec::new();
    for file in files {
        let mut parent = &mut roots;
        let mut path = String::new();
        let parts: Vec<&str> = file.path.split('/').collect();
        for (index, part) in parts.iter().enumerate() {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            let position = parent
                .iter()
                .position(|item: &TreeItem| item.id.as_ref() == path)
                .unwrap_or_else(|| {
                    parent.push(TreeItem::new(path.clone(), *part).expanded(true));
                    parent.len() - 1
                });
            if index + 1 < parts.len() {
                parent = &mut parent[position].children;
            }
        }
    }
    roots
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
                            .flex_1()
                            .p_4()
                            .child(format!("Could not load changes: {error}"))
                            .into_any_element(),
                        Some(Ok(files)) if files.is_empty() => div()
                            .flex_1()
                            .p_4()
                            .child("No file changes found in the installed copy.")
                            .into_any_element(),
                        Some(Ok(files)) => h_flex()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_files(cx))
                            .child(
                                self.render_diff(
                                    self.tree
                                        .read(cx)
                                        .selected_item()
                                        .and_then(|item| {
                                            files.iter().find(|file| {
                                                file.path == item.id.as_ref()
                                                    || (item.is_folder()
                                                        && file
                                                            .path
                                                            .starts_with(&format!("{}/", item.id)))
                                            })
                                        })
                                        .unwrap_or(&files[0]),
                                    cx,
                                ),
                            )
                            .into_any_element(),
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .px_4()
                            .py_3()
                            .justify_end()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                Button::new("confirm-update")
                                    .primary()
                                    .label("Update")
                                    .disabled(!matches!(self.files, Some(Ok(_))))
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(DiffEvent::Update)),
                                    ),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{file_tree, DiffDialog, DiffEvent, FileDiff, TreeState};
    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{px, size, AppContext, TestAppContext};
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn nested_changes_form_an_expanded_file_tree() {
        let files = ["SKILL.md", "scripts/build.rs", "scripts/test.rs"]
            .into_iter()
            .map(|path| FileDiff {
                path: path.into(),
                lines: Vec::new(),
                is_binary: false,
            })
            .collect::<Vec<_>>();
        let roots = file_tree(&files);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].label.as_ref(), "SKILL.md");
        assert_eq!(roots[1].label.as_ref(), "scripts");
        assert!(roots[1].is_expanded());
        assert_eq!(
            roots[1]
                .children
                .iter()
                .map(|child| child.label.as_ref())
                .collect::<Vec<&str>>(),
            vec!["build.rs", "test.rs"]
        );
    }

    #[gpui_kit::test]
    fn loaded_tree_selects_files_and_footer_emits_update(cx: &mut TestAppContext) {
        let preferences =
            std::env::temp_dir().join(format!("skillshard-diff-test-{}.json", std::process::id()));
        cx.update(|cx| crate::ui::init(preferences, cx));
        let files = ["SKILL.md", "scripts/build.rs"]
            .into_iter()
            .map(|path| FileDiff {
                path: path.into(),
                lines: Vec::new(),
                is_binary: false,
            })
            .collect::<Vec<_>>();
        let mut view = None;
        let handle = cx.open_window(size(px(900.), px(700.)), |window, cx| {
            let tree = cx.new(|cx| TreeState::new(cx).items(file_tree(&files)));
            let dialog = cx.new(|cx| {
                cx.observe(&tree, |_, _, cx| cx.notify()).detach();
                DiffDialog {
                    name: "example".into(),
                    files: Some(Ok(files)),
                    tree,
                }
            });
            view = Some(dialog.clone());
            Root::new(dialog, window, cx)
        });
        let view = view.unwrap();
        let updates = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let updates = updates.clone();
            cx.subscribe(&view, move |_, event, _| {
                if matches!(event, DiffEvent::Update) {
                    updates.set(updates.get() + 1);
                }
            })
            .detach();
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("diff-file-scripts").is_some());
            assert!(window.try_find("diff-file-scripts/build.rs").is_some());
            window.click("diff-file-scripts/build.rs", cx);
            window.render_frame(cx);
            window.click("confirm-update", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let selected = cx.update(|cx| {
            view.read(cx)
                .tree
                .read(cx)
                .selected_item()
                .map(|item| item.id.to_string())
        });
        assert_eq!(selected.as_deref(), Some("scripts/build.rs"));
        assert_eq!(updates.get(), 1);
    }
}
