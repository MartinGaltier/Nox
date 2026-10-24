//! Keep-awake state machine. Pure: wall-clock time is passed in, side effects
//! go through [`Assertions`] and [`LidControl`] (fakes in tests).
//!
//! Adapted from vorssaint-utils (GPL-3.0-or-later), Services/KeepAwakeManager.swift, commit e916961.
//! (session lifecycle, battery protection semantics: stop when on battery at
//! or below the limit; refuse to start in that state.)

use crate::assertions::{AssertionId, AssertionKind, Assertions};
use crate::lid::{HEARTBEAT_SECS, LidControl, LidError};
use nox_core::alert::{Alert, AlertKind, AlertPriority};
use nox_core::model::Timestamp;
use nox_core::model::power::{
    HelperStatus, KeepAwakeDuration, KeepAwakeMode, KeepAwakeSession, PowerSettings, PowerState,
    StopReason,
};
use nox_helper::protocol::{ErrorCode, RestoreOutcome, RevokeReason};
use nox_helper::sys::PowerSourceInfo;

struct Active {
    session: KeepAwakeSession,
    assertions: Vec<AssertionId>,
    lid_held: bool,
    next_heartbeat: Timestamp,
}

pub struct Engine<A: Assertions, L: LidControl> {
    assertions: A,
    lid: L,
    settings: PowerSettings,
    power: Option<PowerSourceInfo>,
    active: Option<Active>,
    state: PowerState,
    alerts: Vec<Alert>,
    dirty: bool,
}

fn alert(title: impl Into<String>, symbol: &str) -> Alert {
    Alert::new(AlertKind::KeepAwake, title, symbol)
}

impl<A: Assertions, L: LidControl> Engine<A, L> {
    pub fn new(assertions: A, lid: L, settings: PowerSettings) -> Self {
        Self {
            assertions,
            lid,
            settings,
            power: None,
            active: None,
            state: PowerState::default(),
            alerts: Vec::new(),
            dirty: true,
        }
    }

    pub fn state(&self) -> &PowerState {
        &self.state
    }

    /// `true` once after each state change (the module then publishes).
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn take_alerts(&mut self) -> Vec<Alert> {
        std::mem::take(&mut self.alerts)
    }

    pub fn lid_mut(&mut self) -> &mut L {
        &mut self.lid
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn set_notice(&mut self, notice: Option<String>) {
        self.state.notice = notice;
        self.dirty = true;
    }

    pub fn set_helper_busy(&mut self, busy: bool) {
        self.state.helper_busy = busy;
        self.dirty = true;
    }

    pub fn set_error(&mut self, e: Option<String>) {
        self.state.last_error = e;
        self.dirty = true;
    }

    /// Re-reads helper status and `SleepDisabled`.
    pub fn refresh(&mut self) {
        self.state.helper = self.lid.status();
        self.state.lid_sleep_disabled = self.lid.sleep_disabled();
        self.dirty = true;
    }

    fn refuse(&mut self, msg: String) {
        log::info!("keep-awake refused: {msg}");
        self.alerts.push(
            alert("Maintien éveillé impossible", "exclamationmark.triangle").subtitle(msg.clone()),
        );
        self.state.last_error = Some(msg);
        self.dirty = true;
    }

    /// Power guard for the current settings: why a session may not run now.
    fn power_guard(&self) -> Option<StopReason> {
        let p = self.power?;
        if self.settings.only_on_ac && p.on_battery {
            Some(StopReason::OnBattery)
        } else if p.below(self.settings.stop_below_percent) {
            Some(StopReason::BatteryThreshold)
        } else {
            None
        }
    }

    pub fn toggle(&mut self, now: Timestamp) {
        if self.active.is_some() {
            self.stop(StopReason::User, now);
        } else {
            self.start(
                self.settings.default_mode,
                self.settings.default_duration,
                now,
            );
        }
    }

    /// Starts a session, replacing a running one (keeping the closed-lid
    /// lease when both are closed-lid, so pmset is not toggled needlessly).
    pub fn start(&mut self, mode: KeepAwakeMode, duration: KeepAwakeDuration, now: Timestamp) {
        if let Some(r) = self.power_guard() {
            let msg = match r {
                StopReason::OnBattery => {
                    "Sur batterie : l’option « uniquement sur secteur » est active.".to_string()
                }
                _ => format!(
                    "Batterie à {} % (seuil {} %).",
                    self.power
                        .and_then(|p| p.battery_percent)
                        .map_or("?".into(), |p| p.to_string()),
                    self.settings.stop_below_percent
                ),
            };
            self.refuse(msg);
            return;
        }

        let keep_lid = mode.needs_helper() && self.active.as_ref().is_some_and(|a| a.lid_held);
        if let Some(old) = self.active.take() {
            self.release_assertions(&old.assertions);
            if old.lid_held && !keep_lid {
                self.release_lid();
            }
        }

        let mut kinds = vec![AssertionKind::System];
        if mode == KeepAwakeMode::SystemAndDisplay {
            kinds.push(AssertionKind::Display);
        }
        let mut ids = Vec::new();
        for k in kinds {
            match self.assertions.create(k) {
                Ok(id) => ids.push(id),
                Err(e) => {
                    self.release_assertions(&ids);
                    if keep_lid {
                        self.release_lid();
                    }
                    self.refuse(format!("Assertion d’énergie refusée par macOS ({e})."));
                    return;
                }
            }
        }

        if mode.needs_helper() && !keep_lid {
            if let Err(e) = self.lid.acquire(self.settings.stop_below_percent) {
                self.release_assertions(&ids);
                if matches!(e, LidError::NotInstalled | LidError::Forbidden(_)) {
                    self.refresh();
                }
                self.refuse(e.message_fr());
                return;
            }
        } else if keep_lid && let Err(e) = self.lid.acquire(self.settings.stop_below_percent) {
            // Refresh of the existing lease failed: treat as lost.
            self.release_assertions(&ids);
            self.release_lid();
            self.refuse(e.message_fr());
            return;
        }

        let session = KeepAwakeSession {
            mode,
            duration,
            started_at: now,
            ends_at: duration.as_secs().map(|s| now + s as f64),
        };
        log::info!("keep-awake started: {mode:?} {duration:?}");
        self.alerts.push(
            alert("Maintien éveillé activé", mode.symbol()).subtitle(format!(
                "{} · {}",
                mode.label_fr(),
                duration.label_fr()
            )),
        );
        self.active = Some(Active {
            session: session.clone(),
            assertions: ids,
            lid_held: mode.needs_helper(),
            next_heartbeat: now + HEARTBEAT_SECS,
        });
        self.state.active = Some(session);
        self.state.last_error = None;
        if mode.needs_helper() {
            self.state.lid_sleep_disabled = self.lid.sleep_disabled();
        }
        self.dirty = true;
    }

    pub fn stop(&mut self, reason: StopReason, _now: Timestamp) {
        let Some(old) = self.active.take() else {
            return;
        };
        self.release_assertions(&old.assertions);
        if old.lid_held {
            self.release_lid();
        }
        log::info!("keep-awake stopped: {reason:?}");
        if reason.is_automatic() {
            self.alerts.push(
                alert("Maintien éveillé arrêté", "moon.zzz")
                    .subtitle(reason.label_fr())
                    .priority(AlertPriority::High),
            );
        }
        self.state.active = None;
        self.state.last_stop = Some(reason);
        self.dirty = true;
    }

    fn release_assertions(&mut self, ids: &[AssertionId]) {
        for &id in ids {
            if let Err(e) = self.assertions.release(id) {
                log::warn!("assertion {id} release failed: {e}");
            }
        }
    }

    fn release_lid(&mut self) {
        match self.lid.release() {
            Ok(RestoreOutcome::Failed) => {
                self.state.last_error = Some(
                    "La veille capot fermé n’a pas pu être rétablie ; l’assistant réessaie.".into(),
                )
            }
            Ok(_) => {}
            // The connection is closed anyway: the helper restores after the
            // grace period.
            Err(e) => {
                log::warn!("lid release: {e:?}");
                self.state.last_error = Some(format!(
                    "{} La veille capot fermé sera rétablie par l’assistant d’ici {} s.",
                    e.message_fr(),
                    crate::lid::LEASE_SECS
                ));
            }
        }
        self.state.lid_sleep_disabled = self.lid.sleep_disabled();
    }

    pub fn on_power(&mut self, info: Option<PowerSourceInfo>, now: Timestamp) {
        self.power = info;
        if self.active.is_some()
            && let Some(r) = self.power_guard()
        {
            self.stop(r, now);
        }
    }

    pub fn on_settings(&mut self, s: PowerSettings, now: Timestamp) {
        let threshold_changed = s.stop_below_percent != self.settings.stop_below_percent;
        self.settings = s;
        if self.active.is_none() {
            return;
        }
        if let Some(r) = self.power_guard() {
            self.stop(r, now);
            return;
        }
        if threshold_changed && self.active.as_ref().is_some_and(|a| a.lid_held) {
            // Same connection: the helper only updates the lease parameters.
            if let Err(e) = self.lid.acquire(self.settings.stop_below_percent) {
                self.state.last_error = Some(e.message_fr());
                self.stop(StopReason::HelperLost, now);
            }
        }
    }

    /// Expiry, heartbeats, guards. Returns seconds until the next deadline.
    pub fn tick(&mut self, now: Timestamp) -> Option<f64> {
        let Some(a) = &self.active else {
            return None;
        };
        if a.session.ends_at.is_some_and(|e| now >= e) {
            self.stop(StopReason::Expired, now);
            return None;
        }
        if let Some(r) = self.power_guard() {
            self.stop(r, now);
            return None;
        }
        if a.lid_held && now >= a.next_heartbeat {
            match self.heartbeat_or_rejoin() {
                Ok(()) => {
                    if let Some(a) = &mut self.active {
                        a.next_heartbeat = now + HEARTBEAT_SECS;
                    }
                    let v = self.lid.sleep_disabled();
                    if v != self.state.lid_sleep_disabled {
                        self.state.lid_sleep_disabled = v;
                        self.dirty = true;
                    }
                }
                Err(e) => {
                    let reason = match &e {
                        LidError::Revoked(RevokeReason::Battery)
                        | LidError::Refused {
                            code: ErrorCode::Battery,
                            ..
                        } => StopReason::BatteryThreshold,
                        LidError::Revoked(RevokeReason::Thermal)
                        | LidError::Refused {
                            code: ErrorCode::Thermal,
                            ..
                        } => StopReason::Thermal,
                        _ => StopReason::HelperLost,
                    };
                    // The helper already restored (or will after the grace).
                    // `release` here only closes our connection and clears the
                    // marker; on a dead lease the helper answers "nothing".
                    if let Some(a) = &mut self.active {
                        a.lid_held = false;
                    }
                    let _ = self.lid.release();
                    self.stop(reason, now);
                    self.state.last_error = Some(e.message_fr());
                    self.state.lid_sleep_disabled = self.lid.sleep_disabled();
                    return None;
                }
            }
        }
        let a = self.active.as_ref()?;
        let mut next = a.session.ends_at.map(|e| e - now);
        if a.lid_held {
            let hb = (a.next_heartbeat - now).max(0.0);
            next = Some(next.map_or(hb, |n| n.min(hb)));
        }
        next.map(|n| n.max(0.0))
    }

    /// Heartbeat; if the connection dropped (helper relaunched by launchd),
    /// reconnect once and take the lease back.
    fn heartbeat_or_rejoin(&mut self) -> Result<(), LidError> {
        match self.lid.heartbeat() {
            Err(LidError::Unreachable(_)) => self
                .lid
                .acquire(self.settings.stop_below_percent)
                .map(|_| ()),
            r => r,
        }
    }

    /// App quit / module stop.
    pub fn shutdown(&mut self, now: Timestamp) {
        self.stop(StopReason::AppQuit, now);
    }

    pub fn helper_status(&self) -> HelperStatus {
        self.state.helper
    }
}
