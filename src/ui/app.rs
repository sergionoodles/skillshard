//! The main window: scopes on the left, skills in the middle, detail on the right.

use crate::agents::Agent;
use crate::editors::{self, Editor};
use crate::model::{InstallKind, Scope, Skill, UpdateState};
use crate::skills_cli::{self, Launcher};
use crate::tokens::{self, TokenCost};
use crate::{assets, ops, paths, preferences, registry, scan, themes, updates};
use gpui_kit::component::button::{Button, ButtonVariants, DropdownButton};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::separator::Separator;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Width of both side panes: the scope sidebar and the detail pane.
const SIDE_PANE_WIDTH: f32 = 300.;
/// Below this, the list column loses the worded update badge and shows an
/// arrow instead.
const COMPACT_LIST_WIDTH: f32 = 460.;
/// Icon size for sidebar entries, sized to sit beside `text_base` labels.
const NAV_ICON_SIZE: f32 = 18.;

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
    /// Global skills, scanned alongside a project so the sidebar can total
    /// what a session in that project actually carries. Empty otherwise.
    global_skills: Vec<Skill>,
    selected: Option<String>,
    search: Entity<InputState>,
    activity: Vec<Activity>,
    /// Set while a CLI command is running, to keep actions from overlapping.
    busy: Option<SharedString>,
    launcher: Launcher,
    install: Option<Entity<super::install::InstallDialog>>,
    settings: Option<Entity<super::settings::SettingsDialog>>,
    project_settings: Option<Entity<super::project_settings::ProjectSettingsDialog>>,
    diff: Option<Entity<super::diff::DiffDialog>>,
    /// skills.sh install counts, keyed by [`install_key`].
    installs: HashMap<String, u64>,
    /// Counts already asked for, so re-scans do not repeat in-flight lookups.
    installs_requested: HashSet<String>,
    /// Agents installed on this machine, detected once at startup.
    installed: Vec<&'static Agent>,
    /// Editors a skill folder can be opened in, detected once at startup.
    editors: Vec<Editor>,
    /// A folder a skill was just sent to, while asking whether it should join
    /// the sidebar as a project.
    offered_project: Option<PathBuf>,
    _subscriptions: Vec<Subscription>,
}

/// Identifies a skill on skills.sh: the same name can come from many sources.
fn install_key(source: &str, name: &str) -> String {
    format!("{source}/{name}")
}

/// Sending a skill to another scope either keeps it here or takes it away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transfer {
    Move,
    Copy,
}

impl Transfer {
    /// Lower-case verb for the activity log.
    fn verb(self) -> &'static str {
        match self {
            Transfer::Move => "move",
            Transfer::Copy => "copy",
        }
    }
}

/// The agents the app speaks for in `scope`: those installed on this machine
/// that can hold skills here, plus any holding a skill in its own directory
/// here — detected or not, that entry is something to manage.
fn shown_agents(
    scope: &Scope,
    skills: &[Skill],
    installed: &[&'static Agent],
) -> Vec<&'static Agent> {
    let mut agents: Vec<&'static Agent> = installed
        .iter()
        .copied()
        .filter(|a| scope.agent_dir(a).is_some())
        .collect();
    for install in skills.iter().flat_map(|s| &s.installs) {
        if install.kind.is_unlinkable() && !agents.iter().any(|a| a.key == install.agent.key) {
            agents.push(install.agent);
        }
    }
    agents.sort_by_key(|a| a.display);
    agents
}

impl Skillshard {
    /// The application as shipped: the user's global skills, plus any project
    /// they open later.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_scopes(vec![Scope::Global], window, cx)
    }

    /// Replace the agents detected on this machine. The UI tests use this so
    /// what they see does not depend on what the test machine has installed.
    pub fn with_installed_agents(mut self, agents: Vec<&'static Agent>) -> Self {
        self.installed = agents;
        self
    }

    /// Replace the editors detected on this machine, for the same reason.
    pub fn with_editors(mut self, editors: Vec<Editor>) -> Self {
        self.editors = editors;
        self
    }

    /// Start on a specific set of scopes, followed by the projects saved in
    /// preferences. The UI tests use this to point the window at a fixture
    /// tree instead of the real home directory.
    pub fn with_scopes(
        mut scopes: Vec<Scope>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        for project in &preferences::get(cx).projects {
            let scope = Scope::Project(project.path.clone());
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search skills…"));
        themes::apply(window, cx);
        let subscriptions = vec![
            // Only matters while the appearance preference is "system", but
            // `apply` already knows that.
            window.observe_window_appearance(themes::apply),
            cx.observe_global_in::<preferences::Store>(window, |this, window, cx| {
                themes::apply(window, cx);
                this.report_preferences_error(cx);
                cx.notify();
            }),
        ];

        let mut this = Self {
            scopes,
            active_scope: 0,
            skills: Vec::new(),
            global_skills: Vec::new(),
            selected: None,
            search,
            activity: Vec::new(),
            busy: None,
            launcher: Launcher::detect(),
            install: None,
            settings: None,
            project_settings: None,
            diff: None,
            installs: HashMap::new(),
            installs_requested: HashSet::new(),
            installed: scan::installed_agents(),
            editors: editors::detect(),
            offered_project: None,
            _subscriptions: subscriptions,
        };
        this.report_preferences_error(cx);
        this.reload(cx);
        if preferences::get(cx).check_updates_on_startup {
            this.check_updates(cx);
        }
        this
    }

    /// Surface a failure to read or save preferences, once.
    fn report_preferences_error(&mut self, cx: &mut Context<Self>) {
        if cx.global::<preferences::Store>().error.is_none() {
            return;
        }
        if let Some(error) = cx.global_mut::<preferences::Store>().error.take() {
            self.log(format!("settings: {error}"), true);
        }
    }

    /// Look up skills.sh install counts for tracked skills not yet known.
    fn fetch_install_counts(&mut self, cx: &mut Context<Self>) {
        for skill in &self.skills {
            let Some(lock) = &skill.lock else {
                continue;
            };
            let key = install_key(&lock.source, &skill.name);
            if !self.installs_requested.insert(key.clone()) {
                continue;
            }
            let (source, name) = (lock.source.clone(), skill.name.clone());
            // One task per skill: each lookup is a full search, and doing them
            // one after another takes seconds.
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { registry::fetch_install_count(&source, &name) })
                    .await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(Some(count)) => {
                            this.installs.insert(key, count);
                        }
                        // Not every source is listed on skills.sh.
                        Ok(None) => {}
                        Err(e) => {
                            // Allow a later re-scan to try again.
                            this.installs_requested.remove(&key);
                            this.log(format!("install counts unavailable: {e}"), true);
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// The skills.sh install count for `skill`, when known.
    fn install_count(&self, skill: &Skill) -> Option<u64> {
        let lock = skill.lock.as_ref()?;
        self.installs
            .get(&install_key(&lock.source, &skill.name))
            .copied()
    }

    /// The scope currently being shown.
    fn scope(&self) -> &Scope {
        &self.scopes[self.active_scope]
    }

    /// Re-scan the active scope from disk.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let scope = self.scope().clone();
        self.skills = scan::scan(&scope);
        // Only when global is one of the window's scopes: the UI tests leave
        // it out so the real home directory never leaks into their totals.
        self.global_skills = if !scope.is_global() && self.scopes.contains(&Scope::Global) {
            scan::scan(&Scope::Global)
        } else {
            Vec::new()
        };
        if let Some(name) = &self.selected {
            if !self.skills.iter().any(|s| &s.name == name) {
                self.selected = None;
            }
        }
        self.fetch_install_counts(cx);
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

    fn shown_agents(&self) -> Vec<&'static Agent> {
        shown_agents(self.scope(), &self.skills, &self.installed)
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

    /// Open a skill's folder in `editor`, reporting how it went.
    fn open_in_editor(&mut self, editor: Editor, dir: PathBuf, cx: &mut Context<Self>) {
        let label = format!("open {} in {}", paths::shorten(&dir), editor.name);
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async move { editor.open(&dir) })
                .await;
            this.update(cx, |this, cx| {
                match status {
                    Ok(status) if status.success() => this.log(format!("{label}: done"), false),
                    Ok(status) => this.log(format!("{label}: {status}"), true),
                    Err(e) => this.log(format!("{label}: {e}"), true),
                }
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

        // Left and right sections share the leftover space equally, which is
        // what keeps the search box truly centred.
        h_flex()
            .w_full()
            .px_4()
            .py_2()
            .gap_4()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(h_flex().flex_1().flex_basis(px(0.)).child(wordmark(cx)))
            .child(Input::new(&self.search).id("search").small().w(px(360.)))
            .child(
                h_flex()
                    .flex_1()
                    .flex_basis(px(0.))
                    .justify_end()
                    .gap_2()
                    .when(updatable > 0, |this| {
                        this.child(
                            Tag::warning()
                                .small()
                                .child(format!("{updatable} to update")),
                        )
                    })
                    .child(
                        Button::new("check-updates")
                            .small()
                            .ghost()
                            .label("Check updates")
                            .disabled(self.busy.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.check_updates(cx))),
                    )
                    .child(
                        Button::new("install")
                            .small()
                            .primary()
                            .label("Install skill")
                            .disabled(self.busy.is_some())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_install(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("open-settings")
                            .small()
                            .ghost()
                            .icon(IconName::Settings)
                            .tooltip("Settings")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                            ),
                    ),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.skills.iter().filter(|s| !s.disabled).count();
        let disabled = self.skills.len() - enabled;
        let always = always_cost(&self.skills);
        // Shown for a project whenever global is in the sidebar, even with no
        // global skills: the total is then the project figure, which is true.
        let combined = (!self.scope().is_global() && self.scopes.contains(&Scope::Global))
            .then(|| combined_always_cost(&self.global_skills, &self.skills));

        let mut items: Vec<AnyElement> = Vec::new();
        for (index, scope) in self.scopes.iter().enumerate() {
            items.push(self.render_scope_item(index, scope, cx).into_any_element());
            // Global stands apart from the projects listed under it.
            if scope.is_global() {
                items.push(
                    div()
                        .py_1()
                        .child(Separator::horizontal())
                        .into_any_element(),
                );
            }
        }

        v_flex()
            .w(px(SIDE_PANE_WIDTH))
            .flex_shrink_0()
            .h_full()
            .p_3()
            .gap_0p5()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(section_label("Scope", cx))
            .children(items)
            .child(
                nav_row("open-project", cx)
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Icon::empty()
                            .path("icons/folder-plus.svg")
                            .with_size(px(NAV_ICON_SIZE)),
                    )
                    .child("Add project…")
                    .on_click(cx.listener(|this, _, window, cx| this.pick_project(window, cx)))
                    .test_support(),
            )
            .child(div().flex_1())
            .child(
                v_flex()
                    .gap_0p5()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{enabled} enabled · {disabled} disabled · {}",
                        plural(self.shown_agents().len(), "agent")
                    ))
                    .child(
                        always_row("always-total", always.tokens, "tokens always loaded", cx)
                            .aria_label(format!(
                                "{} tokens always loaded",
                                registry::format_count(always.tokens as u64)
                            ))
                            .tooltip(move |window, cx| {
                                Tooltip::new(format!(
                                    "The descriptions of the {} an agent loads here sit in the \
                                     system prompt of every session, whether or not they are \
                                     used. Estimated.",
                                    plural(always.skills, "skill"),
                                ))
                                .build(window, cx)
                            })
                            .test_support(),
                    )
                    .when_some(combined, |this, combined| {
                        this.child(
                            always_row("always-combined", combined.tokens, "total with global", cx)
                                .aria_label(format!(
                                    "{} tokens always loaded with global",
                                    registry::format_count(combined.tokens as u64)
                                ))
                                .tooltip(move |window, cx| {
                                    let shadowed = if combined.shadowed == 0 {
                                        String::new()
                                    } else {
                                        format!(
                                            " {} named like a global skill {} counted once, as \
                                             the global copy takes precedence.",
                                            plural(combined.shadowed, "project skill"),
                                            if combined.shadowed == 1 { "is" } else { "are" },
                                        )
                                    };
                                    Tooltip::new(format!(
                                        "What a session in this project carries: {} from this \
                                         project and global combined.{shadowed} Estimated.",
                                        plural(combined.skills, "skill"),
                                    ))
                                    .build(window, cx)
                                })
                                .test_support(),
                        )
                    })
                    .child(
                        div()
                            .truncate()
                            .child(paths::shorten(&self.scope().canonical_dir())),
                    ),
            )
    }

    /// A sidebar entry for one scope: its icon, name and, for projects, a
    /// button to the project's settings.
    fn render_scope_item(
        &self,
        index: usize,
        scope: &Scope,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = index == self.active_scope;
        let (icon, tint) = match scope {
            Scope::Global => (Icon::new(IconName::Globe), None),
            Scope::Project(path) => {
                let project = preferences::get(cx).project(path);
                (
                    super::project_settings::icon(project),
                    super::project_settings::color(project, cx),
                )
            }
        };

        nav_row(SharedString::from(format!("scope-{index}")), cx)
            .when(active, |this| this.bg(cx.theme().list_active).font_medium())
            .child(
                icon.with_size(px(NAV_ICON_SIZE))
                    .text_color(tint.unwrap_or(cx.theme().muted_foreground)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("scope-{index}-label")))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(scope.label())
                    .test_support(),
            )
            .when(!scope.is_global(), |this| {
                this.child(
                    Button::new(SharedString::from(format!("project-settings-{index}")))
                        .small()
                        .ghost()
                        .icon(Icon::empty().path("icons/ellipsis.svg"))
                        .tooltip("Project settings")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // The button sits inside the row; without this
                            // the click would also select the project.
                            cx.stop_propagation();
                            this.open_project_settings(index, cx)
                        })),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.active_scope = index;
                this.selected = None;
                this.reload(cx);
            }))
            .test_support()
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
        let installs = self.install_count(skill);

        h_flex()
            .id(SharedString::from(format!("row-{}", skill.name)))
            .w_full()
            .px_4()
            .py_2()
            .gap_4()
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
                v_flex()
                    .flex_shrink_0()
                    .gap_0p5()
                    .items_end()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .when_some(installs, |this, count| {
                                this.child(
                                    h_flex()
                                        .id(SharedString::from(format!("installs-{}", skill.name)))
                                        .gap_1()
                                        .items_center()
                                        .child(Icon::empty().path("icons/download.svg").xsmall())
                                        .child(registry::format_count(count))
                                        .tooltip(|window, cx| {
                                            Tooltip::new("Installs on skills.sh").build(window, cx)
                                        })
                                        .test_support(),
                                )
                            })
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(Icon::new(IconName::Bot).xsmall())
                                    .child(plural(agent_count, "agent")),
                            ),
                    )
                    .when(!skill.cost.is_empty(), |this| {
                        this.child(row_cost(&skill.name, &skill.cost, cx))
                    }),
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
                .w(px(SIDE_PANE_WIDTH))
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

        let scope = self.scope().clone();
        // An agent that reads .agents/skills sees the skill there whatever its
        // own directory holds, so it belongs in the shared row. Without a
        // canonical copy, its own directory is the only place it has it.
        let (shared, own): (Vec<_>, Vec<_>) = skill
            .installs
            .iter()
            .filter(|i| i.kind != InstallKind::Inherited)
            .partition(|i| {
                i.kind == InstallKind::Canonical
                    || (skill.canonical.is_some() && scope.reads_canonical(i.agent))
            });
        let shown = self.shown_agents();
        let shared: Vec<&'static Agent> = shared
            .into_iter()
            .map(|i| i.agent)
            .filter(|a| shown.contains(a))
            .collect();
        let mut togglable: Vec<&'static Agent> = shown
            .into_iter()
            .filter(|a| ops::is_togglable(&scope, a))
            .collect();
        for install in own {
            if !togglable.iter().any(|a| a.key == install.agent.key) {
                togglable.push(install.agent);
            }
        }
        togglable.sort_by_key(|a| a.display);
        let inherited = skill
            .installs
            .iter()
            .filter(|i| i.kind == InstallKind::Inherited)
            .map(|i| i.agent.display)
            .collect::<Vec<_>>()
            .join(", ");

        v_flex()
            .id("detail")
            .w(px(SIDE_PANE_WIDTH))
            .h_full()
            .p_4()
            .gap_3()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_lg()
                            .font_semibold()
                            .truncate()
                            .child(skill.display_name().to_string()),
                    )
                    .when_some(skill.content_path(), |this, dir| {
                        this.child(self.render_edit_button(dir, cx))
                    }),
            )
            // The description wraps here rather than truncating: the detail
            // pane is where you actually read it. It is the only part that
            // gives up height, scrolling on its own so the controls below
            // stay put.
            .child(
                div()
                    .id("detail-description")
                    .w_full()
                    .min_w_0()
                    .min_h(px(64.))
                    .overflow_y_scroll()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(skill.description.clone())
                    .test_support(),
            )
            .child(self.render_detail_controls(&skill, shared, togglable, inherited, cx))
            .into_any_element()
    }

    /// The fixed lower half of the detail pane: metadata, actions and agents.
    fn render_detail_controls(
        &self,
        skill: &Skill,
        shared: Vec<&'static Agent>,
        togglable: Vec<&'static Agent>,
        inherited: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = skill.name.clone();
        let scope = self.scope().clone();
        v_flex()
            .w_full()
            .flex_shrink_0()
            .gap_3()
            .child(Separator::horizontal())
            .child(self.render_meta(skill, cx))
            .when(!skill.cost.is_empty(), |this| {
                this.child(Separator::horizontal())
                    .child(render_cost(&skill.cost, cx))
            })
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
                        .child(
                            Button::new("diff-preview")
                                .small()
                                .label("Diff preview")
                                .disabled(skill.lock.is_none() || skill.content_path().is_none())
                                .on_click(cx.listener({
                                    let name = name.clone();
                                    move |this, _, _, cx| this.open_diff(&name, cx)
                                })),
                        )
                    })
                    .child(self.render_transfer_button(Transfer::Move, &name, cx))
                    .child(self.render_transfer_button(Transfer::Copy, &name, cx))
                    .child(
                        Button::new("remove")
                            .small()
                            .danger()
                            // Outline keeps the row's weight even: a thin
                            // border in a muted red, with the label in full
                            // danger red.
                            .outline()
                            .icon(Icon::empty().path("icons/trash.svg"))
                            .label("Remove")
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
            .when(!shared.is_empty(), |this| {
                this.child(self.render_shared_agents(&shared, &scope, cx))
            })
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
            .when(!inherited.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "Also loaded from another agent's directory: {inherited}"
                        )),
                )
            })
    }

    /// "Edit" opens `SKILL.md` with the system's handler for Markdown; the
    /// caret lists the editors that can open the whole folder instead.
    fn render_edit_button(&self, dir: &Path, cx: &mut Context<Self>) -> AnyElement {
        let skill_md = dir.join("SKILL.md");
        let edit = Button::new("edit")
            .small()
            .icon(Icon::empty().path("icons/square-pen.svg"))
            .label("Edit")
            .on_click(move |_, _, cx| cx.open_with_system(&skill_md));
        if self.editors.is_empty() {
            return edit.into_any_element();
        }
        let editors = self.editors.clone();
        let view = cx.entity().downgrade();
        let dir = dir.to_path_buf();
        DropdownButton::new("edit-in")
            .button(edit)
            .dropdown_menu(move |mut menu, _, _| {
                for editor in &editors {
                    let (view, editor, dir) = (view.clone(), editor.clone(), dir.clone());
                    menu = menu.item(
                        PopupMenuItem::new(format!("Open folder in {}", editor.name)).on_click(
                            move |_, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.open_in_editor(editor.clone(), dir.clone(), cx)
                                })
                                .ok();
                            },
                        ),
                    );
                }
                menu
            })
            .into_any_element()
    }

    /// "Move to" or "Copy to", opening a menu of every scope — global first,
    /// set apart from the projects as in the sidebar — and last, any folder.
    fn render_transfer_button(
        &self,
        transfer: Transfer,
        name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (id, label) = match transfer {
            Transfer::Move => ("move-to", "Move to"),
            Transfer::Copy => ("copy-to", "Copy to"),
        };
        let mut scopes: Vec<(usize, Scope)> = self.scopes.iter().cloned().enumerate().collect();
        scopes.sort_by_key(|(_, scope)| !scope.is_global());
        let current = self.active_scope;
        let view = cx.entity().downgrade();
        let name = name.to_string();

        Button::new(id)
            .small()
            .label(label)
            .dropdown_caret(true)
            .disabled(self.busy.is_some())
            .dropdown_menu(move |mut menu, _, _| {
                for (index, scope) in &scopes {
                    let (view, name, to) = (view.clone(), name.clone(), scope.clone());
                    menu = menu.item(
                        PopupMenuItem::new(scope.label())
                            .disabled(*index == current)
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.transfer_skill(transfer, name.clone(), to.clone(), cx)
                                })
                                .ok();
                            }),
                    );
                    if scope.is_global() {
                        menu = menu.separator();
                    }
                }
                let (view, name) = (view.clone(), name.clone());
                menu.separator()
                    .item(PopupMenuItem::new("Folder…").on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.pick_folder(transfer, name.clone(), cx))
                            .ok();
                    }))
            })
    }

    /// The agents reading the canonical directory directly, as one locked
    /// row: they lose the skill only on a full uninstall.
    fn render_shared_agents(
        &self,
        agents: &[&'static Agent],
        scope: &Scope,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (shown, rest) = split_shared_agents(agents);
        let why: SharedString = format!(
            "They read {} directly: remove the skill to take it away",
            paths::shorten(&scope.canonical_dir())
        )
        .into();
        h_flex()
            .w_full()
            .py_1()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("agent-shared-why")
                    .tooltip(move |window, cx| Tooltip::new(why.clone()).build(window, cx))
                    .child(Checkbox::new("agent-shared").checked(true).disabled(true)),
            )
            .child(div().text_sm().child(shown.join(", ")))
            .when(!rest.is_empty(), |this| {
                let list: SharedString = rest.join(", ").into();
                this.child(
                    div()
                        .id("agent-shared-more")
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("and {} more", rest.len()))
                        .tooltip(move |window, cx| Tooltip::new(list.clone()).build(window, cx))
                        .test_support(),
                )
            })
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
        let source = skill.source_label().to_string();
        let source = match skill.lock.as_ref().and_then(|l| l.repo_url()) {
            Some(url) => link("source-link", source, move |cx| cx.open_url(&url), cx),
            None => source.into_any_element(),
        };
        // Opening a directory with the system handler shows it in the file
        // manager.
        let location = match skill.content_path() {
            Some(path) => {
                let label = paths::shorten(path);
                let path = path.to_path_buf();
                link(
                    "location-link",
                    label,
                    move |cx| cx.open_with_system(&path),
                    cx,
                )
            }
            None => "missing".into_any_element(),
        };

        v_flex()
            .gap_1()
            .text_sm()
            .child(meta_row("Source", source, cx))
            .when_some(self.install_count(skill), |this, count| {
                this.child(meta_row(
                    "Installs",
                    format!("{} on skills.sh", registry::format_count(count)),
                    cx,
                ))
            })
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
            .text_xs()
            .when_some(self.busy.clone(), |this, label| {
                this.child(Spinner::new().small()).child(label)
            })
            .when(self.busy.is_none(), |this| {
                let (text, color) = match last {
                    Some(entry) if entry.failed => (entry.text.clone(), cx.theme().danger),
                    Some(entry) => (entry.text.clone(), cx.theme().muted_foreground),
                    None => ("Ready".into(), cx.theme().muted_foreground),
                };
                this.child(
                    div()
                        .id("status-text")
                        .text_color(color)
                        .aria_label(text.clone())
                        .child(text)
                        .test_support(),
                )
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

/// The logo and the two-tone lowercase name.
fn wordmark(cx: &App) -> impl IntoElement {
    let (base, accent) = (cx.theme().foreground, cx.theme().primary);
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .relative()
                .size(px(20.))
                .child(
                    svg()
                        .absolute()
                        .size_full()
                        .path(assets::LOGO_LEFT)
                        .text_color(base),
                )
                .child(
                    svg()
                        .absolute()
                        .size_full()
                        .path(assets::LOGO_RIGHT)
                        .text_color(accent),
                ),
        )
        .child(
            h_flex()
                .text_base()
                .font_semibold()
                .child(div().text_color(base).child("skill"))
                .child(div().text_color(accent).child("shard")),
        )
}

/// What a set of skills costs before anything is used: the front matter of
/// every skill at least one agent loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct AlwaysCost {
    tokens: u32,
    skills: usize,
    /// Project skills left out because a global skill has the same name.
    shadowed: usize,
}

/// Skills that are in some agent's system prompt. Skills parked as disabled,
/// and canonical copies no agent links to, cost nothing.
fn loaded(skills: &[Skill]) -> impl Iterator<Item = &Skill> {
    skills.iter().filter(|s| !s.installs.is_empty())
}

/// The always-on cost of one scope's skills.
fn always_cost(skills: &[Skill]) -> AlwaysCost {
    combined_always_cost(&[], skills)
}

/// The always-on cost of a project session: the project's skills plus the
/// global ones injected alongside them.
///
/// Agents know a skill by its front-matter name, so two copies under one name
/// are one skill in the prompt and are counted once. On a clash the global
/// copy is the one counted, following Claude Code's documented precedence
/// (personal over project).
fn combined_always_cost(global: &[Skill], project: &[Skill]) -> AlwaysCost {
    let mut seen = HashSet::new();
    let mut cost = AlwaysCost::default();
    for skill in loaded(global) {
        if seen.insert(skill.display_name()) {
            cost.tokens += skill.cost.description;
            cost.skills += 1;
        }
    }
    let global_names = seen.clone();
    for skill in loaded(project) {
        if global_names.contains(skill.display_name()) {
            cost.shadowed += 1;
        } else if seen.insert(skill.display_name()) {
            cost.tokens += skill.cost.description;
            cost.skills += 1;
        }
    }
    cost
}

/// A gauge icon, a budget-coloured token figure and a caption.
fn always_row(id: &'static str, tokens: u32, caption: &'static str, cx: &App) -> Stateful<Div> {
    h_flex()
        .id(id)
        .w_full()
        .gap_1()
        .items_center()
        .child(Icon::empty().path("icons/gauge.svg").xsmall())
        .child(
            div()
                .text_color(budget_color(tokens, tokens::SCOPE_ALWAYS_BUDGET, cx))
                .child(format!("≈ {}", registry::format_count(tokens as u64))),
        )
        .child(caption)
}

/// A left-aligned, clickable sidebar row. `Button` centres its content, so
/// sidebar entries are built from a plain row instead.
fn nav_row(id: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    h_flex()
        .id(id.into())
        .w_full()
        .h_9()
        .px_2()
        .gap_2()
        .items_center()
        .text_base()
        .rounded(cx.theme().radius)
        .hover(|this| this.bg(cx.theme().list_hover))
}

/// Small uppercase heading above a group of controls.
fn section_label(text: &str, cx: &App) -> impl IntoElement {
    div()
        .px_1()
        .pb_1()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_uppercase())
}

/// `1 agent`, `3 agents`.
/// Agents named first in the shared-directory row; the most widely used of
/// those that read `.agents/skills` directly.
const FEATURED_SHARED_AGENTS: &[&str] = &["codex", "opencode", "pi"];

/// Split the shared-directory agents into the names shown in the row and the
/// rest, which collapse into "and N more". Featured agents lead; others fill
/// any slots they leave.
fn split_shared_agents(agents: &[&'static Agent]) -> (Vec<&'static str>, Vec<&'static str>) {
    let featured = |a: &&&'static Agent| FEATURED_SHARED_AGENTS.contains(&a.key);
    let mut names: Vec<&'static str> = FEATURED_SHARED_AGENTS
        .iter()
        .filter_map(|key| agents.iter().find(|a| a.key == *key))
        .map(|a| a.display)
        .collect();
    names.extend(agents.iter().filter(|a| !featured(a)).map(|a| a.display));
    let rest = names.split_off(names.len().min(FEATURED_SHARED_AGENTS.len()));
    (names, rest)
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The same three figures as the detail pane, compacted to one line for the
/// list: always, on trigger, and bundled.
///
/// Only the first is tinted — it is the one being paid right now, whether or
/// not the skill is ever used.
fn row_cost(name: &str, cost: &TokenCost, cx: &App) -> impl IntoElement {
    let figure = |tokens: u32| {
        if tokens == 0 {
            "—".to_string()
        } else {
            registry::format_count(tokens as u64)
        }
    };
    let separator = |cx: &App| div().text_color(cx.theme().muted_foreground).child("·");

    h_flex()
        .id(SharedString::from(format!("cost-{name}")))
        .gap_1()
        .items_center()
        .aria_label(format!(
            "{} always, {} on trigger, {} bundled",
            figure(cost.description),
            figure(cost.body),
            figure(cost.resources),
        ))
        .child(Icon::empty().path("icons/gauge.svg").xsmall())
        .child(
            div()
                .text_color(budget_color(
                    cost.description,
                    tokens::DESCRIPTION_BUDGET,
                    cx,
                ))
                .child(figure(cost.description)),
        )
        .child(separator(cx))
        .child(figure(cost.body))
        .child(separator(cx))
        .child(figure(cost.resources))
        .tooltip(|window, cx| {
            Tooltip::new(
                "Estimated tokens: always in the system prompt · read when the skill triggers · \
                 bundled files, read only on demand.",
            )
            .build(window, cx)
        })
        .test_support()
}

/// What the skill costs an agent in context, split by when that cost is paid.
///
/// The three figures answer different questions, so they are never summed:
/// the description is charged to every session the skill is enabled in, the
/// body only once it triggers, and the bundled files only if the agent opens
/// them.
fn render_cost(cost: &TokenCost, cx: &App) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_1()
        .child(cost_row(
            "cost-always",
            "Always ≈",
            "Estimated tokens for the front-matter name and description, which sit in the \
             system prompt of every session this skill is enabled for — paid whether or not the \
             skill is ever used.",
            cost.description,
            tokens::DESCRIPTION_BUDGET,
            // The only figure that is spent unconditionally, so it is the one
            // worth warning about when it grows.
            budget_color(cost.description, tokens::DESCRIPTION_BUDGET, cx),
            cx,
        ))
        .child(cost_row(
            "cost-trigger",
            "Trigger ≈",
            "Estimated tokens for the body of SKILL.md, read once the skill activates.",
            cost.body,
            tokens::BODY_BUDGET,
            cx.theme().primary,
            cx,
        ))
        .child(cost_row(
            "cost-bundled",
            "Bundled ≈",
            "Estimated tokens in the other files bundled with the skill. Agents load these only \
             on demand, so this is a ceiling rather than a cost.",
            cost.resources,
            tokens::RESOURCES_BUDGET,
            cx.theme().muted_foreground,
            cx,
        ))
}

/// One labelled bar and its token count.
fn cost_row(
    id: &'static str,
    label: &str,
    tooltip: &'static str,
    tokens: u32,
    budget: u32,
    color: Hsla,
    cx: &App,
) -> impl IntoElement {
    let count = if tokens == 0 {
        "none".to_string()
    } else {
        registry::format_count(tokens as u64)
    };
    h_flex()
        .id(id)
        .w_full()
        .gap_2()
        .items_center()
        .text_sm()
        // The bar carries the shape and the figure carries the detail; screen
        // readers and the UI tests get both from here.
        .aria_label(format!("{label} {count} tokens"))
        .child(
            div()
                .w(px(76.))
                .flex_shrink_0()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            // Bars are scaled against a fixed budget rather than against the
            // other skills on screen, so a skill's bar does not move when the
            // ones around it change.
            Progress::new(SharedString::from(format!("{id}-bar")))
                .value((tokens as f32 / budget as f32 * 100.).min(100.))
                .color(color)
                .flex_1(),
        )
        .child(
            div()
                .w(px(40.))
                .flex_shrink_0()
                .text_right()
                .text_color(if tokens == 0 {
                    cx.theme().muted_foreground
                } else {
                    cx.theme().foreground
                })
                .child(if tokens == 0 {
                    "—".to_string()
                } else {
                    count
                }),
        )
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .test_support()
}

/// Green below half the budget, amber approaching it, red over.
fn budget_color(tokens: u32, budget: u32, cx: &App) -> Hsla {
    match tokens * 2 {
        t if t <= budget => cx.theme().success,
        _ if tokens <= budget => cx.theme().warning,
        _ => cx.theme().danger,
    }
}

/// One `label: value` line in the detail pane.
fn meta_row(label: &str, value: impl IntoElement, cx: &App) -> impl IntoElement {
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
        .child(div().flex_1().min_w_0().child(value))
}

/// Clickable text styled as a link.
fn link(
    id: &'static str,
    label: String,
    on_click: impl Fn(&mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let color = cx.theme().link;
    div()
        .id(id)
        .cursor_pointer()
        .text_color(color)
        .hover(move |this| this.text_color(color.opacity(0.8)).underline())
        .on_click(move |_, _, cx| on_click(cx))
        .child(label)
        .test_support()
        .into_any_element()
}

impl Skillshard {
    /// Asks whether a folder a skill was just sent to should become a project.
    fn render_add_project(&self, path: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| paths::shorten(path));
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
                    .id("add-project-dialog")
                    .w(px(420.))
                    .p_5()
                    .gap_4()
                    .rounded(cx.theme().radius_lg)
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .font_semibold()
                            .child(format!("Add {name} as a project?")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} is not in the sidebar yet. As a project, its skills can be \
                                 managed from here.",
                                paths::shorten(path)
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("dismiss-add-project")
                                    .small()
                                    .ghost()
                                    .label("Not now")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.offered_project = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("add-project")
                                    .small()
                                    .primary()
                                    .label("Add project")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(path) = this.offered_project.take() {
                                            this.add_project(path, cx);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .test_support(),
            )
    }
}

impl Render for Skillshard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Both side panes are always on screen at a fixed width, so what is
        // left over for the list is exact — no measurement pass needed.
        let list_width = f32::from(window.viewport_size().width) - 2. * SIDE_PANE_WIDTH;
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
            .children(self.settings.clone())
            .children(self.project_settings.clone())
            .children(self.diff.clone())
            .when_some(self.offered_project.clone(), |this, path| {
                this.child(self.render_add_project(&path, cx))
            })
    }
}

// ─── Dialog, projects and scope moves ───

impl Skillshard {
    fn open_diff(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(skill) = self.skills.iter().find(|skill| skill.name == name) else {
            return;
        };
        let (Some(entry), Some(installed)) = (skill.lock.clone(), skill.content_path()) else {
            return;
        };
        let dialog = cx.new(|cx| {
            super::diff::DiffDialog::new(
                skill.display_name().to_string(),
                entry,
                installed.to_path_buf(),
                cx,
            )
        });
        cx.subscribe(&dialog, |this, _, _, cx| {
            this.diff = None;
            cx.notify();
        })
        .detach();
        self.diff = Some(dialog);
        cx.notify();
    }

    /// Open the install dialog, pre-filled for the active scope.
    fn open_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let agents = self.shown_agents();
        let dialog = cx.new(|cx| super::install::InstallDialog::new(agents, window, cx));

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

    fn open_settings(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let mut agents = self.shown_agents();
        let extra = self.installed.iter().copied().chain(
            preferences::get(cx)
                .install_agents
                .iter()
                .filter_map(|key| crate::agents::by_key(key)),
        );
        for agent in extra {
            if !agents.iter().any(|a| a.key == agent.key) {
                agents.push(agent);
            }
        }
        // Default agents feed `skills add`, which rejects keys it does not know.
        agents.retain(|a| a.in_cli());
        agents.sort_by_key(|a| a.display);

        let dialog = cx.new(|_| super::settings::SettingsDialog::new(agents));
        cx.subscribe(&dialog, |this, _, event, cx| match event {
            super::settings::SettingsEvent::Close => {
                this.settings = None;
                cx.notify();
            }
        })
        .detach();
        self.settings = Some(dialog);
        cx.notify();
    }

    fn open_project_settings(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(Scope::Project(path)) = self.scopes.get(index).cloned() else {
            return;
        };
        let dialog = cx.new(|cx| super::project_settings::ProjectSettingsDialog::new(path, cx));
        cx.subscribe(&dialog, |this, _, event, cx| {
            match event {
                super::project_settings::ProjectSettingsEvent::Close => {}
                super::project_settings::ProjectSettingsEvent::Remove(path) => {
                    this.remove_project(path.clone(), cx)
                }
            }
            this.project_settings = None;
            cx.notify();
        })
        .detach();
        self.project_settings = Some(dialog);
        cx.notify();
    }

    /// Take a project out of the sidebar and preferences. Its files are left
    /// exactly as they are.
    fn remove_project(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        preferences::update(cx, |p| p.remove_project(&path));
        let scope = Scope::Project(path);
        let Some(index) = self.scopes.iter().position(|s| s == &scope) else {
            return;
        };
        self.scopes.remove(index);
        // Keep the same scope selected when an entry above it goes; fall back
        // to the first one when the selected project itself was removed.
        if self.active_scope == index {
            self.active_scope = 0;
            self.selected = None;
        } else if self.active_scope > index {
            self.active_scope -= 1;
        }
        self.log(format!("removed {} from the sidebar", scope.label()), false);
        self.reload(cx);
    }

    /// Run the install the dialog assembled.
    fn start_install(
        &mut self,
        mut request: crate::skills_cli::InstallRequest,
        cx: &mut Context<Self>,
    ) {
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
        self.run_steps(label, vec![(request.args(), cwd)], |_, _| {}, cx);
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
                this.active_scope = this.add_project(root, cx);
                this.selected = None;
                this.reload(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Put a project in the sidebar and preferences, returning its index.
    /// A project already there is left where it is.
    fn add_project(&mut self, root: PathBuf, cx: &mut Context<Self>) -> usize {
        preferences::update(cx, |p| {
            p.project_mut(&root);
        });
        let scope = Scope::Project(root);
        match self.scopes.iter().position(|s| s == &scope) {
            Some(index) => index,
            None => {
                self.scopes.push(scope);
                self.scopes.len() - 1
            }
        }
    }

    /// Ask for any folder, not only a project, and send the skill there.
    fn pick_folder(&mut self, transfer: Transfer, name: String, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                match transfer {
                    Transfer::Move => "Move here",
                    Transfer::Copy => "Copy here",
                }
                .into(),
            ),
        });

        cx.spawn(async move |this, cx| {
            let chosen = match paths.await {
                Ok(Ok(Some(chosen))) => chosen,
                // Cancelled.
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(e)) => {
                    this.update(cx, |this, cx| {
                        this.log(format!("could not choose a folder: {e}"), true);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let Some(root) = chosen.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.transfer_skill(transfer, name, Scope::Project(root), cx)
            })
            .ok();
        })
        .detach();
    }

    /// Ask whether `path` should join the sidebar as a project, unless it is
    /// there already.
    ///
    /// Raised once a skill has landed in a folder. Public so the UI tests can
    /// raise it without running the CLI.
    pub fn offer_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.scopes.contains(&Scope::Project(path.clone())) {
            return;
        }
        self.offered_project = Some(path);
        cx.notify();
    }

    /// Install a skill into another scope, and for a move remove it here.
    ///
    /// Reinstalling through the CLI (rather than moving files) is what keeps
    /// the lock files correct in both scopes, so update tracking survives the
    /// trip.
    fn transfer_skill(
        &mut self,
        transfer: Transfer,
        name: String,
        to: Scope,
        cx: &mut Context<Self>,
    ) {
        let Some(skill) = self.skills.iter().find(|s| s.name == name).cloned() else {
            return;
        };
        let from = self.scope().clone();
        if to == from {
            return;
        }

        let Some(source) = skill
            .lock
            .as_ref()
            .map(|l| l.source_url.clone().unwrap_or_else(|| l.source.clone()))
        else {
            // Without a lock entry there is no source to reinstall from.
            self.log(
                format!(
                    "{name} has no recorded source to {} it from",
                    transfer.verb()
                ),
                true,
            );
            cx.notify();
            return;
        };

        let agents: Vec<String> = skill
            .installs
            .iter()
            .filter(|i| i.kind.is_unlinkable() && i.agent.in_cli())
            .map(|i| i.agent.key.to_string())
            .collect();

        let install = crate::skills_cli::InstallRequest {
            source,
            scope: to.clone(),
            skills: vec![name.clone()],
            agents,
            copy: false,
        };
        let mut steps = vec![(install.args(), skills_cli::working_dir(&to))];
        if transfer == Transfer::Move {
            steps.push((
                skills_cli::remove_args(&name, &from, &[]),
                skills_cli::working_dir(&from),
            ));
        }
        self.run_steps(
            format!("{} {name} to {}", transfer.verb(), to.label()),
            steps,
            move |this, cx| {
                if let Scope::Project(path) = to {
                    this.offer_project(path, cx);
                }
            },
            cx,
        );
    }

    /// Run CLI commands in order, stopping at the first failure, then call
    /// `on_success` if every one succeeded.
    fn run_steps(
        &mut self,
        label: String,
        steps: Vec<(Vec<String>, std::path::PathBuf)>,
        on_success: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
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
                    last.unwrap_or_else(|| Err(std::io::Error::other("nothing to run")))
                })
                .await;

            this.update(cx, |this, cx| {
                this.busy = None;
                match outcome {
                    Ok(outcome) if outcome.success => {
                        this.log(format!("{label}: done"), false);
                        on_success(this, cx);
                    }
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

#[cfg(test)]
mod tests {
    // Not `super::*`: the kit's glob export includes GPUI's `test` macro when
    // test support is on, which would shadow `#[test]`.
    use super::{always_cost, combined_always_cost, split_shared_agents, AlwaysCost};
    use crate::agents::by_key;
    use crate::model::{AgentInstall, InstallKind, Scope, Skill, UpdateState};
    use crate::tokens::TokenCost;

    /// A skill whose always-on cost is `tokens`, loaded by Claude Code unless
    /// `loaded` is false.
    fn skill(name: &str, tokens: u32, loaded: bool, scope: Scope) -> Skill {
        let installs = if loaded {
            vec![AgentInstall {
                agent: by_key("claude-code").unwrap(),
                path: name.into(),
                kind: InstallKind::Copy,
            }]
        } else {
            Vec::new()
        };
        Skill {
            name: name.into(),
            title: None,
            description: String::new(),
            scope,
            canonical: None,
            installs,
            lock: None,
            disabled: false,
            update: UpdateState::Unknown,
            cost: TokenCost {
                description: tokens,
                ..TokenCost::default()
            },
        }
    }

    fn global(name: &str, tokens: u32) -> Skill {
        skill(name, tokens, true, Scope::Global)
    }

    fn project(name: &str, tokens: u32) -> Skill {
        skill(name, tokens, true, Scope::Project("/p".into()))
    }

    #[test]
    fn the_total_adds_global_skills_to_the_project_s() {
        let cost = combined_always_cost(&[global("a", 10)], &[project("b", 20)]);
        assert_eq!(
            cost,
            AlwaysCost {
                tokens: 30,
                skills: 2,
                shadowed: 0
            }
        );
    }

    #[test]
    fn a_name_in_both_scopes_is_counted_once_at_the_global_cost() {
        let cost = combined_always_cost(
            &[global("shared", 10), global("a", 5)],
            &[project("shared", 99), project("b", 20)],
        );
        assert_eq!(
            cost,
            AlwaysCost {
                tokens: 35,
                skills: 3,
                shadowed: 1
            }
        );
    }

    #[test]
    fn the_front_matter_name_decides_a_clash_not_the_directory() {
        let mut renamed = project("dir-name", 99);
        renamed.title = Some("shared".into());
        let cost = combined_always_cost(&[global("shared", 10)], &[renamed]);
        assert_eq!((cost.tokens, cost.shadowed), (10, 1));
    }

    #[test]
    fn a_global_skill_no_agent_loads_neither_counts_nor_shadows() {
        let unloaded = skill("shared", 10, false, Scope::Global);
        let cost = combined_always_cost(&[unloaded], &[project("shared", 20)]);
        assert_eq!(
            cost,
            AlwaysCost {
                tokens: 20,
                skills: 1,
                shadowed: 0
            }
        );
    }

    #[test]
    fn one_scope_alone_counts_only_what_an_agent_loads() {
        let unloaded = skill("off", 50, false, Scope::Global);
        assert_eq!(always_cost(&[global("on", 7), unloaded]).tokens, 7);
    }

    fn split(keys: &[&str]) -> (Vec<&'static str>, Vec<&'static str>) {
        let agents: Vec<_> = keys.iter().map(|k| by_key(k).unwrap()).collect();
        split_shared_agents(&agents)
    }

    #[test]
    fn the_popular_agents_lead_and_the_rest_are_counted() {
        let (shown, rest) = split(&["cline", "codex", "dexto", "opencode", "pi", "zed"]);
        assert_eq!(shown, ["Codex", "OpenCode", "Pi"]);
        assert_eq!(rest, ["Cline", "Dexto", "Zed"]);
    }

    #[test]
    fn other_agents_fill_in_when_the_popular_ones_are_missing() {
        let (shown, rest) = split(&["cline", "codex", "dexto", "zed"]);
        assert_eq!(shown, ["Codex", "Cline", "Dexto"]);
        assert_eq!(rest, ["Zed"]);
    }
}
