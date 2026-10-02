//! The skill lists under the usage chart: every used skill ranked, and the
//! loaded skills that went unused.

use super::chart;
use super::insights;
use super::stats::{card, muted_line};
use super::{Filter, UsageView};
use crate::usage::{Metric, Ranking};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// How many unused skills are named before the rest are counted.
const UNUSED_SHOWN: usize = 12;

impl UsageView {
    pub(super) fn render_ranking(
        &self,
        rankings: &[&Ranking],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let metric = self.filters.metric;
        let leader = rankings
            .first()
            .map_or(0, |ranking| ranking.counts.value(metric));
        let total = self.snapshot.counts.value(metric);

        card(cx)
            .child(
                h_flex()
                    .items_center()
                    .child(div().flex_1().font_semibold().child("Ranking"))
                    .child(muted_line("Click a skill to compare it in the chart", cx)),
            )
            .when(rankings.is_empty(), |this| {
                this.child(muted_line("No skill matches the search.", cx))
            })
            .children(
                rankings.iter().enumerate().map(|(index, ranking)| {
                    self.render_ranking_row(index, ranking, leader, total, cx)
                }),
            )
    }

    fn render_ranking_row(
        &self,
        index: usize,
        ranking: &Ranking,
        leader: u64,
        total: u64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let metric = self.filters.metric;
        let value = ranking.counts.value(metric);
        let slot = self.selection.slot(ranking.skill_id);
        let bar_color = slot.map_or(cx.theme().primary, |slot| chart::slot_color(slot, cx));
        let fraction = if leader == 0 {
            0.
        } else {
            value as f32 / leader as f32
        };
        let skill_id = ranking.skill_id;
        let name = ranking.name.clone();
        let context = self
            .ranking_contexts
            .get(&ranking.skill_id)
            .cloned()
            .unwrap_or_default();

        h_flex()
            .id(("usage-rank", ranking.skill_id as u64))
            .px_2()
            .py_1p5()
            .gap_3()
            .items_center()
            .rounded(cx.theme().radius)
            .hover(|this| this.bg(cx.theme().list_hover))
            .when(slot.is_some(), |this| this.bg(cx.theme().list_active))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_filter(Filter::Skill(skill_id, name.clone()), cx)
            }))
            .child(
                div()
                    .w(px(24.))
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{}", index + 1)),
            )
            .child(
                v_flex()
                    .w(px(220.))
                    .flex_shrink_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .children(slot.map(|slot| chart::swatch(slot, cx)))
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .truncate()
                                    .child(ranking.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(context),
                    ),
            )
            .child(
                div().flex_1().min_w_0().h(px(8.)).child(
                    div()
                        .h_full()
                        .w(relative(fraction))
                        .rounded(px(4.))
                        .bg(bar_color),
                ),
            )
            .child(
                div()
                    .w(px(56.))
                    .text_right()
                    .font_semibold()
                    .child(value.to_string()),
            )
            .child(
                div()
                    .w(px(44.))
                    .text_right()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    // Against a selection's total, other skills' shares would
                    // pass 100%.
                    .when(
                        metric == Metric::Activations && self.selection.is_empty(),
                        |this| this.child(format!("{}%", insights::share(value, total))),
                    ),
            )
            .test_support()
    }

    /// Loaded skills nobody used: candidates to disable, since every agent
    /// session still carries their descriptions.
    pub(super) fn render_unused(&self, cx: &App) -> Option<impl IntoElement> {
        if !self.selection.is_empty() || self.query_pending {
            return None;
        }
        let unused = insights::unused(&self.installed, &self.snapshot.rankings);
        if unused.is_empty() {
            return None;
        }
        let hidden = unused.len().saturating_sub(UNUSED_SHOWN);

        Some(
            card(cx)
                .id("usage-unused")
                .child(
                    div()
                        .font_semibold()
                        .child(format!("Not used in the last {} days", self.range.days())),
                )
                .child(muted_line(
                    "These skills' descriptions still sit in every agent session. \
                     Consider disabling the ones you no longer need.",
                    cx,
                ))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .children(
                            unused
                                .iter()
                                .take(UNUSED_SHOWN)
                                .map(|name| Tag::secondary().small().child(name.to_string())),
                        )
                        .when(hidden > 0, |this| {
                            this.child(muted_line(format!("and {hidden} more"), cx))
                        }),
                )
                .test_support(),
        )
    }
}
