// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchNotificationService.swift, commit e916961.

//! System notifications in the island (C5): banners of
//! `com.apple.notificationcenterui` read through Accessibility (AXObserver +
//! bounded tree walks). Off by default; the Accessibility prompt is shown
//! only on `NotificationCommand::RequestAccess`. Items live in memory only
//! and are never logged or persisted.
//!
//! "Hide native banners" (Vorssaint moves the Notification Center window off
//! screen on macOS 27) is evaluated, not implemented: moving the system's
//! window can also hide persistent alerts the island did not take.

pub mod tree;

use crate::sys::ax::{self, Element, Observer};
use crate::sys::workspace;
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::notifications::{
    NotificationCommand, NotificationItem, NotificationSettings, NotificationsState,
};
use nox_core::permissions::PermissionState;
use nox_core::{
    Alert, AlertKind, CancelToken, Hub, Module, ModuleContext, ModuleError, ModuleId, Settings,
    Worker,
};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::NSString;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tree::{Content, Walker};

const CENTER: &str = "com.apple.notificationcenterui";
/// Banners are laid out a few hundredths of a second after their window.
const DEBOUNCE: Duration = Duration::from_millis(80);
/// Trust / center-process check while idle.
const CHECK: Duration = Duration::from_secs(2);

enum Msg {
    Command(NotificationCommand),
    Settings(Arc<Settings>),
}

#[derive(Default)]
pub struct NotificationsModule {
    worker: Option<Worker>,
    tx: Option<Sender<Msg>>,
}

impl Module for NotificationsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Notifications
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let (tx, rx) = unbounded();
        let hub = ctx.hub.clone();
        let settings = ctx.settings.notifications.clone();
        hub.update(|s| s.notifications = Some(NotificationsState::default()));
        self.worker = Some(Worker::spawn("notifications", move |token| {
            run(hub, settings, rx, token)
        }));
        self.tx = Some(tx);
        Ok(())
    }

    fn stop(&mut self) {
        self.tx.take();
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }

    fn command(&mut self, cmd: nox_core::Command) {
        if let (nox_core::Command::Notifications(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(Msg::Command(c));
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Settings(settings.clone()));
        }
    }
}

fn center_pid() -> Option<i32> {
    let apps =
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(CENTER));
    apps.iter().next().map(|a| a.processIdentifier())
}

/// (name, bundle id) of running apps, to resolve a banner's source.
fn running_apps() -> Vec<(String, String)> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter_map(|a| {
            Some((
                a.localizedName()?.to_string(),
                a.bundleIdentifier()?.to_string(),
            ))
        })
        .collect()
}

/// Resolves the source app from the header or icon labels.
pub fn resolve_app(
    content: &Content,
    labels: &[String],
    running: &[(String, String)],
) -> (String, Option<String>) {
    let by_name = |n: &str| {
        running
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(n.trim()))
            .cloned()
    };
    if !content.app.is_empty() {
        let id = by_name(&content.app).map(|(_, id)| id);
        return (content.app.clone(), id);
    }
    let message = [&content.title, &content.subtitle, &content.body];
    for l in labels {
        if message.iter().any(|m| m.eq_ignore_ascii_case(l)) {
            continue;
        }
        if let Some((name, id)) = by_name(l) {
            return (name, Some(id));
        }
    }
    (String::new(), None)
}

pub fn excluded(s: &NotificationSettings, app: &str, bundle: Option<&str>) -> bool {
    s.excluded_apps.iter().any(|e| {
        let e = e.trim();
        !e.is_empty()
            && (e.eq_ignore_ascii_case(app) || bundle.is_some_and(|b| e.eq_ignore_ascii_case(b)))
    })
}

struct Live {
    id: u64,
    content: Content,
    element: Element,
}

struct Engine {
    hub: Hub,
    settings: NotificationSettings,
    state: NotificationsState,
    asked: bool,
    pid: Option<i32>,
    observer: Option<Observer>,
    events_tx: Sender<()>,
    app: Option<Element>,
    live: Vec<Live>,
    next_id: u64,
}

impl Engine {
    fn publish(&self) {
        let s = self.state.clone();
        self.hub.update(move |st| {
            if st.notifications.is_some() {
                st.notifications = Some(s);
            }
        });
    }

    fn access(&self) -> PermissionState {
        if ax::is_trusted() {
            PermissionState::Granted
        } else if self.asked {
            PermissionState::Denied
        } else {
            PermissionState::NotDetermined
        }
    }

    fn detach(&mut self) {
        if let Some(o) = self.observer.take() {
            // The observer's run-loop source lives on the main run loop.
            nox_core::main_thread::run_async(move || drop(o));
        }
        self.app = None;
        self.pid = None;
        self.live.clear();
    }

    /// Attaches to the Notification Center process (again if it restarted).
    fn ensure_attached(&mut self) {
        let access = self.access();
        if access != self.state.access {
            self.state.access = access;
            self.publish();
        }
        if access != PermissionState::Granted {
            self.detach();
            return;
        }
        let pid = center_pid();
        if pid == self.pid && self.observer.is_some() {
            return;
        }
        self.detach();
        let Some(pid) = pid else { return };
        let tx = self.events_tx.clone();
        self.observer =
            crate::sys::on_main(crate::sys::MAIN_TIMEOUT, move || Observer::new(pid, tx)).flatten();
        if self.observer.is_some() {
            self.pid = Some(pid);
            self.app = Element::application(pid);
            self.scan();
        }
    }

    fn scan(&mut self) {
        let Some(app) = self.app.clone() else { return };
        // The open center lists old notifications: never import them.
        if app.has_focused_window() {
            return;
        }
        let mut w = Walker::default();
        let mut found = vec![];
        for win in app.windows().into_iter().take(32) {
            w.find(&win, 0, &mut found);
        }
        let running = running_apps();
        let previous: HashSet<Content> = self.live.iter().map(|l| l.content.clone()).collect();
        let mut live = vec![];
        let mut changed = false;
        for f in found {
            let Some(content) = f.content else { continue };
            if let Some(old) = self.live.iter().find(|l| l.content == content) {
                live.push(Live {
                    id: old.id,
                    content,
                    element: f.root,
                });
                continue;
            }
            if previous.contains(&content) {
                continue;
            }
            let (app_name, bundle) = resolve_app(&content, &f.image_labels, &running);
            if excluded(&self.settings, &app_name, bundle.as_deref()) {
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            let item = NotificationItem {
                id,
                app_name: app_name.clone(),
                bundle_id: bundle,
                title: content.title.clone(),
                subtitle: (!content.subtitle.is_empty()).then(|| content.subtitle.clone()),
                body: (!self.settings.hide_body && !content.body.is_empty())
                    .then(|| content.body.clone()),
                received_at: nox_core::model::now(),
            };
            let title = if app_name.is_empty() {
                item.title.clone()
            } else {
                format!("{app_name} · {}", item.title)
            };
            let mut a = Alert::new(AlertKind::Notification, title, "bell.badge");
            if let Some(b) = item.body.clone().or_else(|| item.subtitle.clone()) {
                a = a.subtitle(b);
            }
            self.hub.push_alert(a.ttl(5.0));
            self.state.items.insert(0, item);
            changed = true;
            live.push(Live {
                id,
                content,
                element: f.root,
            });
        }
        // An incomplete walk must not drop banners it could not read.
        if !w.incomplete || live.len() >= self.live.len() {
            self.live = live;
        }
        let max = self.settings.max_items.clamp(1, 200);
        if self.state.items.len() > max {
            self.state.items.truncate(max);
            changed = true;
        }
        if changed {
            self.publish();
        }
    }

    fn on_command(&mut self, c: NotificationCommand) {
        match c {
            NotificationCommand::Dismiss(id) => self.state.items.retain(|i| i.id != id),
            NotificationCommand::ClearAll => self.state.items.clear(),
            NotificationCommand::OpenApp(id) => {
                let pressed = self.live.iter().find(|l| l.id == id).is_some_and(|l| {
                    l.element.actions().iter().any(|a| a == "AXPress")
                        && l.element.perform("AXPress")
                });
                if !pressed
                    && let Some(b) = self
                        .state
                        .items
                        .iter()
                        .find(|i| i.id == id)
                        .and_then(|i| i.bundle_id.clone())
                {
                    workspace::activate_app(None, Some(&b));
                }
            }
            NotificationCommand::RequestAccess => {
                self.asked = true;
                // Shows the system prompt (with its own "Open Settings").
                let trusted = crate::sys::on_main(crate::sys::MAIN_TIMEOUT, ax::prompt_trust)
                    .unwrap_or(false);
                if trusted {
                    self.ensure_attached();
                }
            }
        }
        self.state.access = self.access();
        self.publish();
    }
}

fn run(hub: Hub, settings: NotificationSettings, rx: Receiver<Msg>, token: CancelToken) {
    let (events_tx, events_rx) = crossbeam_channel::bounded(1);
    let mut e = Engine {
        hub,
        settings,
        state: NotificationsState::default(),
        asked: false,
        pid: None,
        observer: None,
        events_tx,
        app: None,
        live: vec![],
        next_id: 1,
    };
    e.state.access = e.access();
    e.publish();
    e.ensure_attached();
    let mut last_check = Instant::now();
    while !token.is_cancelled() {
        select! {
            recv(events_rx) -> _ => {
                std::thread::sleep(DEBOUNCE);
                e.scan();
            },
            recv(rx) -> m => match m {
                Ok(Msg::Command(c)) => e.on_command(c),
                Ok(Msg::Settings(s)) => {
                    e.settings = s.notifications.clone();
                    let st = e.settings.clone();
                    e.state.items.retain(|i| !excluded(&st, &i.app_name, i.bundle_id.as_deref()));
                    if st.hide_body {
                        e.state.items.iter_mut().for_each(|i| i.body = None);
                    }
                    e.publish();
                }
                Err(_) => break,
            },
            default(Duration::from_millis(500)) => {},
        }
        if last_check.elapsed() >= CHECK {
            last_check = Instant::now();
            e.ensure_attached();
        }
    }
    e.detach();
    e.hub.update(|s| s.notifications = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(app: &str, title: &str) -> Content {
        Content {
            app: app.into(),
            title: title.into(),
            subtitle: String::new(),
            body: "b".into(),
        }
    }

    #[test]
    fn resolves_source_app() {
        let running = vec![("Messages".to_owned(), "com.apple.MobileSMS".to_owned())];
        assert_eq!(
            resolve_app(&content("Messages", "x"), &[], &running),
            ("Messages".into(), Some("com.apple.MobileSMS".into()))
        );
        // Header missing: icon label names the app; a label equal to the
        // title (a contact photo) is ignored.
        assert_eq!(
            resolve_app(&content("", "Messages"), &["Messages".into()], &running).1,
            None
        );
        assert_eq!(
            resolve_app(&content("", "Synthetic"), &["messages".into()], &running).1,
            Some("com.apple.MobileSMS".into())
        );
    }

    #[test]
    fn exclusions_by_name_or_bundle() {
        let s = NotificationSettings {
            excluded_apps: vec!["com.apple.mail".into(), "Slack".into()],
            ..Default::default()
        };
        assert!(excluded(&s, "Mail", Some("com.apple.Mail")));
        assert!(excluded(&s, "slack", None));
        assert!(!excluded(&s, "Messages", Some("com.apple.MobileSMS")));
    }

    #[test]
    #[ignore = "real machine: reads trust and the Notification Center pid only (no prompt)"]
    fn live_trust_and_center() {
        eprintln!(
            "AX trusted: {}, center pid found: {}",
            ax::is_trusted(),
            center_pid().is_some()
        );
    }
}
