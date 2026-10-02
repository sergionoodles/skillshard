//! Local usage tracking controls and rebuild confirmation inside Settings.

use super::{dropdown, switch, SettingsDialog, SettingsEvent};
use crate::preferences::{self, TrackingDays};
use crate::usage::ServiceState;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

impl SettingsDialog {
    fn request_rebuild(&mut self, resume: bool, cx: &mut Context<Self>) {
        if rebuild_is_busy(self.usage_status.state) {
            return;
        }
        self.confirming_rebuild = false;
        self.usage_status.state = ServiceState::Rebuilding;
        cx.emit(if resume {
            SettingsEvent::ResumeRebuild
        } else {
            SettingsEvent::RebuildHistory
        });
        cx.notify();
    }

    pub(super) fn tracking_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let view = cx.entity().downgrade();
        SettingPage::new("Tracking")
            .icon(Icon::empty().path("icons/chart-line.svg"))
            .group(SettingGroup::new().items([
                SettingItem::new(
                    "Track skill activations",
                    switch(|p| p.tracking_enabled, |p, value| p.tracking_enabled = value),
                )
                .description("Import local agent histories while Skillshard is open. No history is uploaded."),
                SettingItem::new(
                    "Tracking history",
                    dropdown(
                        &[("7", "7 days"), ("15", "15 days"), ("30", "30 days"), ("60", "60 days")],
                        |p| p.tracking_days.days().to_string(),
                        |p, value| {
                            if let Some(days) = TrackingDays::ALL.into_iter().find(|days| days.days().to_string() == value) {
                                p.tracking_days = days;
                            }
                        },
                    ),
                )
                .description("Controls import, retention, and the maximum reporting range. Increasing it rescans available history."),
                SettingItem::render(move |_, _, cx| {
                    let Some(view) = view.upgrade() else {
                        return div().into_any_element();
                    };
                    render_rebuild_controls(&view, cx).into_any_element()
                }),
            ]))
    }
}

fn rebuild_is_busy(state: ServiceState) -> bool {
    matches!(
        state,
        ServiceState::Starting | ServiceState::Stopping | ServiceState::Rebuilding
    )
}

fn render_rebuild_controls(view: &Entity<SettingsDialog>, cx: &App) -> impl IntoElement {
    let dialog = view.read(cx);
    let status = &dialog.usage_status;
    let busy = rebuild_is_busy(status.state);
    let paused = status.state == ServiceState::RebuildPaused;
    let confirming = dialog.confirming_rebuild;
    let days = preferences::get(cx).tracking_days.days();
    let action_view = view.clone();
    let cancel_view = view.clone();
    let label = if paused {
        "Resume rebuild"
    } else if confirming {
        "Confirm rebuild"
    } else if status.state == ServiceState::Failed {
        "Retry rebuild"
    } else {
        "Rebuild local history"
    };
    v_flex()
        .gap_2()
        .child(div().text_sm().font_medium().child("Rebuild local history"))
        .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!(
            "Replaces Skillshard's local usage data and rescans available agent histories for the last {days} days. Agent transcripts, installed skills, CLI lockfiles, and preferences are unchanged. With tracking off, this runs once while the app is open."
        )))
        .when(busy, |this| this.child(div().text_sm().child(format!(
            "{} · {} records imported", crate::ui::usage::state_label(status.state), status.imported_records
        ))))
        .when(paused, |this| this.child(div().text_sm().child("Rebuild paused. Resume to finish the interrupted local scan.")))
        .when_some(status.error.clone(), |this, error| this.child(div().text_sm().text_color(cx.theme().danger).child(error)))
        .child(h_flex().gap_2().child(
            Button::new("rebuild-usage")
                .small()
                .label(label)
                .disabled(busy)
                .on_click(move |_, _, cx| action_view.update(cx, |dialog, cx| {
                    if paused || confirming {
                        dialog.request_rebuild(paused, cx);
                        return;
                    }
                    dialog.confirming_rebuild = true;
                    cx.notify();
                })),
        ).when(confirming, |this| this.child(
            Button::new("cancel-rebuild-usage").small().ghost().label("Cancel")
                .on_click(move |_, _, cx| cancel_view.update(cx, |dialog, cx| {
                    dialog.confirming_rebuild = false;
                    cx.notify();
                })),
        )))
}

#[cfg(test)]
mod tests {
    use super::{SettingsDialog, SettingsEvent};
    use crate::usage::{ServiceState, Status};
    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{px, size, AppContext, TestAppContext};
    use std::cell::Cell;
    use std::rc::Rc;

    #[gpui_kit::test]
    fn rebuild_requires_confirmation_and_blocks_repeated_requests(cx: &mut TestAppContext) {
        let preferences =
            std::env::temp_dir().join(format!("skillshard-rebuild-ui-{}.json", std::process::id()));
        cx.update(|cx| crate::ui::init(preferences, cx));
        let mut dialog = None;
        let handle = cx.open_window(size(px(1100.), px(800.)), |window, cx| {
            let view = cx.new(|cx| SettingsDialog::new(Vec::new(), window, cx));
            dialog = Some(view.clone());
            Root::new(view, window, cx)
        });
        let dialog = dialog.unwrap();
        let requests = Rc::new(Cell::new(0));
        let resumes = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let requests = requests.clone();
            let resumes = resumes.clone();
            cx.subscribe(&dialog, move |_, event, _| match event {
                SettingsEvent::RebuildHistory => requests.set(requests.get() + 1),
                SettingsEvent::ResumeRebuild => resumes.set(resumes.get() + 1),
                SettingsEvent::Close => {}
            })
            .detach();
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            // GPUI Kit identifies sidebar items by group and page position.
            window.click("0-4", cx);
            window.render_frame(cx);
            window.click("rebuild-usage", cx);
            assert_eq!(requests.get(), 0);
            window.render_frame(cx);
            window.click("rebuild-usage", cx);
            window.render_frame(cx);
            window.click("rebuild-usage", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(requests.get(), 1);
        cx.update(|cx| {
            dialog.update(cx, |view, cx| {
                view.update_usage_status(
                    Status {
                        state: ServiceState::RebuildPaused,
                        ..Status::default()
                    },
                    cx,
                )
            })
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("rebuild-usage", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(resumes.get(), 1);
    }
}
