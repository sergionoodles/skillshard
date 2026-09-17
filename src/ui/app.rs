//! The main window: scopes on the left, skills in the middle, detail on the right.

use crate::agents::Agent;
use crate::model::{InstallKind, Scope, Skill, UpdateState};
use crate::skills_cli::{self, Launcher};
use crate::{ops, paths, scan, updates};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::separator::Separator;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// Width of the scope sidebar.
const SIDEBAR_WIDTH: f32 = 240.;
/// Width of the detail pane, when a skill is selected.
const DETAIL_WIDTH: f32 = 360.;
/// Below this, the list column loses the worded update badge and shows an
/// arrow instead.
const COMPACT_LIST_WIDTH: f32 = 460.;

/// A line in the activity log, recording what the app did.
pub struct Activity {
    pub text: SharedString,
    pub failed: bool,
}

/// Root view.
pub struct Skillshard {
    /// Scopes the user can switch between: global plus any opened project.
    scopes: Vec<Scope>,
    active_scope: usize,
    skills: Vec<Skill>,
    selected: Option<String>,
    search: Entity<InputState>,
    activity: Vec<Activity>,
    /// Set while a CLI command is running, to keep actions from overlapping.
    busy: Option<SharedString>,
    launcher: Launcher,
    install: Option<Entity<super::install::InstallDialog>>,
}

impl Skillshard {
    /// The application as shipped: the user's global skills, plus any project
    /// they open later.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_scopes(vec![Scope::Global], window, cx)
    }

    /// Start on a specific set of scopes. Used by the UI tests to point the
    /// window at a fixture tree instead of the real home directory.
    pub fn with_scopes(scopes: Vec<Scope>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search skills…"));
        let mut this = Self {
            scopes,
            active_scope: 0,
            skills: Vec::new(),
            selected: None,
            search,
            activity: Vec::new(),
            busy: None,
            launcher: Launcher::detect(),
            install: None,
        };
        this.reload(cx);
        this
    }

    /// The scope currently being shown.
    fn scope(&self) -> &Scope {
        &self.scopes[self.active_scope]
    }

    /// Re-scan the active scope from disk.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let scope = self.scope().clone();
        self.skills = scan::scan(&scope);
        if let Some(name) = &self.selected {
            if !self.skills.iter().any(|s| &s.name == name) {
                self.selected = None;
            }
        }
        cx.notify();
    }

    fn log(&mut self, text: impl Into<SharedString>, failed: bool) {
        self.activity.push(Activity {
            text: text.into(),
            failed,
        });
        // Keep the panel short; it is a recent-activity view, not a full log.
        if self.activity.len() > 50 {
            self.activity.remove(0);
        }
    }

    /// Skills matching the search box.
    fn visible_skills(&self, cx: &App) -> Vec<&Skill> {
        let query = self.search.read(cx).value().to_lowercase();
        self.skills
            .iter()
            .filter(|s| {
                query.is_empty()
                    || s.name.to_lowercase().contains(&query)
                    || s.description.to_lowercase().contains(&query)
                    || s.source_label().to_lowercase().contains(&query)
            })
            .collect()
    }

    fn selected_skill(&self) -> Option<&Skill> {
        let name = self.selected.as_ref()?;
        self.skills.iter().find(|s| &s.name == name)
    }

    /// Turn a skill on or off without uninstalling it.
    fn toggle_skill(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(skill) = self.skills.iter().find(|s| s.name == name).cloned() else {
            return;
        };
        let result = if skill.disabled {
            ops::enable(&skill).map(|_| "enabled")
        } else {
            ops::disable(&skill).map(|_| "disabled")
        };
        match result {
            Ok(verb) => self.log(format!("{name} {verb}"), false),
            Err(e) => self.log(format!("{name}: {e}"), true),
        }
        self.reload(cx);
    }

    /// Link or unlink one agent.
    fn toggle_agent(&mut self, name: String, agent: &'static Agent, cx: &mut Context<Self>) {
        let Some(skill) = self.skills.iter().find(|s| s.name == name).cloned() else {
            return;
        };
        let result = if skill.is_enabled_for(agent) {
            ops::unlink_agent(&skill, agent).map(|_| "removed from")
        } else {
            ops::link_agent(&skill, agent).map(|_| "added to")
        };
        match result {
            Ok(verb) => self.log(format!("{name} {verb} {}", agent.display), false),
            Err(e) => self.log(format!("{}: {e}", agent.display), true),
        }
        self.reload(cx);
    }

    /// Give every agent that already has at least one skill this skill too.
    fn sync_everywhere(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(skill) = self.skills.iter().find(|s| s.name == name).cloned() else {
            return;
        };
        let scope = self.scope().clone();
        // Target every agent that is already in use in this scope, so syncing
        // does not create directories for agents the user does not have.
        let targets: Vec<&'static Agent> = self
            .in_use_agents()
            .into_iter()
            .filter(|a| ops::is_togglable(&scope, a))
            .collect();
        let errors = ops::sync_agents(&skill, &targets);
        if errors.is_empty() {
            self.log(format!("{name} synced to {} agents", targets.len()), false);
        } else {
            for (agent, e) in errors {
                self.log(format!("{agent}: {e}"), true);
            }
        }
        self.reload(cx);
    }

    /// Agents that hold at least one skill in this scope, plus any that have a
    /// skills directory already.
    fn in_use_agents(&self) -> Vec<&'static Agent> {
        let mut keys: Vec<&'static Agent> = Vec::new();
        for skill in &self.skills {
            for install in &skill.installs {
                if !keys.iter().any(|a| a.key == install.agent.key) {
                    keys.push(install.agent);
                }
            }
        }
        for agent in scan::present_agents(self.scope()) {
            if !keys.iter().any(|a| a.key == agent.key) {
                keys.push(agent);
            }
        }
        keys.sort_by_key(|a| a.display);
        keys
    }

    /// Check every tracked skill against its upstream source.
    fn check_updates(&mut self, cx: &mut Context<Self>) {
        let entries: Vec<(String, crate::model::LockEntry)> = self
            .skills
            .iter()
            .filter_map(|s| s.lock.clone().map(|l| (s.name.clone(), l)))
            .collect();
        if entries.is_empty() {
            self.log("no tracked skills to check", false);
            return;
        }
        for skill in &mut self.skills {
            if skill.lock.is_some() {
                skill.update = UpdateState::Checking;
            }
        }
        self.busy = Some("Checking for updates…".into());
        cx.notify();

        cx.spawn(async move |this, cx| {
            // Network work belongs off the UI thread.
            let checked = cx
                .background_executor()
                .spawn(async move {
                    entries
                        .into_iter()
                        .map(|(name, entry)| (name, updates::check(&entry)))
                        .collect::<Vec<_>>()
                })
                .await;

            this.update(cx, |this, cx| {
                this.busy = None;
                this.apply_update_states(checked, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Record the outcome of an update check against the listed skills.
    ///
    /// Separate from the network call so the result can be applied from
    /// anywhere, including the UI tests.
    pub fn apply_update_states(
        &mut self,
        states: Vec<(String, UpdateState)>,
        cx: &mut Context<Self>,
    ) {
        let mut available = 0;
        for (name, state) in states {
            if state == UpdateState::Available {
                available += 1;
            }
            if let Some(skill) = self.skills.iter_mut().find(|s| s.name == name) {
                skill.update = state;
            }
        }
        self.log(
            match available {
                0 => "all tracked skills are up to date".to_string(),
                1 => "1 skill has an update".to_string(),
                n => format!("{n} skills have updates"),
            },
            false,
        );
        cx.notify();
    }

    /// Run a `skills` subcommand, then re-scan.
    fn run_cli(&mut self, label: String, args: Vec<String>, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let launcher = self.launcher.clone();
        let cwd = skills_cli::working_dir(self.scope());
        self.busy = Some(label.clone().into());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { skills_cli::run(&launcher, &args, &cwd) })
                .await;

            this.update(cx, |this, cx| {
                this.busy = None;
                match outcome {
                    Ok(outcome) if outcome.success => this.log(format!("{label}: done"), false),
                    Ok(outcome) => {
                        // The CLI keeps diagnostics on stderr; surface its last
                        // line rather than a bare exit code.
                        let detail = outcome
                            .stderr
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .next_back()
                            .unwrap_or("command failed")
                            .to_string();
                        this.log(format!("{label}: {detail}"), true);
                    }
                    Err(e) => this.log(format!("{label}: {e}"), true),
                }
                this.reload(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn update_skill(&mut self, name: String, cx: &mut Context<Self>) {
        let args = skills_cli::update_args(&[name.clone()], self.scope());
        self.run_cli(format!("update {name}"), args, cx);
    }

    fn remove_skill(&mut self, name: String, cx: &mut Context<Self>) {
        let args = skills_cli::remove_args(&name, self.scope(), &[]);
        self.run_cli(format!("remove {name}"), args, cx);
    }
}

// ─── Rendering ───

/// Badge for states other than "an update is waiting".
fn secondary_update_tag(state: &UpdateState) -> Option<(&'static str, Tag)> {
    match state {
        UpdateState::Checking => Some(("checking…", Tag::secondary())),
        UpdateState::Failed(_) => Some(("check failed", Tag::danger())),
        UpdateState::Available
        | UpdateState::UpToDate
        | UpdateState::NotTracked
        | UpdateState::Unknown => None,
    }
}

/// The "an update is waiting" marker shown beside a skill's name.
///
/// A worded badge when the list column has room for it, and an upward arrow
/// when it does not — the name should give way to the rest of the row before
/// the badge starts wrapping.
fn update_marker(name: &str, compact: bool, cx: &App) -> AnyElement {
    if compact {
        return div()
            .id(SharedString::from(format!("update-icon-{name}")))
            .flex_shrink_0()
            .text_color(cx.theme().warning)
            .child(Icon::new(IconName::ArrowUp).small())
            .tooltip(|window, cx| Tooltip::new("An update is available").build(window, cx))
            .test_support()
            .into_any_element();
    }
    Tag::warning()
        .small()
        .flex_shrink_0()
        .child(
            div()
                .id(SharedString::from(format!("update-tag-{name}")))
                .child("update")
                .test_support(),
        )
        .into_any_element()
}

impl Skillshard {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let updatable = self
            .skills
            .iter()
            .filter(|s| s.update == UpdateState::Available)
            .count();

        h_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .font_semibold()
                    .text_lg()
                    .child("Skillshard"),
            )
            .child(Input::new(&self.search).id("search").w(px(280.)))
            .child(div().flex_1())
            .when(updatable > 0, |this| {
                this.child(Tag::warning().child(format!("{updatable} to update")))
            })
            .child(
                Button::new("check-updates")
                    .label("Check updates")
                    .disabled(self.busy.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.check_updates(cx))),
            )
            .child(
                Button::new("install")
                    .primary()
                    .label("Install skill")
                    .disabled(self.busy.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.open_install(window, cx))),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.skills.iter().filter(|s| !s.disabled).count();
        let disabled = self.skills.len() - enabled;

        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .p_3()
            .gap_1()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .pb_1()
                    .child("SCOPE"),
            )
            .children(
                self.scopes
                    .iter()
                    .enumerate()
                    .map(|(index, scope)| {
                        let active = index == self.active_scope;
                        Button::new(SharedString::from(format!("scope-{index}")))
                            .w_full()
                            .justify_start()
                            .label(scope.label())
                            .when(active, |b| b.primary())
                            .when(!active, |b| b.ghost())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active_scope = index;
                                this.selected = None;
                                this.reload(cx);
                            }))
                    })
                    .collect::<Vec<_>>(),
            )
            .child(
                Button::new("open-project")
                    .w_full()
                    .justify_start()
                    .ghost()
                    .label("Add project…")
                    .on_click(cx.listener(|this, _, window, cx| this.pick_project(window, cx))),
            )
            .child(div().h_2())
            .child(Separator::horizontal())
            .child(div().h_2())
            .child(
                v_flex()
                    .gap_1()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{enabled} enabled"))
                    .child(format!("{disabled} disabled"))
                    .child(format!("{} agents in use", self.in_use_agents().len())),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(paths::shorten(&self.scope().canonical_dir())),
            )
    }

    /// One row in the skill list.
    ///
    /// `compact` is set when the list column is too narrow to carry a worded
    /// update badge; the badge collapses to an arrow instead.
    fn render_skill_row(
        &self,
        skill: &Skill,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = skill.name.clone();
        let selected = self.selected.as_deref() == Some(skill.name.as_str());
        let agent_count = skill.installs.len();
        let missing = skill.canonical.is_none() && skill.installs.is_empty();
        let needs_update = skill.update == UpdateState::Available;

        h_flex()
            .id(SharedString::from(format!("row-{}", skill.name)))
            .w_full()
            .px_3()
            .py_2()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(selected, |this| this.bg(cx.theme().list_active))
            .hover(|this| this.bg(cx.theme().list_hover))
            .on_click(cx.listener({
                let name = name.clone();
                move |this, _, _, cx| {
                    this.selected = Some(name.clone());
                    cx.notify();
                }
            }))
            .child(
                v_flex()
                    .flex_1()
                    // Without a zero minimum a flex child refuses to shrink
                    // below its content, and nothing would ever truncate.
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .font_medium()
                                    .flex_shrink(1.)
                                    .min_w_0()
                                    .truncate()
                                    .child(skill.display_name().to_string()),
                            )
                            // The status markers keep their full width; the
                            // name gives way to them instead.
                            .when(needs_update, |this| {
                                this.child(update_marker(&skill.name, compact, cx))
                            })
                            .when(!needs_update, |this| {
                                this.when_some(
                                    secondary_update_tag(&skill.update),
                                    |this, (label, tag)| {
                                        this.child(tag.small().flex_shrink_0().child(label))
                                    },
                                )
                            })
                            .when(missing, |this| {
                                this.child(
                                    Tag::danger().small().flex_shrink_0().child("not on disk"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(if skill.description.is_empty() {
                                "No description".to_string()
                            } else {
                                skill.description.clone()
                            }),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{agent_count} agents")),
            )
            // The on/off switch sits at the trailing edge of the row.
            .child(
                Switch::new(SharedString::from(format!("toggle-{}", skill.name)))
                    .checked(!skill.disabled)
                    .disabled(missing)
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, _, cx| this.toggle_skill(name.clone(), cx)
                    })),
            )
            // Makes the row addressable by the headless UI tests. Compiles
            // away to the element itself in normal builds.
            .test_support()
    }

    fn render_list(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let skills: Vec<Skill> = self.visible_skills(cx).into_iter().cloned().collect();
        v_flex()
            .id("skill-list")
            .flex_1()
            // A flex child sizes to its content unless it is allowed to shrink
            // below it; without this, one long description widens the column
            // and pushes the detail pane off screen.
            .min_w_0()
            .h_full()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .when(skills.is_empty(), |this| {
                this.child(
                    div()
                        .p_6()
                        .text_color(cx.theme().muted_foreground)
                        .child("No skills here yet. Use “Install skill” to add one."),
                )
            })
            .children(
                skills
                    .iter()
                    .map(|skill| self.render_skill_row(skill, compact, cx))
                    .collect::<Vec<_>>(),
            )
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(skill) = self.selected_skill().cloned() else {
            return v_flex()
                .w(px(DETAIL_WIDTH))
                .h_full()
                .p_4()
                .border_l_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child("Select a skill to see where it is installed."),
                )
                .into_any_element();
        };

        let name = skill.name.clone();
        let scope = self.scope().clone();
        let togglable: Vec<&'static Agent> = self
            .in_use_agents()
            .into_iter()
            .filter(|a| ops::is_togglable(&scope, a))
            .collect();
        let shared: Vec<&'static Agent> = skill
            .installs
            .iter()
            .filter(|i| i.kind == InstallKind::Canonical)
            .map(|i| i.agent)
            .collect();

        v_flex()
            .id("detail")
            .w(px(DETAIL_WIDTH))
            .h_full()
            .p_4()
            .gap_3()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_lg()
                    .font_semibold()
                    .truncate()
                    .child(skill.display_name().to_string()),
            )
            // The description wraps here rather than truncating: the detail
            // pane is where you actually read it.
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(skill.description.clone()),
            )
            .child(Separator::horizontal())
            .child(self.render_meta(&skill, cx))
            .child(Separator::horizontal())
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .when(skill.update == UpdateState::Available, |this| {
                        this.child(
                            Button::new("do-update")
                                .primary()
                                .small()
                                .label("Update")
                                .disabled(self.busy.is_some())
                                .on_click(cx.listener({
                                    let name = name.clone();
                                    move |this, _, _, cx| this.update_skill(name.clone(), cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("sync")
                            .small()
                            .label("Sync to all agents")
                            .on_click(cx.listener({
                                let name = name.clone();
                                move |this, _, _, cx| this.sync_everywhere(name.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new("move-scope")
                            .small()
                            .label(if scope.is_global() {
                                "Move to project…"
                            } else {
                                "Move to global"
                            })
                            .disabled(self.busy.is_some() || self.scopes.len() < 2)
                            .on_click(cx.listener({
                                let name = name.clone();
                                move |this, _, _, cx| this.move_scope(name.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new("remove")
                            .small()
                            .danger()
                            .label("Uninstall")
                            .disabled(self.busy.is_some())
                            .on_click(cx.listener({
                                let name = name.clone();
                                move |this, _, _, cx| this.remove_skill(name.clone(), cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("AGENTS"),
            )
            .children(
                togglable
                    .into_iter()
                    .map(|agent| {
                        let on = skill.is_enabled_for(agent);
                        h_flex()
                            .w_full()
                            .py_1()
                            .gap_2()
                            .items_center()
                            .child(
                                Checkbox::new(SharedString::from(format!("agent-{}", agent.key)))
                                    .checked(on)
                                    .disabled(skill.disabled || skill.canonical.is_none())
                                    .on_click(cx.listener({
                                        let name = name.clone();
                                        move |this, _, _, cx| {
                                            this.toggle_agent(name.clone(), agent, cx)
                                        }
                                    })),
                            )
                            .child(div().text_sm().child(agent.display))
                    })
                    .collect::<Vec<_>>(),
            )
            .when(!shared.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "Always on while enabled (they read {} directly): {}",
                            paths::shorten(&scope.canonical_dir()),
                            shared
                                .iter()
                                .map(|a| a.display)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )),
                )
            })
            .into_any_element()
    }

    fn render_meta(&self, skill: &Skill, cx: &mut Context<Self>) -> impl IntoElement {
        let update_text = match &skill.update {
            UpdateState::UpToDate => "up to date".to_string(),
            UpdateState::Available => "update available".to_string(),
            UpdateState::Checking => "checking…".to_string(),
            UpdateState::Failed(e) => format!("check failed — {e}"),
            UpdateState::NotTracked => "not tracked by the CLI".to_string(),
            UpdateState::Unknown => "not checked".to_string(),
        };
        let location = skill
            .content_path()
            .map(paths::shorten)
            .unwrap_or_else(|| "missing".to_string());

        v_flex()
            .gap_1()
            .text_sm()
            .child(meta_row("Source", skill.source_label().to_string(), cx))
            .child(meta_row("Updates", update_text, cx))
            .child(meta_row("Location", location, cx))
            .child(meta_row(
                "State",
                if skill.disabled {
                    "disabled".to_string()
                } else {
                    format!("enabled for {} agents", skill.installs.len())
                },
                cx,
            ))
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let last = self.activity.last();
        h_flex()
            .w_full()
            .px_4()
            .py_2()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_sm()
            .when_some(self.busy.clone(), |this, label| {
                this.child(Spinner::new().small()).child(label)
            })
            .when(self.busy.is_none(), |this| {
                this.child(match last {
                    Some(entry) => div()
                        .text_color(if entry.failed {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(entry.text.clone()),
                    None => div()
                        .text_color(cx.theme().muted_foreground)
                        .child("Ready"),
                })
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(match &self.launcher {
                        Launcher::Binary(bin) => format!("using {bin}"),
                        Launcher::Npx { spec } => format!("using npx {spec}"),
                    }),
            )
    }
}

/// One `label: value` line in the detail pane.
fn meta_row(label: &str, value: String, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .w(px(76.))
                .flex_shrink_0()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(div().flex_1().child(value))
}

impl Render for Skillshard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The sidebar and detail pane are fixed width, so what is left over
        // for the list is exact — no measurement pass needed.
        let detail_width = if self.selected_skill().is_some() {
            DETAIL_WIDTH
        } else {
            0.
        };
        let list_width = f32::from(window.viewport_size().width) - SIDEBAR_WIDTH - detail_width;
        let compact = list_width < COMPACT_LIST_WIDTH;

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_list(compact, cx))
                    .child(self.render_detail(cx)),
            )
            .child(self.render_status(cx))
            .children(self.install.clone())
    }
}

// ─── Dialog, projects and scope moves ───

impl Skillshard {
    /// Open the install dialog, pre-filled for the active scope.
    fn open_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scope = self.scope().clone();
        let agents = self.in_use_agents();
        let dialog = cx.new(|cx| super::install::InstallDialog::new(&scope, agents, window, cx));

        cx.subscribe(&dialog, |this, _, event, cx| match event {
            super::install::InstallEvent::Cancel => {
                this.install = None;
                cx.notify();
            }
            super::install::InstallEvent::Submit(request) => {
                this.install = None;
                this.start_install(request.clone(), cx);
            }
        })
        .detach();

        self.install = Some(dialog);
        cx.notify();
    }

    /// Run the install the dialog assembled.
    fn start_install(&mut self, mut request: crate::skills_cli::InstallRequest, cx: &mut Context<Self>) {
        // The dialog chooses global vs project; the concrete project comes
        // from the window's active scope.
        if !request.scope.is_global() {
            match self.scopes.iter().find(|s| !s.is_global()) {
                Some(project) => request.scope = project.clone(),
                None => {
                    self.log("add a project before installing into one", true);
                    cx.notify();
                    return;
                }
            }
        }
        let label = format!("install {}", request.source);
        let cwd = skills_cli::working_dir(&request.scope);
        self.run_steps(label, vec![(request.args(), cwd)], cx);
    }

    /// Add a project directory to the scope list.
    fn pick_project(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose project".into()),
        });

        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(chosen))) = paths.await else {
                return;
            };
            let Some(root) = chosen.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                let scope = Scope::Project(root);
                if let Some(index) = this.scopes.iter().position(|s| s == &scope) {
                    this.active_scope = index;
                } else {
                    this.scopes.push(scope);
                    this.active_scope = this.scopes.len() - 1;
                }
                this.selected = None;
                this.reload(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Move a skill between global and project scope.
    ///
    /// Reinstalling through the CLI (rather than moving files) is what keeps
    /// the lock files correct in both scopes, so update tracking survives the
    /// move.
    fn move_scope(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(skill) = self.skills.iter().find(|s| s.name == name).cloned() else {
            return;
        };
        let from = self.scope().clone();
        let Some(to) = self
            .scopes
            .iter()
            .find(|s| s.is_global() != from.is_global())
            .cloned()
        else {
            self.log("add a project to move skills into", true);
            cx.notify();
            return;
        };

        let Some(source) = skill
            .lock
            .as_ref()
            .map(|l| l.source_url.clone().unwrap_or_else(|| l.source.clone()))
        else {
            // Without a lock entry there is no source to reinstall from.
            self.log(
                format!("{name} has no recorded source, so it cannot be moved"),
                true,
            );
            cx.notify();
            return;
        };

        let agents: Vec<String> = skill
            .installs
            .iter()
            .filter(|i| i.kind.is_unlinkable())
            .map(|i| i.agent.key.to_string())
            .collect();

        let install = crate::skills_cli::InstallRequest {
            source,
            scope: to.clone(),
            skills: vec![name.clone()],
            agents,
            copy: false,
        };
        let steps = vec![
            (install.args(), skills_cli::working_dir(&to)),
            (
                skills_cli::remove_args(&name, &from, &[]),
                skills_cli::working_dir(&from),
            ),
        ];
        self.run_steps(format!("move {name} to {}", to.label()), steps, cx);
    }

    /// Run CLI commands in order, stopping at the first failure.
    fn run_steps(
        &mut self,
        label: String,
        steps: Vec<(Vec<String>, std::path::PathBuf)>,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        let launcher = self.launcher.clone();
        self.busy = Some(label.clone().into());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let mut last = None;
                    for (args, cwd) in steps {
                        match skills_cli::run(&launcher, &args, &cwd) {
                            Ok(outcome) if outcome.success => last = Some(Ok(outcome)),
                            other => return other,
                        }
                    }
                    last.unwrap_or_else(|| {
                        Err(std::io::Error::other("nothing to run"))
                    })
                })
                .await;

            this.update(cx, |this, cx| {
                this.busy = None;
                match outcome {
                    Ok(outcome) if outcome.success => this.log(format!("{label}: done"), false),
                    Ok(outcome) => {
                        let detail = outcome
                            .stderr
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .next_back()
                            .unwrap_or("command failed")
                            .to_string();
                        this.log(format!("{label}: {detail}"), true);
                    }
                    Err(e) => this.log(format!("{label}: {e}"), true),
                }
                this.reload(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
