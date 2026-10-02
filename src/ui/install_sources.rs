//! Custom sources in the install dialog: one click selects a source and lists
//! the skills in it.

use super::{empty_note, failure, loading, matches_query, result_heading, result_row};
use super::{InstallDialog, Lookup};
use crate::custom_sources::{self, SourceSkill};
use gpui_kit::component::button::Button;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// The custom source whose skills are listed.
pub(super) struct Browsed {
    index: usize,
    skills: Lookup<Vec<SourceSkill>>,
}

impl InstallDialog {
    /// Make custom source `index` the install source and list its skills.
    fn browse(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.custom_sources.get(index).map(|s| s.source.clone()) else {
            return;
        };
        // No skill names: the whole source installs unless one is picked.
        self.pick(source.clone(), String::new(), window, cx);
        self.browsed = Some(Browsed {
            index,
            skills: Lookup::Loading,
        });
        cx.notify();

        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { custom_sources::list_skills(&source) })
                .await;
            this.update(cx, |this, cx| {
                // The user may have moved on to another source meanwhile.
                let Some(browsed) = this.browsed.as_mut().filter(|b| b.index == index) else {
                    return;
                };
                browsed.skills = match found {
                    Ok(skills) => Lookup::Ready(skills),
                    Err(e) => Lookup::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The listed source, while it is still the one in the Source field.
    fn current_browsed(&self, cx: &App) -> Option<&Browsed> {
        let current = self.source_value(cx);
        self.browsed
            .as_ref()
            .filter(|b| self.custom_sources.get(b.index).map(|s| &s.source) == Some(&current))
    }

    pub(super) fn render_custom_sources(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.current_browsed(cx).map(|b| b.index);
        let chips = self
            .custom_sources
            .iter()
            .enumerate()
            .map(|(index, custom)| {
                Button::new(("custom-source", index))
                    .small()
                    .outline()
                    .icon(Icon::empty().path("icons/folder-git-2.svg"))
                    .label(custom.label().to_string())
                    .tooltip(custom.source.clone())
                    .selected(selected == Some(index))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.browse(index, window, cx)),
                    )
            })
            .collect::<Vec<_>>();

        v_flex()
            .gap_2()
            .child(result_heading("Your sources", cx))
            .child(h_flex().flex_wrap().gap_2().children(chips))
            .when_some(self.current_browsed(cx), |this, browsed| {
                this.child(self.render_browsed(browsed, cx))
            })
    }

    fn render_browsed(&self, browsed: &Browsed, cx: &mut Context<Self>) -> AnyElement {
        let skills = match &browsed.skills {
            Lookup::Idle => return div().into_any_element(),
            Lookup::Loading => return loading("Cloning repository…"),
            Lookup::Failed(e) => return failure(e, cx),
            Lookup::Ready(skills) => skills,
        };
        if skills.is_empty() {
            return empty_note("No skills in this repository.", cx).into_any_element();
        }

        let source = self.source_value(cx);
        let query = self.query.read(cx).value().to_string();
        let rows = skills
            .iter()
            .filter(|s| matches_query(&s.name, &s.description, &query))
            .enumerate()
            .map(|(index, skill)| {
                let (source, name) = (source.clone(), skill.name.clone());
                result_row(
                    SharedString::from(format!("source-skill-{index}")),
                    skill.name.clone(),
                    skill.folder.clone(),
                    None,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pick(source.clone(), name.clone(), window, cx)
                }))
                .test_support()
            })
            .collect::<Vec<_>>();

        v_flex()
            .gap_1()
            .child(empty_note(
                "Pick a skill, or leave Skills empty to install them all.",
                cx,
            ))
            .child(
                v_flex()
                    .id("source-skills")
                    .max_h(px(160.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}
