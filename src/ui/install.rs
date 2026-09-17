//! The install dialog.
//!
//! Every choice the `skills` CLI would ask for interactively — source, which
//! skills, which agents, global or project, symlink or copy — is made here
//! first, so the command runs start to finish without a terminal prompt. The
//! partner security review for the chosen source is fetched and shown before
//! the install button does anything.

use crate::agents::Agent;
use crate::model::Scope;
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
    audit: Lookup<BTreeMap<String, SkillAudit>>,
    /// Set once the user has seen a review for the current source.
    reviewed_source: Option<String>,
}

impl InstallDialog {
    pub fn new(
        scope: &Scope,
        agents: Vec<&'static Agent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = cx.new(|cx| {
            InputState::new(window, cx).placeholder("owner/repo, a URL, or a local path")
        });
        let skills = cx.new(|cx| {
            InputState::new(window, cx).placeholder("all skills (or a comma-separated list)")
        });
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search skills.sh…"));

        Self {
            source,
            skills,
            query,
            // Default to the agents already in use, which is almost always
            // what the user wants and avoids creating stray directories.
            agents: agents.into_iter().map(|a| (a, true)).collect(),
            global: scope.is_global(),
            copy: false,
            results: Lookup::Idle,
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
    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.query).id("install-query").flex_1())
                    .child(
                        Button::new("do-search")
                            .label("Search")
                            .on_click(cx.listener(|this, _, _, cx| this.search(cx))),
                    ),
            )
            .child(match &self.results {
                Lookup::Idle => div().into_any_element(),
                Lookup::Loading => h_flex()
                    .gap_2()
                    .child(Spinner::new().small())
                    .child("Searching…")
                    .into_any_element(),
                Lookup::Failed(e) => div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(e.clone())
                    .into_any_element(),
                Lookup::Ready(hits) => v_flex()
                    .id("results")
                    .max_h(px(160.))
                    .overflow_y_scroll()
                    .children(
                        hits.iter()
                            .enumerate()
                            .map(|(index, hit)| {
                                let source = hit.source.clone();
                                let name = hit.name.clone();
                                h_flex()
                                    .id(("hit", index))
                                    .w_full()
                                    .px_2()
                                    .py_1()
                                    .gap_2()
                                    .items_center()
                                    .rounded(cx.theme().radius)
                                    .hover(|s| s.bg(cx.theme().list_hover))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        // Picking a result fills in both the
                                        // source and the single skill to install.
                                        this.source.update(cx, |state, cx| {
                                            state.set_value(source.clone(), window, cx)
                                        });
                                        this.skills.update(cx, |state, cx| {
                                            state.set_value(name.clone(), window, cx)
                                        });
                                        this.audit = Lookup::Idle;
                                        this.reviewed_source = None;
                                        cx.notify();
                                    }))
                                    .child(div().flex_1().text_sm().child(hit.name.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(hit.source.clone()),
                                    )
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
                    .child(labelled("Source", Input::new(&self.source).id("source"), cx))
                    .child(labelled("Skills", Input::new(&self.skills).id("skills"), cx))
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
                                Button::new("cancel").label("Cancel").on_click(cx.listener(
                                    |_, _, _, cx| cx.emit(InstallEvent::Cancel),
                                )),
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
