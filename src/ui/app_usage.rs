//! Application-owned usage worker integration; all I/O runs in the background.

use super::Skillshard;
use crate::model::{Scope, Skill};
use crate::preferences::{self, TrackingDays};
use crate::usage::service::{Config, Relocation, Service, View};
use crate::usage::{Query, ServiceState, Snapshot, Status};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[path = "app_usage_view.rs"]
mod view;

pub(super) struct Runtime {
    service: Option<Arc<Mutex<Service>>>,
    view: Arc<Mutex<View>>,
    /// Created the first time the usage view is shown, then kept so its
    /// filters survive switching views.
    pub(super) panel: Option<Entity<crate::ui::usage::UsageView>>,
    applied_config: Option<(bool, TrackingDays, Vec<Scope>)>,
    control_revision: Arc<AtomicU64>,
    query_revision: Arc<AtomicU64>,
    refresh: Option<Task<()>>,
    seen_revision: u64,
    changing: bool,
    cancelled: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    expiry_ticks: u8,
    pending_relocations: Arc<Mutex<Vec<Relocation>>>,
}

impl Runtime {
    pub(super) fn new(cx: &App) -> Self {
        let environment = cx.global::<crate::ui::UsageEnvironment>();
        let (service, view, cancelled, shutdown) = match environment.directory.clone() {
            Ok(directory) => {
                let service = Service::new(directory, environment.roots.clone());
                let view = service.view();
                let cancelled = service.cancellation();
                let shutdown = service.shutdown_signal();
                (
                    Some(Arc::new(Mutex::new(service))),
                    view,
                    cancelled,
                    shutdown,
                )
            }
            Err(error) => (
                None,
                Arc::new(Mutex::new(View {
                    status: Status {
                        state: ServiceState::Failed,
                        error: Some(error),
                        ..Status::default()
                    },
                    ..View::default()
                })),
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicBool::new(false)),
            ),
        };
        Self {
            service,
            view,
            panel: None,
            applied_config: None,
            control_revision: Arc::new(AtomicU64::new(0)),
            query_revision: Arc::new(AtomicU64::new(0)),
            refresh: None,
            seen_revision: u64::MAX,
            changing: false,
            cancelled,
            shutdown,
            expiry_ticks: 0,
            pending_relocations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn current_view(&self) -> View {
        self.view
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.control_revision.fetch_add(1, Ordering::AcqRel);
        self.query_revision.fetch_add(1, Ordering::AcqRel);
        self.cancelled.store(true, Ordering::Release);
        if let Some(service) = &self.service {
            if let Err(error) = service
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .stop()
            {
                eprintln!("could not stop usage tracking on window close: {error}");
            }
        }
    }
}

impl Skillshard {
    pub(super) fn start_usage_updates(&mut self, cx: &mut Context<Self>) {
        let service = self.usage.service.clone();
        let control_revision = self.usage.control_revision.clone();
        let query_revision = self.usage.query_revision.clone();
        let cancelled = self.usage.cancelled.clone();
        let shutdown = self.usage.shutdown.clone();
        self._subscriptions.push(cx.on_app_quit(move |_, cx| {
            shutdown.store(true, Ordering::Release);
            control_revision.fetch_add(1, Ordering::AcqRel);
            query_revision.fetch_add(1, Ordering::AcqRel);
            cancelled.store(true, Ordering::Release);
            let service = service.clone();
            cx.background_executor().spawn(async move {
                if let Some(service) = service {
                    if let Err(error) = service
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .stop()
                    {
                        eprintln!("could not stop usage tracking on app close: {error}");
                    }
                }
            })
        }));
        self.usage.refresh = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if this
                .update(cx, |this, cx| this.publish_usage_view(cx))
                .is_err()
            {
                break;
            }
        }));
    }

    pub(super) fn configure_usage(&mut self, rebuild: bool, cx: &mut Context<Self>) {
        let preferences = preferences::get(cx);
        let key = (
            preferences.tracking_enabled,
            preferences.tracking_days,
            self.scopes.clone(),
        );
        if !rebuild && self.usage.applied_config.as_ref() == Some(&key) {
            return;
        }
        let Some(service) = self.usage.service.clone() else {
            return;
        };
        self.usage.applied_config = Some(key.clone());
        self.usage.changing = true;
        let expected = self.usage.control_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let revision = self.usage.control_revision.clone();
        let config = Config {
            enabled: key.0,
            days: key.1,
            scopes: key.2,
        };
        let state = if rebuild {
            ServiceState::Rebuilding
        } else if config.enabled {
            ServiceState::Starting
        } else {
            ServiceState::Stopping
        };
        if let Some(panel) = &self.usage.panel {
            panel.update(cx, |panel, cx| {
                panel.update_data(
                    Snapshot::default(),
                    Status {
                        state,
                        ..Status::default()
                    },
                    cx,
                );
                panel.update_tracking_days(config.days, cx);
            });
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut service = service.lock().unwrap_or_else(|poison| poison.into_inner());
                    if revision.load(Ordering::Acquire) != expected {
                        return Ok(());
                    }
                    if rebuild {
                        service.rebuild(config)
                    } else {
                        service.configure(config)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.usage.control_revision.load(Ordering::Acquire) != expected {
                    return;
                }
                this.usage.changing = false;
                if let Err(error) = result {
                    this.report_usage_error(error);
                }
                this.publish_usage_view(cx);
                this.request_usage_query(cx);
            });
        })
        .detach();
    }

    fn report_usage_error(&mut self, error: String) {
        let mut view = self
            .usage
            .view
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        view.status.error = Some(error);
        view.status.state = ServiceState::Failed;
        view.snapshot = Snapshot::default();
        view.revision += 1;
    }

    fn request_usage_query(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.usage.service.clone() else {
            return;
        };
        let query = self
            .usage
            .panel
            .as_ref()
            .map_or_else(Query::default, |panel| panel.read(cx).query());
        let expected = self.usage.query_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let revision = self.usage.query_revision.clone();
        let control_revision = self.usage.control_revision.clone();
        let expected_control = control_revision.load(Ordering::Acquire);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut service = service.lock().unwrap_or_else(|poison| poison.into_inner());
                    if revision.load(Ordering::Acquire) != expected
                        || control_revision.load(Ordering::Acquire) != expected_control
                    {
                        return Ok(());
                    }
                    service.query(query)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_usage_query(result, expected, expected_control, cx);
            });
        })
        .detach();
    }

    fn finish_usage_query(
        &mut self,
        result: Result<(), String>,
        expected_query: u64,
        expected_control: u64,
        cx: &mut Context<Self>,
    ) {
        if self.usage.query_revision.load(Ordering::Acquire) != expected_query
            || self.usage.control_revision.load(Ordering::Acquire) != expected_control
        {
            return;
        }
        if let Err(error) = result {
            self.report_usage_error(error);
        }
        self.publish_usage_view(cx);
    }

    pub(super) fn refresh_usage_installs(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.usage.service.clone() else {
            return;
        };
        let scopes = self.scopes.clone();
        let pending_relocations = self.usage.pending_relocations.clone();
        let revision = self.usage.control_revision.clone();
        let expected = revision.load(Ordering::Acquire);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let service = service.lock().unwrap_or_else(|poison| poison.into_inner());
                    let changes = std::mem::take(
                        &mut *pending_relocations
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner()),
                    );
                    service.apply_install_changes(changes, scopes)
                })
                .await;
            let _ = this.update(cx, |this, _| {
                if this.usage.control_revision.load(Ordering::Acquire) != expected {
                    return;
                }
                if let Err(error) = result {
                    this.report_usage_error(error);
                }
            });
        })
        .detach();
    }

    pub(super) fn record_usage_move(
        &mut self,
        skill: Skill,
        destination: Scope,
        _: &mut Context<Self>,
    ) {
        self.usage
            .pending_relocations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(Relocation::Move(skill, destination));
    }

    pub(super) fn record_usage_toggle(&mut self, skill: Skill) {
        let change = if skill.disabled {
            Relocation::Enable(skill)
        } else {
            Relocation::Disable(skill)
        };
        self.usage
            .pending_relocations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(change);
    }

    fn expire_disabled_usage(&self, cx: &mut Context<Self>) {
        if preferences::get(cx).tracking_enabled {
            return;
        }
        let Some(service) = self.usage.service.clone() else {
            return;
        };
        let expected = self.usage.control_revision.load(Ordering::Acquire);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    service
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .expire_disabled()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.usage.control_revision.load(Ordering::Acquire) != expected {
                    return;
                }
                if let Err(error) = result {
                    this.report_usage_error(error);
                }
                this.publish_usage_view(cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
#[path = "app_usage_tests.rs"]
mod tests;
