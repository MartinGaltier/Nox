// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchCalendarService.swift, commit e916961.

//! Calendar (C1–C4): EventKit events (+ optional reminders), change
//! notifications, hidden calendars, video-call detection, "starts soon" alert.
//! Events stay in memory; nothing is logged or persisted.

pub mod links;

use crate::sys::eventkit::{self, Entity, RawCalendar, RawItem, Store};
use crate::sys::workspace;
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::calendar::{
    CalendarCommand, CalendarEvent, CalendarInfo, CalendarSettings, CalendarState,
};
use nox_core::permissions::{Permission, PermissionState};
use nox_core::{
    Alert, AlertKind, AlertPriority, CancelToken, Hub, Module, ModuleContext, ModuleError,
    ModuleId, Settings, Worker,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Periodic re-read even without a change notification (day rollover…).
const REFRESH_EVERY: Duration = Duration::from_secs(15 * 60);
/// EventKit sends bursts of change notifications.
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(500);
/// Longest sleep: a backstop for wall-clock jumps (alerts and refreshes wake on time).
const MAX_WAIT: Duration = Duration::from_secs(60);

enum Msg {
    Command(CalendarCommand),
    Settings(Arc<Settings>),
}

#[derive(Default)]
pub struct CalendarModule {
    worker: Option<Worker>,
    tx: Option<Sender<Msg>>,
}

impl Module for CalendarModule {
    fn id(&self) -> ModuleId {
        ModuleId::Calendar
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let (tx, rx) = unbounded();
        let hub = ctx.hub.clone();
        let settings = ctx.settings.calendar.clone();
        hub.update(|s| s.calendar = Some(CalendarState::default()));
        self.worker = Some(Worker::spawn("calendar", move |token| {
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
        if let (nox_core::Command::Calendar(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(Msg::Command(c));
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Settings(settings.clone()));
        }
    }
}

pub fn is_hidden(settings: &CalendarSettings, id: &str, title: &str) -> bool {
    settings
        .hidden_calendars
        .iter()
        .any(|h| h == id || h == title)
}

pub fn to_event(item: RawItem, is_reminder: bool) -> CalendarEvent {
    let call = links::detect(
        item.url.as_deref(),
        item.location.as_deref(),
        item.notes.as_deref(),
    );
    CalendarEvent {
        id: item.id,
        title: item.title,
        start: item.start,
        end: item.end,
        all_day: item.all_day,
        calendar: item.calendar_title,
        color: item.color,
        location: item.location,
        url: item.url,
        call,
        is_reminder,
    }
}

/// Start of the local day containing `t` (Unix seconds).
fn start_of_day(t: f64) -> f64 {
    let secs = t as libc::time_t;
    let mut tm: libc::tm = libc::tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null_mut(),
    };
    // SAFETY: localtime_r writes into our stack `tm`; mktime reads it back.
    unsafe {
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return t - t.rem_euclid(86_400.0);
        }
        tm.tm_sec = 0;
        tm.tm_min = 0;
        tm.tm_hour = 0;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm) as f64
    }
}

/// Events that should raise a "starts soon" alert now. `alerted` holds
/// `id@start` keys already announced.
pub fn due_alerts<'a>(
    events: &'a [CalendarEvent],
    minutes_before: u32,
    now: f64,
    alerted: &HashSet<String>,
) -> Vec<&'a CalendarEvent> {
    if minutes_before == 0 {
        return vec![];
    }
    let window = f64::from(minutes_before) * 60.0;
    events
        .iter()
        .filter(|e| !e.all_day && !e.is_reminder)
        .filter(|e| e.start - now <= window && e.start - now > -60.0)
        .filter(|e| !alerted.contains(&alert_key(e)))
        .collect()
}

/// Time until the next event enters its alert window (`None` if none will).
fn alert_wait(
    events: &[CalendarEvent],
    minutes_before: u32,
    now: f64,
    alerted: &HashSet<String>,
) -> Option<Duration> {
    let window = f64::from(minutes_before) * 60.0;
    if window <= 0.0 {
        return None;
    }
    events
        .iter()
        .filter(|e| !e.all_day && !e.is_reminder)
        .filter(|e| !alerted.contains(&alert_key(e)))
        .map(|e| e.start - window)
        .filter(|t| *t > now)
        .min_by(f64::total_cmp)
        .map(|t| Duration::from_secs_f64(t - now))
}

fn alert_key(e: &CalendarEvent) -> String {
    format!("{}@{}", e.id, e.start as i64)
}

pub fn alert_subtitle(e: &CalendarEvent, now: f64) -> String {
    let mins = ((e.start - now) / 60.0).ceil().max(0.0) as i64;
    let when = if mins <= 0 {
        "Maintenant".to_owned()
    } else {
        format!("Dans {mins} min")
    };
    match &e.call {
        Some(c) => format!("{when} · {}", c.service.label()),
        None => when,
    }
}

struct Engine {
    hub: Hub,
    settings: CalendarSettings,
    state: CalendarState,
    store: Store,
    access_tx: Sender<(Entity, bool)>,
    reminders_tx: Sender<Vec<RawItem>>,
    events: Vec<CalendarEvent>,
    reminders: Vec<CalendarEvent>,
    alerted: HashSet<String>,
    last_refresh: Option<Instant>,
    dirty_at: Option<Instant>,
}

impl Engine {
    fn publish(&mut self) {
        let mut all: Vec<CalendarEvent> = self
            .events
            .iter()
            .chain(self.reminders.iter())
            .cloned()
            .collect();
        all.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.title.cmp(&b.title)));
        self.state.events = all;
        let s = self.state.clone();
        self.hub.update(move |st| {
            if st.calendar.is_some() {
                st.calendar = Some(s);
            }
        });
    }

    fn reminders_wanted(&self) -> bool {
        self.settings.show_reminders
    }

    fn refresh_access(&mut self) {
        self.state.access = eventkit::status(Entity::Events);
        self.state.reminders_access = if self.reminders_wanted() {
            eventkit::status(Entity::Reminders)
        } else {
            PermissionState::NotRequired
        };
    }

    fn request_access(&mut self, open_settings_if_denied: bool) {
        self.refresh_access();
        let mut denied = false;
        match self.state.access {
            PermissionState::NotDetermined => self
                .store
                .request_access(Entity::Events, self.access_tx.clone()),
            PermissionState::Denied | PermissionState::Restricted => denied = true,
            _ => {}
        }
        if self.reminders_wanted() {
            match self.state.reminders_access {
                PermissionState::NotDetermined => self
                    .store
                    .request_access(Entity::Reminders, self.access_tx.clone()),
                PermissionState::Denied | PermissionState::Restricted => denied = true,
                _ => {}
            }
        }
        if denied && open_settings_if_denied {
            // The system will not prompt again: send the user to Settings.
            workspace::open_url(Permission::Calendars.settings_url());
        }
    }

    fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        self.dirty_at = None;
        self.refresh_access();
        let settings = self.settings.clone();
        let hidden = move |c: &RawCalendar| is_hidden(&settings, &c.id, &c.title);
        let now = nox_core::model::now();
        let start = start_of_day(now);
        let end = start + f64::from(self.settings.days_ahead.clamp(1, 62)) * 86_400.0 + 86_400.0;
        if self.state.access == PermissionState::Granted {
            self.state.calendars = self
                .store
                .calendars(Entity::Events)
                .into_iter()
                .map(|c| CalendarInfo {
                    id: c.id,
                    title: c.title,
                    color: c.color,
                    source: c.source,
                })
                .collect();
            self.events = self
                .store
                .events(start, end, &hidden)
                .into_iter()
                .filter(|e| !e.cancelled && e.end > now)
                .map(|e| to_event(e, false))
                .collect();
        } else {
            self.state.calendars.clear();
            self.events.clear();
        }
        if self.reminders_wanted() && self.state.reminders_access == PermissionState::Granted {
            self.store.reminders(
                start - 86_400.0 * 30.0,
                end,
                &hidden,
                self.reminders_tx.clone(),
            );
        } else {
            self.reminders.clear();
        }
        self.check_alerts();
        self.publish();
    }

    /// How long the loop may sleep: until the next alert window opens, the
    /// periodic refresh, or the debounced change refresh is due.
    fn wait(&self, now: f64) -> Duration {
        let refresh = self
            .last_refresh
            .map(|t| REFRESH_EVERY.saturating_sub(t.elapsed()));
        let change = self
            .dirty_at
            .map(|t| t.saturating_duration_since(Instant::now()));
        let alert = alert_wait(
            &self.events,
            self.settings.alert_minutes_before,
            now,
            &self.alerted,
        );
        [alert, refresh, change]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(MAX_WAIT)
            .min(MAX_WAIT)
    }

    fn check_alerts(&mut self) {
        let now = nox_core::model::now();
        let due: Vec<CalendarEvent> = due_alerts(
            &self.events,
            self.settings.alert_minutes_before,
            now,
            &self.alerted,
        )
        .into_iter()
        .cloned()
        .collect();
        for e in due {
            self.alerted.insert(alert_key(&e));
            self.hub.push_alert(
                Alert::new(AlertKind::CalendarSoon, e.title.clone(), "calendar")
                    .subtitle(alert_subtitle(&e, now))
                    .priority(AlertPriority::High)
                    .ttl(8.0),
            );
        }
        // Forget keys of events that are long past.
        let live: HashSet<String> = self.events.iter().map(alert_key).collect();
        self.alerted.retain(|k| live.contains(k));
    }

    fn on_command(&mut self, c: CalendarCommand) {
        match c {
            CalendarCommand::RequestAccess => self.request_access(true),
            CalendarCommand::Refresh => self.refresh(),
            CalendarCommand::OpenEvent(id) => {
                let url = format!(
                    "ical://ekevent/{}?method=show&options=more",
                    crate::sys::http::encode_component(&id)
                );
                if !workspace::open_url(&url) {
                    workspace::open_app("com.apple.iCal");
                }
            }
            CalendarCommand::JoinCall(id) => {
                if let Some(call) = self
                    .state
                    .events
                    .iter()
                    .find(|e| e.id == id)
                    .and_then(|e| e.call.clone())
                {
                    workspace::open_url(&call.url);
                }
            }
        }
    }
}

fn run(hub: Hub, settings: CalendarSettings, rx: Receiver<Msg>, token: CancelToken) {
    let (access_tx, access_rx) = unbounded();
    let (reminders_tx, reminders_rx) = unbounded();
    let (change_tx, change_rx) = unbounded::<()>();
    // Keeps `change_rx` connected if the observer is dropped, so the select
    // below cannot spin on a disconnected channel.
    let _change_keepalive = change_tx.clone();
    let _observer = eventkit::observe_changes(move || {
        let _ = change_tx.send(());
    });
    let mut e = Engine {
        hub,
        settings,
        state: CalendarState::default(),
        store: Store::new(),
        access_tx,
        reminders_tx,
        events: vec![],
        reminders: vec![],
        alerted: HashSet::new(),
        last_refresh: None,
        dirty_at: None,
    };
    // First start of the module: ask once if never asked.
    e.request_access(false);
    e.refresh();
    while !token.is_cancelled() {
        let wait = e.wait(nox_core::model::now());
        select! {
            recv(rx) -> m => match m {
                Ok(Msg::Command(c)) => e.on_command(c),
                Ok(Msg::Settings(s)) => {
                    let wants_reminders = s.calendar.show_reminders && !e.settings.show_reminders;
                    e.settings = s.calendar.clone();
                    if wants_reminders {
                        e.request_access(false);
                    }
                    e.refresh();
                }
                Err(_) => break,
            },
            recv(access_rx) -> a => if a.is_ok() {
                // A new store sees calendars granted after its creation.
                e.store = Store::new();
                e.refresh();
            },
            recv(change_rx) -> _ => {
                e.dirty_at.get_or_insert_with(|| Instant::now() + CHANGE_DEBOUNCE);
            },
            recv(reminders_rx) -> r => if let Ok(items) = r {
                let now = nox_core::model::now();
                e.reminders = items
                    .into_iter()
                    .filter(|r| r.end >= start_of_day(now))
                    .map(|r| to_event(r, true))
                    .collect();
                e.publish();
            },
            default(wait) => {},
        }
        if e.dirty_at.is_some_and(|t| Instant::now() >= t)
            || e.last_refresh.is_none_or(|t| t.elapsed() >= REFRESH_EVERY)
        {
            e.refresh();
        } else {
            e.check_alerts();
        }
    }
    e.hub.update(|s| s.calendar = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_core::model::calendar::CallService;

    fn ev(id: &str, start: f64) -> CalendarEvent {
        to_event(
            RawItem {
                id: id.into(),
                title: format!("Synthetic {id}"),
                start,
                end: start + 1800.0,
                all_day: false,
                calendar_id: "cal-1".into(),
                calendar_title: "Work".into(),
                color: [0.1, 0.2, 0.3],
                location: Some("https://x.zoom.us/j/1".into()),
                notes: None,
                url: None,
                cancelled: false,
            },
            false,
        )
    }

    #[test]
    fn hidden_by_id_or_title() {
        let s = CalendarSettings {
            hidden_calendars: vec!["cal-2".into(), "Anniversaires".into()],
            ..Default::default()
        };
        assert!(is_hidden(&s, "cal-2", "Perso"));
        assert!(is_hidden(&s, "x", "Anniversaires"));
        assert!(!is_hidden(&s, "cal-1", "Work"));
    }

    #[test]
    fn alerts_fire_once_in_window() {
        let now = 10_000.0;
        let events = vec![
            ev("a", now + 240.0),
            ev("b", now + 900.0),
            ev("c", now - 120.0),
        ];
        let mut alerted = HashSet::new();
        let due = due_alerts(&events, 5, now, &alerted);
        assert_eq!(
            due.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["a"]
        );
        assert_eq!(due[0].call.as_ref().unwrap().service, CallService::Zoom);
        assert_eq!(alert_subtitle(due[0], now), "Dans 4 min · Zoom");
        alerted.insert(alert_key(due[0]));
        assert!(due_alerts(&events, 5, now, &alerted).is_empty());
        assert!(
            due_alerts(&events, 0, now, &HashSet::new()).is_empty(),
            "0 = off"
        );
    }

    #[test]
    fn alert_wait_sleeps_until_the_window_opens() {
        let now = 10_000.0;
        let events = vec![ev("a", now + 900.0), ev("b", now + 120.0)];
        // "a" opens its 5-minute window in 600 s; "b" is already inside it.
        assert_eq!(
            alert_wait(&events, 5, now, &HashSet::new()),
            Some(Duration::from_secs(600))
        );
        let alerted = HashSet::from([alert_key(&events[0])]);
        assert_eq!(alert_wait(&events, 5, now, &alerted), None);
        assert_eq!(
            alert_wait(&events, 0, now, &HashSet::new()),
            None,
            "0 = off"
        );
    }

    #[test]
    fn start_of_day_is_local_midnight() {
        let now = nox_core::model::now();
        let s = start_of_day(now);
        assert!(s <= now && now - s < 86_400.0 + 3_600.0);
    }

    #[test]
    #[ignore = "real machine: reads EventKit if this process already has Calendar access"]
    fn live_eventkit_read() {
        let st = eventkit::status(Entity::Events);
        eprintln!("calendar access: {st:?}");
        if st != PermissionState::Granted {
            eprintln!("non testé : permission");
            return;
        }
        let store = Store::new();
        let cals = store.calendars(Entity::Events);
        let now = nox_core::model::now();
        let evs = store.events(start_of_day(now), now + 7.0 * 86_400.0, &|_| false);
        // Counts only: titles are private.
        eprintln!("{} calendars, {} events in 7 days", cals.len(), evs.len());
    }
}
