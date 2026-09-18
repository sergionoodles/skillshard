//! The install dialog.
//!
//! Every choice the `skills` CLI would ask for interactively — source, which
//! skills, which agents, global or project, symlink or copy — is made here
//! first, so the command runs start to finish without a terminal prompt. The
//! partner security review for the chosen source is fetched and shown before
//! the install button does anything.

use crate::agents::Agent;
use crate::local_repos::{self, LocalSkill};
use crate::model::Scope;
use crate::preferences;
use crate::registry::{self, Risk, SearchHit, SkillAudit};
use crate::skills_cli::InstallRequest;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::separator::Separator;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::BTreeMap;

/// What the dialog reports back to the main window.
pub enum InstallEvent {
    /// Run this install.
    Submit(InstallRequest),
    /// Close without installing.
    Cancel,
}

impl EventEmitter<InstallEvent> for InstallDialog {}

/// Progress of a background lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Lookup<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

pub struct InstallDialog {
    source: Entity<InputState>,
    skills: Entity<InputState>,
    query: Entity<InputState>,
    /// Agents offered, with their selection state.
    agents: Vec<(&'static Agent, bool)>,
    global: bool,
    copy: bool,
    results: Lookup<Vec<SearchHit>>,
    /// Skills found in the configured local repositories, and any roots that
    /// could not be read.
    local: Lookup<(Vec<LocalSkill>, Vec<String>)>,
    audit: Lookup<BTreeMap<String, SkillAudit>>,
    /// Set once the user has seen a review for the current source.
    reviewed_source: Option<String>,
}

impl InstallDialog {
    /// `agents` are those already in use; the rest of the defaults come from
    /// preferences.
    pub fn new(agents: Vec<&'static Agent>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let source = cx.new(|cx| {
            InputState::new(window, cx).placeholder("owner/repo, a URL, or a local path")
        });
        let skills = cx.new(|cx| {
            InputState::new(window, cx).placeholder("all skills (or a comma-separated list)")
        });
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search skills…"));
        // Local results filter as you type; remote ones wait for "Search".
        cx.observe(&query, |_, _, cx| cx.notify()).detach();

        let prefs = preferences::get(cx).clone();
        let roots = prefs.local_repositories.clone();
        let has_roots = !roots.is_empty();
        if has_roots {
            // A repository can be a large tree; walk it off the UI thread.
            cx.spawn(async move |this, cx| {
                let found = cx
                    .background_executor()
                    .spawn(async move { local_repos::discover(&roots) })
                    .await;
                this.update(cx, |this, cx| {
                    this.local = Lookup::Ready(found);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }

        Self {
            source,
            skills,
            query,
            agents: default_agents(agents, &prefs.install_agents),
            global: prefs.install_global,
            copy: prefs.install_copy,
            results: Lookup::Idle,
            local: if has_roots {
                Lookup::Loading
            } else {
                Lookup::Idle
            },
            audit: Lookup::Idle,
            reviewed_source: None,
        }
    }

    fn source_value(&self, cx: &App) -> String {
        self.source.read(cx).value().trim().to_string()
    }

    /// Skill names typed into the dialog; empty means every skill in the source.
    fn skill_names(&self, cx: &App) -> Vec<String> {
        self.skills
            .read(cx)
            .value()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn selected_agents(&self) -> Vec<String> {
        self.agents
            .iter()
            .filter(|(_, on)| *on)
            .map(|(a, _)| a.key.to_string())
            .collect()
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().trim().to_string();
        if query.is_empty() {
            return;
        }
        self.results = Lookup::Loading;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { registry::search(&query, 20) })
                .await;
            this.update(cx, |this, cx| {
                this.results = match found {
                    Ok(hits) => Lookup::Ready(hits),
                    Err(e) => Lookup::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetch the partner security review for the chosen source and skills.
    fn review(&mut self, cx: &mut Context<Self>) {
        let source = self.source_value(cx);
        if source.is_empty() {
            return;
        }
        if std::path::Path::new(&source).exists() {
            self.audit = Lookup::Failed(
                "local skills are not audited by skills.sh — read them before installing".into(),
            );
            cx.notify();
            return;
        }
        let names = self.skill_names(cx);
        self.audit = Lookup::Loading;
        self.reviewed_source = Some(source.clone());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { registry::audit(&source, &names) })
                .await;
            this.update(cx, |this, cx| {
                this.audit = match result {
                    Ok(map) => Lookup::Ready(map),
                    Err(e) => Lookup::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let source = self.source_value(cx);
        if source.is_empty() {
            return;
        }
        cx.emit(InstallEvent::Submit(InstallRequest {
            source,
            scope: if self.global {
                Scope::Global
            } else {
                // The main window substitutes the active project.
                Scope::Project(std::path::PathBuf::new())
            },
            skills: self.skill_names(cx),
            agents: self.selected_agents(),
            copy: self.copy,
        }));
    }
}

/// Agents offered in the dialog, and which start ticked.
///
/// With no preferred agents every agent already in use is ticked, which is
/// almost always what the user wants and avoids creating stray directories.
/// Preferred agents are offered even when not yet in use.
fn default_agents(
    in_use: Vec<&'static Agent>,
    preferred: &[String],
) -> Vec<(&'static Agent, bool)> {
    let mut offered = in_use;
    for agent in preferred
        .iter()
        .filter_map(|key| crate::agents::by_key(key))
    {
        if !offered.iter().any(|a| a.key == agent.key) {
            offered.push(agent);
        }
    }
    offered.sort_by_key(|a| a.display);
    offered
        .into_iter()
        .map(|agent| {
            let ticked = preferred.is_empty() || preferred.iter().any(|k| k == agent.key);
            (agent, ticked)
        })
        .collect()
}

/// Local skills whose name or description contains `query`.
fn matching_local<'a>(skills: &'a [LocalSkill], query: &str) -> Vec<&'a LocalSkill> {
    let query = query.trim().to_lowercase();
    skills
        .iter()
        .filter(|s| {
            query.is_empty()
                || s.name.to_lowercase().contains(&query)
                || s.description.to_lowercase().contains(&query)
        })
        .collect()
}

/// Colour a risk verdict.
fn risk_tag(risk: Risk) -> Tag {
    match risk {
        Risk::Safe => Tag::success(),
        Risk::Low => Tag::info(),
        Risk::Medium => Tag::warning(),
        Risk::High | Risk::Critical => Tag::danger(),
        Risk::Unknown => Tag::secondary(),
    }
}

impl InstallDialog {
    /// Fill the form from a picked result.
    fn pick(&mut self, source: String, skill: String, window: &mut Window, cx: &mut Context<Self>) {
        self.source
            .update(cx, |state, cx| state.set_value(source, window, cx));
        self.skills
            .update(cx, |state, cx| state.set_value(skill, window, cx));
        self.audit = Lookup::Idle;
        self.reviewed_source = None;
        cx.notify();
    }

    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.query).id("install-query").small().flex_1())
                    .child(
                        Button::new("do-search")
                            .small()
                            .label("Search skills.sh")
                            .on_click(cx.listener(|this, _, _, cx| this.search(cx))),
                    ),
            )
            .when(!matches!(self.local, Lookup::Idle), |this| {
                this.child(self.render_local(cx))
            })
            .child(self.render_remote(cx))
    }

    fn render_local(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query.read(cx).value().to_string();
        v_flex()
            .id("local-results")
            .gap_1()
            .child(result_heading("Local repositories", cx))
            .child(match &self.local {
                Lookup::Idle => div().into_any_element(),
                Lookup::Loading => loading("Looking through folders…"),
                Lookup::Failed(e) => div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(e.clone())
                    .into_any_element(),
                Lookup::Ready((skills, errors)) => {
                    let matches = matching_local(skills, &query);
                    v_flex()
                        .id("local-list")
                        .max_h(px(140.))
                        .overflow_y_scroll()
                        .children(errors.iter().map(|e| {
                            div()
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(e.clone())
                        }))
                        .when(matches.is_empty() && errors.is_empty(), |this| {
                            this.child(empty_note("No local skills match.", cx))
                        })
                        .children(
                            matches
                                .into_iter()
                                .enumerate()
                                .map(|(index, skill)| {
                                    let path = skill.path.display().to_string();
                                    result_row(
                                        SharedString::from(format!("local-{index}")),
                                        skill.name.clone(),
                                        crate::paths::shorten(&skill.path),
                                        None,
                                        cx,
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        // A skill directory installs as a whole.
                                        this.pick(path.clone(), String::new(), window, cx)
                                    }))
                                    .test_support()
                                })
                                .collect::<Vec<_>>(),
                        )
                        .into_any_element()
                }
            })
    }

    fn render_remote(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(result_heading("skills.sh", cx))
            .child(match &self.results {
                Lookup::Idle => empty_note("Search the public directory.", cx).into_any_element(),
                Lookup::Loading => loading("Searching…"),
                Lookup::Failed(e) => div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(e.clone())
                    .into_any_element(),
                Lookup::Ready(hits) if hits.is_empty() => {
                    empty_note("Nothing found.", cx).into_any_element()
                }
                Lookup::Ready(hits) => v_flex()
                    .id("results")
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .children(
                        hits.iter()
                            .enumerate()
                            .map(|(index, hit)| {
                                let (source, name) = (hit.source.clone(), hit.name.clone());
                                result_row(
                                    SharedString::from(format!("hit-{index}")),
                                    hit.name.clone(),
                                    hit.source.clone(),
                                    Some(hit.installs),
                                    cx,
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.pick(source.clone(), name.clone(), window, cx)
                                }))
                                .test_support()
                            })
                            .collect::<Vec<_>>(),
                    )
                    .into_any_element(),
            })
    }

    fn render_review(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().font_medium().child("Security review"))
                    .child(
                        Button::new("run-review")
                            .small()
                            .label("Review source")
                            .on_click(cx.listener(|this, _, _, cx| this.review(cx))),
                    ),
            )
            .child(match &self.audit {
                Lookup::Idle => div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Not reviewed yet.")
                    .into_any_element(),
                Lookup::Loading => h_flex()
                    .gap_2()
                    .child(Spinner::new().small())
                    .child("Fetching partner audits…")
                    .into_any_element(),
                Lookup::Failed(e) => div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(format!("Review unavailable: {e}"))
                    .into_any_element(),
                Lookup::Ready(map) if map.is_empty() => div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No audit data published for this source.")
                    .into_any_element(),
                Lookup::Ready(map) => v_flex()
                    .gap_1()
                    .children(
                        map.iter()
                            .map(|(skill, audit)| {
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .items_center()
                                    .child(div().flex_1().text_sm().child(skill.clone()))
                                    .child(
                                        risk_tag(audit.worst())
                                            .small()
                                            .child(audit.worst().label()),
                                    )
                                    .children(audit.partners.iter().map(|(partner, risk)| {
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{partner}: {}", risk.label()))
                                    }))
                            })
                            .collect::<Vec<_>>(),
                    )
                    .into_any_element(),
            })
    }
}

impl Render for InstallDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source_set = !self.source_value(cx).is_empty();
        let concerning = matches!(&self.audit, Lookup::Ready(map)
            if map.values().any(|a| a.is_concerning()));

        // A full-window scrim; the dialog itself sits on top of it.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::hsla(0., 0., 0., 0.45))
            .child(
                v_flex()
                    .id("install-dialog")
                    .w(px(620.))
                    .max_h(px(640.))
                    .p_5()
                    .gap_4()
                    .overflow_y_scroll()
                    .rounded(cx.theme().radius_lg)
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(div().text_lg().font_semibold().child("Install a skill"))
                    .child(self.render_search(cx))
                    .child(Separator::horizontal())
                    .child(labelled(
                        "Source",
                        Input::new(&self.source).id("source"),
                        cx,
                    ))
                    .child(labelled(
                        "Skills",
                        Input::new(&self.skills).id("skills"),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap_4()
                            .child(
                                Switch::new("scope")
                                    .checked(self.global)
                                    .label("Install globally")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.global = *checked;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Switch::new("copy")
                                    .checked(self.copy)
                                    .label("Copy instead of symlink")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.copy = *checked;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().font_medium().child("Agents"))
                            .child(
                                h_flex().gap_3().flex_wrap().children(
                                    self.agents
                                        .iter()
                                        .enumerate()
                                        .map(|(index, (agent, on))| {
                                            Checkbox::new(("agent", index))
                                                .checked(*on)
                                                .label(agent.display)
                                                .on_click(cx.listener(
                                                    move |this, checked: &bool, _, cx| {
                                                        this.agents[index].1 = *checked;
                                                        cx.notify();
                                                    },
                                                ))
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                            ),
                    )
                    .child(Separator::horizontal())
                    .child(self.render_review(cx))
                    .when(concerning, |this| {
                        this.child(
                            div()
                                .p_2()
                                .rounded(cx.theme().radius)
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(
                                    "At least one partner flagged this source. \
                                     Skills run with your agent's full permissions — \
                                     read the code before installing.",
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("cancel").label("Cancel").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(InstallEvent::Cancel)),
                                ),
                            )
                            .child(
                                Button::new("confirm")
                                    .primary()
                                    .label("Install")
                                    .disabled(!source_set)
                                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                            ),
                    ),
            )
    }
}

/// A clickable search result: name, where it comes from, and its installs.
fn result_row(
    id: SharedString,
    name: String,
    origin: String,
    installs: Option<u64>,
    cx: &App,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .w_full()
        .px_2()
        .py_1()
        .gap_3()
        .items_center()
        .rounded(cx.theme().radius)
        .hover(|s| s.bg(cx.theme().list_hover))
        .child(div().flex_1().min_w_0().text_sm().truncate().child(name))
        .child(
            div()
                .max_w(px(220.))
                .text_xs()
                .truncate()
                .text_color(cx.theme().muted_foreground)
                .child(origin),
        )
        .when_some(installs, |this, count| {
            this.child(
                h_flex()
                    .w(px(56.))
                    .justify_end()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(Icon::empty().path("icons/download.svg").xsmall())
                    .child(registry::format_count(count)),
            )
        })
}

fn result_heading(text: &str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_uppercase())
}

fn empty_note(text: &str, cx: &App) -> impl IntoElement {
    div()
        .px_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

fn loading(text: &'static str) -> AnyElement {
    h_flex()
        .px_2()
        .gap_2()
        .text_xs()
        .child(Spinner::new().xsmall())
        .child(text)
        .into_any_element()
}

/// A labelled form row.
fn labelled(label: &str, control: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(control)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the kit's glob export includes GPUI's `test` macro when
    // test support is on, which would shadow `#[test]`.
    use super::{default_agents, matching_local};
    use crate::agents::{by_key, Agent};
    use crate::local_repos::LocalSkill;
    use std::path::PathBuf;

    fn keys(selection: &[(&'static Agent, bool)]) -> Vec<(&'static str, bool)> {
        selection.iter().map(|(a, on)| (a.key, *on)).collect()
    }

    #[test]
    fn with_no_preference_every_agent_in_use_is_ticked() {
        let in_use = vec![by_key("claude-code").unwrap(), by_key("codex").unwrap()];
        assert_eq!(
            keys(&default_agents(in_use, &[])),
            [("claude-code", true), ("codex", true)]
        );
    }

    #[test]
    fn preferred_agents_are_ticked_and_offered_even_if_not_in_use() {
        let in_use = vec![by_key("claude-code").unwrap(), by_key("codex").unwrap()];
        let preferred = ["codex".to_string(), "windsurf".to_string()];
        assert_eq!(
            keys(&default_agents(in_use, &preferred)),
            [("claude-code", false), ("codex", true), ("windsurf", true)]
        );
    }

    #[test]
    fn local_matches_name_or_description_ignoring_case() {
        let skill = |name: &str, description: &str| LocalSkill {
            name: name.into(),
            description: description.into(),
            path: PathBuf::from(name),
            repository: PathBuf::from("/repo"),
        };
        let skills = [
            skill("pdf-tools", "Split PDFs"),
            skill("git-helper", "Commit messages"),
        ];
        let names = |q: &str| -> Vec<String> {
            matching_local(&skills, q)
                .iter()
                .map(|s| s.name.clone())
                .collect()
        };
        assert_eq!(names(""), ["pdf-tools", "git-helper"]);
        assert_eq!(names("PDF"), ["pdf-tools"]);
        assert_eq!(names("commit"), ["git-helper"]);
        assert!(names("nothing").is_empty());
    }
}
