//! Snapshot publication and native usage presentation.

use super::Skillshard;
use crate::model::{Scope, Skill};
use crate::preferences;
use crate::ui::app::MainView;
use crate::ui::usage::{UsageEvent, UsageScope, UsageView};
use crate::usage::{Query, ServiceState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

impl Skillshard {
    pub(in crate::ui::app) fn publish_usage_view(&mut self, cx: &mut Context<Self>) {
        if self.usage.changing {
            return;
        }
        const EXPIRY_INTERVAL_SECONDS: u8 = 30;
        self.usage.expiry_ticks += 1;
        if self.usage.expiry_ticks >= EXPIRY_INTERVAL_SECONDS {
            self.usage.expiry_ticks = 0;
            self.expire_disabled_usage(cx);
        }
        let view = self.usage.current_view();
        if self.usage.seen_revision == view.revision {
            return;
        }
        self.usage.seen_revision = view.revision;
        if let Some(panel) = &self.usage.panel {
            let days = preferences::get(cx).tracking_days;
            panel.update(cx, |panel, cx| {
                panel.update_tracking_days(days, cx);
                let desired = panel.query();
                if query_matches(&view.query, &desired)
                    || matches!(
                        view.status.state,
                        ServiceState::Failed | ServiceState::RebuildPaused
                    )
                {
                    panel.update_data(view.snapshot.clone(), view.status.clone(), cx);
                }
            });
        }
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, cx| {
                settings.update_usage_status(view.status.clone(), cx)
            });
        }
        cx.notify();
    }

    /// Switch the centre and right panes to usage, creating the view the
    /// first time.
    pub(in crate::ui::app) fn show_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.main_view = MainView::Usage;
        if self.usage.panel.is_none() {
            let view = self.usage.current_view();
            let days = preferences::get(cx).tracking_days;
            let search = self.search.clone();
            let panel = cx.new(|cx| UsageView::new(view.snapshot, view.status, days, search, cx));
            cx.subscribe_in(&panel, window, |this, _, event, window, cx| match event {
                UsageEvent::QueryChanged(_) => this.request_usage_query(cx),
                UsageEvent::OpenSettings => this.open_settings(window, cx),
            })
            .detach();
            self.usage.panel = Some(panel);
        }
        self.sync_usage_scope(cx);
        self.request_usage_query(cx);
        cx.notify();
    }

    /// Point the usage view at the sidebar's scope: global covers every
    /// project, a project only its own activity.
    pub(in crate::ui::app) fn sync_usage_scope(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.usage.panel.clone() else {
            return;
        };
        let scope = match self.scope() {
            Scope::Global => UsageScope {
                project: None,
                label: "Global".into(),
            },
            Scope::Project(path) => UsageScope {
                project: Some(path.clone()),
                label: self.scope().label().into(),
            },
        };
        let loaded_skills = self
            .skills
            .iter()
            .filter(|skill| !skill.disabled && !skill.installs.is_empty())
            .cloned()
            .collect();
        panel.update(cx, |panel, cx| panel.set_scope(scope, loaded_skills, cx));
    }

    pub(in crate::ui::app) fn render_skill_usage(
        &self,
        skill: &Skill,
        cx: &App,
    ) -> impl IntoElement {
        let view = self.usage.current_view();
        let skill_ids = crate::usage::storage::installed_group_ids(skill, &view.snapshot)
            .unwrap_or_default()
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let rankings = view
            .snapshot
            .rankings
            .iter()
            .filter(|ranking| skill_ids.contains(&ranking.skill_id))
            .collect::<Vec<_>>();
        let activations = rankings
            .iter()
            .map(|ranking| ranking.counts.activations)
            .sum::<u64>();
        let main = rankings
            .iter()
            .map(|ranking| ranking.counts.main_activations)
            .sum::<u64>();
        let subagents = rankings
            .iter()
            .map(|ranking| ranking.counts.subagent_activations)
            .sum::<u64>();
        let unconfirmed = rankings
            .iter()
            .map(|ranking| ranking.counts.unconfirmed)
            .sum::<u64>();
        v_flex().gap_1().text_xs().text_color(cx.theme().muted_foreground)
            .child("Local skill activity")
            .child(if !rankings.is_empty() {
                format!("{activations} confirmed activations · {main} main · {subagents} subagent · {unconfirmed} unconfirmed")
            } else {
                crate::ui::usage::state_label(view.status.state).to_owned()
            })
            .when_some(rankings.iter().filter_map(|ranking| ranking.counts.last_used).max(), |this, timestamp| {
                this.child(chrono::DateTime::from_timestamp_millis(timestamp).map(|time| format!("Last used {} (recorded time)", time.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"))).unwrap_or_else(|| "Last-used timestamp unavailable".into()))
            })
    }
}

fn query_matches(actual: &Query, desired: &Query) -> bool {
    actual.agent == desired.agent
        && actual.project == desired.project
        && actual.role == desired.role
        && actual.metric == desired.metric
        && actual.skill_ids == desired.skill_ids
        && actual.range_days == desired.range_days
}
