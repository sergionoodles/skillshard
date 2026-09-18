//! The settings modal: preferences grouped by type, one page per group.
//!
//! Every control reads and writes the preferences global directly; the main
//! window observes that global and applies changes (theme and so on) as they
//! happen, so there is no save button.

use crate::agents::Agent;
use crate::paths;
use crate::preferences::{self, Appearance, Preferences};
use crate::themes;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

pub enum SettingsEvent {
    Close,
}

impl EventEmitter<SettingsEvent> for SettingsDialog {}

pub struct SettingsDialog {
    /// Agents offered as install defaults: those on this machine, plus any
    /// already chosen.
    agents: Vec<&'static Agent>,
}

impl SettingsDialog {
    pub fn new(agents: Vec<&'static Agent>) -> Self {
        Self { agents }
    }
}

/// A dropdown bound to a string preference.
fn dropdown(
    options: &[(&str, &str)],
    get: fn(&Preferences) -> String,
    set: fn(&mut Preferences, String),
) -> SettingField<SharedString> {
    SettingField::dropdown(
        options
            .iter()
            .map(|(value, label)| (SharedString::from(*value), SharedString::from(*label)))
            .collect(),
        move |cx| get(preferences::get(cx)).into(),
        move |value, cx| preferences::update(cx, |p| set(p, value.to_string())),
    )
}

/// A switch bound to a boolean preference.
fn switch(get: fn(&Preferences) -> bool, set: fn(&mut Preferences, bool)) -> SettingField<bool> {
    SettingField::switch(
        move |cx| get(preferences::get(cx)),
        move |value, cx| preferences::update(cx, |p| set(p, value)),
    )
}

/// Theme names as dropdown options, value and label alike.
fn theme_options(names: &[&'static str]) -> Vec<(&'static str, &'static str)> {
    names.iter().map(|name| (*name, *name)).collect()
}

fn appearance_page() -> SettingPage {
    SettingPage::new("Appearance")
        .icon(Icon::empty().path("icons/palette.svg"))
        .group(
            SettingGroup::new().title("Theme").items([
                SettingItem::new(
                    "Mode",
                    dropdown(
                        &[
                            ("system", "Match system"),
                            ("light", "Light"),
                            ("dark", "Dark"),
                        ],
                        |p| {
                            match p.appearance {
                                Appearance::System => "system",
                                Appearance::Light => "light",
                                Appearance::Dark => "dark",
                            }
                            .into()
                        },
                        |p, value| {
                            p.appearance = match value.as_str() {
                                "light" => Appearance::Light,
                                "dark" => Appearance::Dark,
                                _ => Appearance::System,
                            }
                        },
                    ),
                )
                .description("With “Match system”, the theme follows your desktop."),
                SettingItem::new(
                    "Light theme",
                    dropdown(
                        &theme_options(themes::LIGHT),
                        |p| p.light_theme.clone(),
                        |p, value| p.light_theme = value,
                    ),
                ),
                SettingItem::new(
                    "Dark theme",
                    dropdown(
                        &theme_options(themes::DARK),
                        |p| p.dark_theme.clone(),
                        |p, value| p.dark_theme = value,
                    ),
                ),
            ]),
        )
}

fn installing_page(agents: Vec<&'static Agent>) -> SettingPage {
    SettingPage::new("Installing")
        .icon(Icon::empty().path("icons/download.svg"))
        .group(
            SettingGroup::new()
                .title("Defaults")
                .description("Pre-filled in the install dialog; you can still change them there.")
                .items([
                    SettingItem::new(
                        "Install globally",
                        switch(|p| p.install_global, |p, v| p.install_global = v),
                    )
                    .description("Off installs into the open project instead."),
                    SettingItem::new(
                        "Copy instead of symlinking",
                        switch(|p| p.install_copy, |p, v| p.install_copy = v),
                    )
                    .description("Symlinks keep one shared copy that every agent sees."),
                    SettingItem::render(move |_, _, cx| render_default_agents(&agents, cx)),
                ]),
        )
}

fn render_default_agents(agents: &[&'static Agent], cx: &App) -> impl IntoElement {
    let chosen = &preferences::get(cx).install_agents;
    v_flex()
        .gap_2()
        .child(div().text_sm().font_medium().child("Default agents"))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("With none selected, every agent already in use is chosen."),
        )
        .child(
            h_flex().flex_wrap().gap_x_4().gap_y_2().children(
                agents
                    .iter()
                    .map(|agent| {
                        let key = agent.key;
                        Checkbox::new(SharedString::from(format!("default-agent-{key}")))
                            .small()
                            .label(agent.display)
                            .checked(chosen.iter().any(|k| k == key))
                            .on_click(move |checked, _, cx| {
                                preferences::update(cx, |p| {
                                    p.install_agents.retain(|k| k != key);
                                    if *checked {
                                        p.install_agents.push(key.to_string());
                                    }
                                });
                            })
                    })
                    .collect::<Vec<_>>(),
            ),
        )
}

fn sources_page() -> SettingPage {
    SettingPage::new("Sources")
        .icon(Icon::empty().path("icons/folder-git-2.svg"))
        .group(
            SettingGroup::new()
                .title("Local repositories")
                .description(
                    "Folders searched for skills on this machine. They are listed \
                     separately from skills.sh when installing.",
                )
                .item(SettingItem::render(|_, _, cx| {
                    render_local_repositories(cx)
                })),
        )
}

fn render_local_repositories(cx: &App) -> impl IntoElement {
    let repositories = preferences::get(cx).local_repositories.clone();
    v_flex()
        .gap_1()
        .when(repositories.is_empty(), |this| {
            this.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No local repositories yet."),
            )
        })
        .children(
            repositories
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    let path = path.clone();
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::Folder)
                                .small()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .truncate()
                                .child(paths::shorten(&path)),
                        )
                        .child(
                            Button::new(("remove-repository", index))
                                .xsmall()
                                .ghost()
                                .icon(Icon::empty().path("icons/x.svg"))
                                .tooltip("Remove")
                                .on_click(move |_, _, cx| {
                                    preferences::update(cx, |p| {
                                        p.local_repositories.retain(|r| r != &path)
                                    })
                                }),
                        )
                })
                .collect::<Vec<_>>(),
        )
        .child(
            h_flex().pt_1().child(
                Button::new("add-repository")
                    .small()
                    .icon(Icon::empty().path("icons/folder-plus.svg"))
                    .label("Add folder…")
                    .on_click(|_, _, cx| add_repository(cx)),
            ),
        )
}

/// Ask for a folder and add it to the local repositories.
fn add_repository(cx: &mut App) {
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: true,
        prompt: Some("Add local repository".into()),
    });
    cx.spawn(async move |cx| {
        let result = chosen.await;
        cx.update(|cx| match result {
            Ok(Ok(Some(folders))) => preferences::update(cx, |p| add_folders(p, folders)),
            // Cancelled.
            Ok(Ok(None)) => {}
            Ok(Err(e)) => {
                cx.global_mut::<preferences::Store>().error =
                    Some(format!("could not open the folder picker: {e}"))
            }
            Err(_) => {
                cx.global_mut::<preferences::Store>().error =
                    Some("the folder picker closed unexpectedly".into())
            }
        })
    })
    .detach();
}

fn add_folders(prefs: &mut Preferences, folders: Vec<PathBuf>) {
    for folder in folders {
        if !prefs.local_repositories.contains(&folder) {
            prefs.local_repositories.push(folder);
        }
    }
}

fn updates_page() -> SettingPage {
    SettingPage::new("Updates")
        .icon(Icon::empty().path("icons/refresh-cw.svg"))
        .group(
            SettingGroup::new().title("Checking").item(
                SettingItem::new(
                    "Check for updates on startup",
                    switch(
                        |p| p.check_updates_on_startup,
                        |p, v| p.check_updates_on_startup = v,
                    ),
                )
                .description("Compares each tracked skill with its GitHub source."),
            ),
        )
}

impl Render for SettingsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A full-window scrim; the dialog sits on top of it.
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
                    .id("settings-dialog")
                    .w(px(820.))
                    .h(px(560.))
                    .overflow_hidden()
                    .rounded(cx.theme().radius_lg)
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .px_4()
                            .py_2()
                            .items_center()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(div().flex_1().font_semibold().child("Settings"))
                            .child(
                                Button::new("close-settings")
                                    .small()
                                    .ghost()
                                    .icon(Icon::empty().path("icons/x.svg"))
                                    .tooltip("Close")
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)),
                                    ),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(
                        Settings::new("preferences").sidebar_width(px(200.)).pages([
                            appearance_page(),
                            installing_page(self.agents.clone()),
                            sources_page(),
                            updates_page(),
                        ]),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: see the note in `ui::install`'s tests.
    use super::add_folders;
    use crate::preferences::Preferences;
    use std::path::PathBuf;

    #[test]
    fn adding_folders_skips_ones_already_listed() {
        let mut prefs = Preferences {
            local_repositories: vec![PathBuf::from("/a")],
            ..Preferences::default()
        };
        add_folders(&mut prefs, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(
            prefs.local_repositories,
            [PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }
}
