//! The usage view: local skill activity for the scope picked in the sidebar,
//! shown in place of the skill list (stats) and the detail pane (filters).
//! Rendering only reads background query snapshots; it never scans histories.

mod chart;
mod filters;
mod insights;
mod labels;
mod ranking;
mod stats;

pub use labels::state_label;

use crate::model::Skill;
use crate::preferences::TrackingDays;
use crate::usage::{Metric, Query, RoleFilter, Snapshot, Status};
use chrono::Utc;
use gpui_kit::component::input::InputState;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use insights::{InstalledSkill, Selection};
use std::collections::HashMap;
use std::path::PathBuf;

pub enum UsageEvent {
    QueryChanged(Query),
    OpenSettings,
}

impl EventEmitter<UsageEvent> for UsageView {}

#[derive(Clone)]
enum Filter {
    Agent(Option<String>),
    Role(RoleFilter),
    Metric(Metric),
    Range(TrackingDays),
    Skill(i64, String),
    ClearSkills,
}

/// Which slice of history the sidebar selection stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageScope {
    /// `None` for global, which covers activity in every project.
    pub project: Option<PathBuf>,
    pub label: SharedString,
}

pub struct UsageView {
    snapshot: Snapshot,
    status: Status,
    /// Agent, role and metric filters; range, project and skills live apart.
    filters: Query,
    scope: UsageScope,
    tracking_days: TrackingDays,
    range: TrackingDays,
    selection: Selection,
    /// Set from a filter change until the matching snapshot arrives; the
    /// previous figures stay on screen meanwhile.
    query_pending: bool,
    /// Skills an agent loads in the scope, to spot those that go unused.
    loaded_skills: Vec<Skill>,
    installed: Vec<InstalledSkill>,
    ranking_contexts: HashMap<i64, String>,
    /// The window's search box, which narrows the skill lists here too.
    search: Entity<InputState>,
    _search_observer: Subscription,
}

impl UsageView {
    pub fn new(
        snapshot: Snapshot,
        status: Status,
        tracking_days: TrackingDays,
        search: Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _search_observer = cx.observe(&search, |_, _, cx| cx.notify());
        let ranking_contexts = ranking_contexts(&snapshot);
        Self {
            snapshot,
            status,
            filters: Query::default(),
            scope: UsageScope {
                project: None,
                label: "Global".into(),
            },
            tracking_days,
            range: tracking_days,
            selection: Selection::default(),
            query_pending: false,
            loaded_skills: Vec::new(),
            installed: Vec::new(),
            ranking_contexts,
            search,
            _search_observer,
        }
    }

    pub fn query(&self) -> Query {
        // A shorter reporting range remains rolling during background refreshes.
        Query {
            project: self.scope.project.clone(),
            since: Some(self.range.cutoff(Utc::now().timestamp_millis())),
            range_days: Some(self.range),
            skill_ids: self.selection.ids(),
            ..self.filters.clone()
        }
    }

    pub fn update_data(&mut self, snapshot: Snapshot, status: Status, cx: &mut Context<Self>) {
        self.ranking_contexts = ranking_contexts(&snapshot);
        self.installed = installed_skills(&self.loaded_skills, &snapshot);
        self.snapshot = snapshot;
        self.status = status;
        self.query_pending = false;
        cx.notify();
    }

    pub fn update_tracking_days(&mut self, days: TrackingDays, cx: &mut Context<Self>) {
        self.tracking_days = days;
        if self.range.days() > days.days() {
            self.set_filter(Filter::Range(days), cx);
        }
        cx.notify();
    }

    /// Follow the sidebar: a new scope re-queries, while `loaded_skills`
    /// only refreshes which skills count as unused.
    pub fn set_scope(
        &mut self,
        scope: UsageScope,
        loaded_skills: Vec<Skill>,
        cx: &mut Context<Self>,
    ) {
        self.installed = installed_skills(&loaded_skills, &self.snapshot);
        self.loaded_skills = loaded_skills;
        if self.scope != scope {
            self.scope = scope;
            self.request_query(cx);
        }
        cx.notify();
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        match filter {
            Filter::Agent(agent) => self.filters.agent = agent,
            Filter::Role(role) => self.filters.role = role,
            Filter::Metric(metric) => self.filters.metric = metric,
            Filter::Range(range) => self.range = range,
            Filter::Skill(skill_id, name) => {
                self.selection = self.selection.toggled(skill_id, &name);
            }
            Filter::ClearSkills => self.selection = Selection::default(),
        }
        self.request_query(cx);
    }

    fn request_query(&mut self, cx: &mut Context<Self>) {
        self.query_pending = true;
        cx.emit(UsageEvent::QueryChanged(self.query()));
        cx.notify();
    }

    /// Lower-cased search text, empty when the box is.
    fn search_text(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }
}

fn ranking_contexts(snapshot: &Snapshot) -> HashMap<i64, String> {
    snapshot
        .rankings
        .iter()
        .map(|ranking| (ranking.skill_id, labels::identity_label(&ranking.identity)))
        .collect()
}

fn installed_skills(skills: &[Skill], snapshot: &Snapshot) -> Vec<InstalledSkill> {
    skills
        .iter()
        .map(|skill| InstalledSkill {
            name: skill.display_name().to_string(),
            // A skill whose identity cannot be read has no recorded use.
            ids: crate::usage::storage::installed_group_ids(skill, snapshot).unwrap_or_default(),
        })
        .collect()
}

impl Render for UsageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("usage-view")
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_stats(cx))
            .child(self.render_filters(cx))
            .test_support()
    }
}

#[cfg(test)]
mod tests;
