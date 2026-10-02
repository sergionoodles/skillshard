//! The right pane of the usage view: period, measure, worker and agent
//! filters, the skill picker for comparisons, and where the data comes from.

use super::chart;
use super::insights::MAX_SELECTED;
use super::labels;
use super::{Filter, UsageEvent, UsageView};
use crate::preferences::TrackingDays;
use crate::ui::app::{section_label, SIDE_PANE_WIDTH};
use crate::usage::{Metric, RoleFilter};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::separator::Separator;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const ALL_AGENTS: &str = "All agent apps";

const MEASURES: [(&str, Metric, &str); 3] = [
    (
        "usage-metric-activations",
        Metric::Activations,
        "Every time an agent loaded a skill",
    ),
    (
        "usage-metric-sessions",
        Metric::Sessions,
        "Agent sessions that loaded a skill",
    ),
    (
        "usage-metric-conversations",
        Metric::Conversations,
        "Conversations, with their subagents folded in",
    ),
];

const WORKERS: [(&str, &str, RoleFilter); 3] = [
    ("usage-role-all", "All", RoleFilter::All),
    ("usage-role-main", "Main", RoleFilter::Main),
    ("usage-role-subagents", "Subagents", RoleFilter::Subagents),
];

impl UsageView {
    pub(super) fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("usage-filters")
            .w(px(SIDE_PANE_WIDTH))
            .flex_shrink_0()
            .h_full()
            .p_4()
            .gap_4()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(self.render_period(cx))
            .child(self.render_measure(cx))
            .child(self.render_workers(cx))
            .child(self.render_agent(cx))
            .child(Separator::horizontal())
            .child(self.render_skill_picker(cx))
            .child(self.render_data_status(cx))
    }

    fn render_period(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .child(section_label("Period", cx))
            .child(h_flex().gap_1().children(TrackingDays::ALL.map(|days| {
                let beyond_history = days.days() > self.tracking_days.days();
                Button::new(("usage-range", days.days() as usize))
                    .small()
                    .flex_1()
                    .label(format!("{}d", days.days()))
                    .selected(self.range == days)
                    .disabled(beyond_history)
                    .when(beyond_history, |this| {
                        this.tooltip("Keep more history under Settings › Tracking")
                    })
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_filter(Filter::Range(days), cx)),
                    )
            })))
    }

    fn render_measure(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_0p5()
            .child(section_label("Measure", cx))
            .children(MEASURES.map(|(id, metric, description)| {
                let selected = self.filters.metric == metric;
                v_flex()
                    .id(id)
                    .px_2()
                    .py_1p5()
                    .rounded(cx.theme().radius)
                    .hover(|this| this.bg(cx.theme().list_hover))
                    .when(selected, |this| this.bg(cx.theme().list_active))
                    .child(
                        div()
                            .text_sm()
                            .when(selected, |this| this.font_semibold())
                            .child(labels::metric_title(metric)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(description),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_filter(Filter::Metric(metric), cx)
                    }))
                    .test_support()
            }))
    }

    fn render_workers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .child(section_label("Workers", cx))
            .child(h_flex().gap_1().children(WORKERS.map(|(id, label, role)| {
                Button::new(id)
                    .small()
                    .flex_1()
                    .label(label)
                    .selected(self.filters.role == role)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_filter(Filter::Role(role), cx)),
                    )
            })))
    }

    fn render_agent(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let choices: Vec<(String, Option<String>)> = std::iter::once((ALL_AGENTS.into(), None))
            .chain(
                self.snapshot
                    .agents
                    .iter()
                    .map(|agent| (labels::agent_label(agent), Some(agent.clone()))),
            )
            .collect();
        let current = self
            .filters
            .agent
            .as_deref()
            .map_or(ALL_AGENTS.into(), labels::agent_label);

        v_flex().child(section_label("Agent app", cx)).child(
            Button::new("usage-agent")
                .small()
                .w_full()
                .label(current)
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for (label, agent) in &choices {
                        let view = view.clone();
                        let agent = agent.clone();
                        menu = menu.item(PopupMenuItem::new(label.clone()).on_click(
                            move |_, _, cx| {
                                if let Some(view) = view.upgrade() {
                                    view.update(cx, |this, cx| {
                                        this.set_filter(Filter::Agent(agent.clone()), cx)
                                    });
                                }
                            },
                        ));
                    }
                    menu.scrollable(true)
                }),
        )
    }

    /// Checkboxes for picking up to [`MAX_SELECTED`] skills to compare; with
    /// none picked, every skill counts.
    fn render_skill_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let search = self.search_text(cx);
        let metric = self.filters.metric;
        // Picked skills stay listed even if the current filters hide their use.
        let picked = self.selection.skills().iter().map(|skill| {
            let value = self
                .snapshot
                .rankings
                .iter()
                .find(|ranking| ranking.skill_id == skill.skill_id)
                .map_or(0, |ranking| ranking.counts.value(metric));
            (skill.skill_id, skill.name.clone(), value)
        });
        let others = self
            .snapshot
            .rankings
            .iter()
            .filter(|ranking| self.selection.slot(ranking.skill_id).is_none())
            .filter(|ranking| ranking.counts.value(metric) > 0)
            .map(|ranking| {
                (
                    ranking.skill_id,
                    ranking.name.clone(),
                    ranking.counts.value(metric),
                )
            });
        let choices: Vec<(i64, String, u64)> = picked
            .chain(others)
            .filter(|(_, name, _)| search.is_empty() || name.to_lowercase().contains(&search))
            .collect();

        v_flex()
            .flex_1()
            .min_h_0()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .child(div().flex_1().child(section_label("Compare skills", cx)))
                    .when(!self.selection.is_empty(), |this| {
                        this.child(
                            Button::new("usage-clear-skills")
                                .xsmall()
                                .ghost()
                                .label("Show all")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_filter(Filter::ClearSkills, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Pick up to {MAX_SELECTED} skills to compare them day by day."
                    )),
            )
            .child(
                v_flex()
                    .id("usage-skill-picker")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_0p5()
                    .when(choices.is_empty(), |this| {
                        this.child(
                            div()
                                .px_1()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No used skills to pick."),
                        )
                    })
                    .children(choices.into_iter().map(|(skill_id, name, value)| {
                        self.render_skill_choice(skill_id, name, value, cx)
                    })),
            )
    }

    fn render_skill_choice(
        &self,
        skill_id: i64,
        name: String,
        value: u64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let slot = self.selection.slot(skill_id);
        let blocked = slot.is_none() && self.selection.is_full();

        h_flex()
            .px_1()
            .py_0p5()
            .gap_2()
            .items_center()
            .child(
                Checkbox::new(("usage-skill", skill_id as u64))
                    .small()
                    .flex_1()
                    .min_w_0()
                    .label(name.clone())
                    .checked(slot.is_some())
                    .disabled(blocked)
                    .on_click(cx.listener(move |this, _: &bool, _, cx| {
                        this.set_filter(Filter::Skill(skill_id, name.clone()), cx)
                    })),
            )
            .children(slot.map(|slot| chart::swatch(slot, cx)))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(value.to_string()),
            )
    }

    fn render_data_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let coverage = self
            .status
            .coverage
            .iter()
            .map(|coverage| {
                format!(
                    "{}: {}",
                    labels::agent_label(&coverage.agent),
                    coverage.status
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        v_flex()
            .gap_1()
            .pt_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("usage-status")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(format!(
                                "{} · {} records",
                                labels::state_label(self.status.state),
                                self.status.imported_records
                            ))
                            .when(!coverage.is_empty(), |this| {
                                this.tooltip(move |window, cx| {
                                    Tooltip::new(coverage.clone()).build(window, cx)
                                })
                            })
                            .test_support(),
                    )
                    .child(
                        Button::new("usage-open-settings")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Settings)
                            .tooltip("Tracking settings")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(UsageEvent::OpenSettings))),
                    ),
            )
    }
}
