//! The centre of the usage view: headline figures, the most used skill, the
//! daily chart and the ranked skills.

use super::chart;
use super::insights::{self, Day, Selection};
use super::labels;
use super::UsageView;
use crate::preferences;
use crate::usage::{Metric, Ranking, RoleFilter, ServiceState};
use chrono::Local;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// Shown in place of a figure the current data cannot give.
const NO_VALUE: &str = "—";

impl UsageView {
    pub(super) fn render_stats(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let metric = self.filters.metric;
        let today = Local::now().date_naive();
        let days = insights::days(
            self.range.days(),
            today,
            &self.snapshot.daily,
            &self.snapshot.daily_by_skill,
            &self.selection,
        );
        let top = insights::shown_rankings(&self.snapshot.rankings, &self.selection, metric)
            .first()
            .copied();
        // The ranking always lists every skill, so more can be picked from it.
        let rankings = self.searched(
            insights::shown_rankings(&self.snapshot.rankings, &Selection::default(), metric),
            cx,
        );
        let has_activity = self.snapshot.counts.activations > 0;

        v_flex()
            .id("usage-stats")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px_6()
            .py_5()
            .gap_5()
            .child(self.render_title(cx))
            .children(self.render_notice(cx))
            .child(self.render_tiles(&days, cx))
            .when(has_activity, |this| {
                this.children(top.map(|top| self.render_top_skill(top, cx)))
                    .child(self.render_daily(&days, cx))
                    .child(self.render_ranking(&rankings, cx))
            })
            .when(!has_activity, |this| this.child(self.render_empty(cx)))
            .children(self.render_unused(cx))
            .child(self.render_footnote(cx))
    }

    /// Rankings whose name matches the search box.
    fn searched<'a>(&self, rankings: Vec<&'a Ranking>, cx: &App) -> Vec<&'a Ranking> {
        let search = self.search_text(cx);
        rankings
            .into_iter()
            .filter(|ranking| search.is_empty() || ranking.name.to_lowercase().contains(&search))
            .collect()
    }

    fn render_title(&self, cx: &App) -> impl IntoElement {
        let mut context = vec![
            self.scope.label.to_string(),
            format!("Last {} days", self.range.days()),
        ];
        if self.scope.project.is_none() {
            context.insert(1, "All projects".into());
        }
        if let Some(agent) = &self.filters.agent {
            context.push(labels::agent_label(agent));
        }
        match self.filters.role {
            RoleFilter::All => {}
            RoleFilter::Main => context.push("Main agents only".into()),
            RoleFilter::Subagents => context.push("Subagents only".into()),
        }

        h_flex()
            .gap_3()
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_xl().font_semibold().child("Skill usage"))
                    .child(
                        div()
                            .id("usage-context")
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .aria_label(context.join(" · "))
                            .child(context.join(" · "))
                            .test_support(),
                    ),
            )
            .when(self.query_pending, |this| {
                this.child(Spinner::new().small())
            })
    }

    /// Why the figures may be missing or incomplete, with the way out.
    fn render_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(error) = self.status.error.clone() {
            return Some(notice(div().text_color(cx.theme().danger).child(error), cx));
        }
        if !preferences::get(cx).tracking_enabled {
            return Some(notice(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().flex_1().child(
                        "Tracking is off, so new agent activity is not imported. \
                         Skillshard reads local agent histories only; nothing is uploaded.",
                    ))
                    .child(
                        Button::new("usage-enable-tracking")
                            .small()
                            .primary()
                            .label("Turn on tracking")
                            .on_click(|_, _, cx| {
                                preferences::update(cx, |p| p.tracking_enabled = true)
                            }),
                    ),
                cx,
            ));
        }
        let importing = matches!(
            self.status.state,
            ServiceState::Starting | ServiceState::Rebuilding
        );
        importing.then(|| {
            notice(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Spinner::new().small())
                    .child(format!(
                        "{} · {} records imported so far",
                        labels::state_label(self.status.state),
                        self.status.imported_records
                    )),
                cx,
            )
        })
    }

    fn render_tiles(&self, days: &[Day], cx: &App) -> impl IntoElement {
        let counts = &self.snapshot.counts;
        let has_activity = counts.activations > 0;
        let figure = |value: u64| {
            if has_activity {
                value.to_string()
            } else {
                NO_VALUE.to_string()
            }
        };
        let used = insights::shown_rankings(
            &self.snapshot.rankings,
            &self.selection,
            Metric::Activations,
        )
        .len();
        let unused = insights::unused(&self.installed, &self.snapshot.rankings).len();
        let busiest = insights::busiest_day(days);
        let conversations = if counts.provisional_conversations {
            format!("{} conversations (provisional)", counts.conversations)
        } else {
            format!("{} conversations", counts.conversations)
        };

        h_flex()
            .gap_3()
            .child(tile(
                "usage-tile-activations",
                "Activations",
                figure(counts.activations),
                format!(
                    "{} main · {} subagent",
                    counts.main_activations, counts.subagent_activations
                ),
                cx,
            ))
            .child(tile(
                "usage-tile-sessions",
                "Sessions",
                figure(counts.sessions),
                conversations,
                cx,
            ))
            .child(tile(
                "usage-tile-skills",
                "Skills used",
                figure(used as u64),
                if self.installed.is_empty() || !self.selection.is_empty() {
                    format!("Last used {}", labels::timestamp_label(counts.last_used))
                } else {
                    format!("{unused} of {} installed unused", self.installed.len())
                },
                cx,
            ))
            .child(tile(
                "usage-tile-busiest",
                "Busiest day",
                busiest.map_or(NO_VALUE.into(), |day| labels::day_label(day.date)),
                busiest.map_or(String::new(), |day| {
                    labels::metric_count(self.filters.metric, day.total)
                }),
                cx,
            ))
    }

    fn render_top_skill(&self, top: &Ranking, cx: &App) -> impl IntoElement {
        let metric = self.filters.metric;
        let value = top.counts.value(metric);
        let selected = !self.selection.is_empty();
        let percent = insights::share(value, self.snapshot.counts.value(metric));
        let caption = if selected {
            "Most used of the selected skills"
        } else {
            "Most used skill"
        };
        let context = self
            .ranking_contexts
            .get(&top.skill_id)
            .cloned()
            .unwrap_or_default();

        h_flex()
            .id("usage-top-skill")
            .p_5()
            .gap_6()
            .items_center()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().primary.opacity(0.5))
            .bg(cx.theme().primary.opacity(0.08))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(caption_text(caption, cx))
                    .child(
                        div()
                            .id("usage-top-skill-name")
                            .text_2xl()
                            .font_semibold()
                            .truncate()
                            .aria_label(top.name.clone())
                            .child(top.name.clone())
                            .test_support(),
                    )
                    .child(muted_line(context, cx))
                    .child(muted_line(
                        format!(
                            "{} main · {} subagent · last used {}",
                            top.counts.main_activations,
                            top.counts.subagent_activations,
                            labels::timestamp_label(top.counts.last_used)
                        ),
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .items_end()
                    .gap_0p5()
                    .child(div().text_3xl().font_semibold().child(value.to_string()))
                    .child(muted_line(labels::metric_title(metric).to_lowercase(), cx))
                    .child(muted_line(
                        labels::share_label(metric, percent, selected),
                        cx,
                    )),
            )
            .test_support()
    }

    fn render_daily(&self, days: &[Day], cx: &App) -> impl IntoElement {
        let metric = self.filters.metric;
        let stacked = !self.selection.is_empty();
        let title = if stacked {
            format!(
                "Daily {} by skill",
                labels::metric_title(metric).to_lowercase()
            )
        } else {
            format!("Daily {}", labels::metric_title(metric).to_lowercase())
        };

        card(cx)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().flex_1().font_semibold().child(title))
                    .when(stacked, |this| {
                        this.child(chart::legend(&self.selection, cx))
                    }),
            )
            .child(chart::render(days, metric, &self.selection, cx))
            .when(stacked && metric != Metric::Activations, |this| {
                this.child(muted_line(
                    "Bars add up each skill's own count: one session using two selected \
                     skills appears in both.",
                    cx,
                ))
            })
    }

    fn render_empty(&self, cx: &App) -> impl IntoElement {
        let message = if self.query_pending {
            "Loading local usage…".to_string()
        } else {
            format!(
                "No skill activations recorded here in the last {} days. Usage appears \
                 once an agent loads one of your skills.",
                self.range.days()
            )
        };
        card(cx)
            .items_center()
            .py_10()
            .child(
                Icon::empty()
                    .path("icons/chart-line.svg")
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(div().text_color(cx.theme().muted_foreground).child(message))
    }

    fn render_footnote(&self, cx: &App) -> impl IntoElement {
        let snapshot = &self.snapshot;
        let excluded = snapshot.counts.unconfirmed + snapshot.unresolved;
        v_flex()
            .gap_1()
            .child(muted_line(
                "An activation means an agent loaded the skill's instructions, not that it \
                 followed them. Days follow your local calendar.",
                cx,
            ))
            .when(excluded > 0, |this| {
                this.child(muted_line(
                    format!(
                        "Not counted: {} unconfirmed detections and {} references to \
                         skills that could not be identified.",
                        snapshot.counts.unconfirmed, snapshot.unresolved
                    ),
                    cx,
                ))
            })
    }
}

pub(super) fn card(cx: &App) -> Div {
    v_flex()
        .p_4()
        .gap_3()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
}

fn notice(content: impl IntoElement, cx: &App) -> AnyElement {
    div()
        .px_4()
        .py_3()
        .rounded(cx.theme().radius_lg)
        .bg(cx.theme().muted)
        .text_sm()
        .child(content)
        .into_any_element()
}

fn tile(
    id: &'static str,
    label: &'static str,
    value: String,
    detail: String,
    cx: &App,
) -> impl IntoElement {
    card(cx)
        .id(id)
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(caption_text(label, cx))
        .child(div().text_2xl().font_semibold().truncate().child(value))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(detail),
        )
        .test_support()
}

fn caption_text(text: &str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_uppercase())
}

pub(super) fn muted_line(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
