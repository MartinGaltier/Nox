//! Keep-awake state machine. Pure: wall-clock time is passed in, side effects
//! go through [`Assertions`] and [`LidControl`] (fakes in tests).
//!
//! Adapted from vorssaint-utils (GPL-3.0-or-later), Services/KeepAwakeManager.swift, commit e916961.
//! (session lifecycle, battery protection semantics: stop when on battery at
//! or below the limit; refuse to start in that state.)

use crate::assertions::{AssertionId, AssertionKind, Assertions};
use crate::guard::{BATTERY_FLOOR_PERCENT, GuardReason, THERMAL_HEAVY};
use crate::lid::{LidControl, LidError, LidPolicy, POLL_SECS};
use crate::probe::PowerSourceInfo;
use nox_core::alert::{Alert, AlertKind, AlertPriority};
use nox_core::model::Timestamp;
use nox_core::model::power::{
    KeepAwakeDuration, KeepAwakeMode, KeepAwakeSession, LidRuleStatus, PowerSettings, PowerState,
    StopReason,
};

struct Active {
    session: KeepAwakeSession,
    assertions: Vec<AssertionId>,
    lid_held: bool,
    /// Next time the journal is checked for a guard stop.
    next_poll: Timestamp,
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

    /// A closed-lid session is running (the module then feeds [`Self::on_sensors`]).
    pub fn lid_held(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.lid_held)
    }

    pub fn set_notice(&mut self, notice: Option<String>) {
        self.state.notice = notice;
        self.dirty = true;
    }

    pub fn set_rule_busy(&mut self, busy: bool) {
        self.state.rule_busy = busy;
        self.dirty = true;
    }

    pub fn set_legacy_helper(&mut self, present: bool) {
        self.state.legacy_helper = present;
        self.dirty = true;
    }

    pub fn set_error(&mut self, e: Option<String>) {
        self.state.last_error = e;
        self.dirty = true;
    }

    /// Re-reads the rule status and `SleepDisabled`.
    pub fn refresh(&mut self) {
        self.state.lid_rule = self.lid.rule_status();
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

    fn policy(&self) -> LidPolicy {
        LidPolicy {
            only_on_ac: self.settings.only_on_ac,
            min_battery: self.settings.stop_below_percent,
        }
    }

    /// Starts a session, replacing a running one (keeping the closed-lid
    /// state when both are closed-lid, so pmset is not toggled needlessly).
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

        if mode.needs_helper() {
            // Same state when kept: only the guard parameters are refreshed.
            if let Err(e) = self.lid.acquire(self.policy()) {
                self.release_assertions(&ids);
                if keep_lid {
                    self.release_lid();
                }
                if e == LidError::NotInstalled {
                    self.refresh();
                }
                self.refuse(e.message_fr());
                return;
            }
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
            next_poll: now + POLL_SECS,
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
        if let Err(e) = self.lid.release() {
            log::warn!("lid release: {e:?}");
            self.state.last_error = Some(format!(
                "{} Nox réessaiera au prochain lancement.",
                e.message_fr()
            ));
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
        self.settings = s;
        if self.active.is_none() {
            return;
        }
        if let Some(r) = self.power_guard() {
            self.stop(r, now);
            return;
        }
        if self.active.as_ref().is_some_and(|a| a.lid_held)
            && let Err(e) = self.lid.acquire(self.policy())
        {
            self.state.last_error = Some(e.message_fr());
            self.stop(StopReason::HelperLost, now);
        }
    }

    /// Closed-lid guards that Nox runs itself, on the same thresholds as the
    /// guard process (`disablesleep` also turns off the software battery and
    /// thermal sleep protections, so both must watch). Battery: at or below
    /// 5 % on battery, whatever the user set. Thermal: heavy pressure.
    pub fn on_sensors(
        &mut self,
        power: Option<PowerSourceInfo>,
        thermal: Option<u64>,
        now: Timestamp,
    ) {
        if !self.lid_held() {
            return;
        }
        let low = power.is_some_and(|p| {
            p.on_battery
                && p.battery_percent
                    .is_some_and(|c| c <= BATTERY_FLOOR_PERCENT)
        });
        let reason = if low {
            Some(GuardReason::Battery)
        } else if thermal.is_some_and(|t| t >= THERMAL_HEAVY) {
            Some(GuardReason::Thermal)
        } else {
            None
        };
        if let Some(r) = reason {
            self.end_lid_session(r, now);
        }
    }

    /// Ends the session because the closed-lid guard tripped: releases the
    /// lid state (restored through the journal) and says why.
    fn end_lid_session(&mut self, reason: GuardReason, now: Timestamp) {
        // `stop` releases the lid state: a no-op when the guard already did it.
        self.stop(stop_reason(reason), now);
        self.state.last_error = Some(guard_message(reason));
        self.state.lid_sleep_disabled = self.lid.sleep_disabled();
    }

    /// Expiry, guard checks, journal polls. Returns seconds until the next deadline.
    pub fn tick(&mut self, now: Timestamp) -> Option<f64> {
        let a = self.active.as_ref()?;
        if a.session.ends_at.is_some_and(|e| now >= e) {
            self.stop(StopReason::Expired, now);
            return None;
        }
        if let Some(r) = self.power_guard() {
            self.stop(r, now);
            return None;
        }
        if a.lid_held && now >= a.next_poll {
            if let Some(reason) = self.lid.take_guard_stop() {
                // The guard already put lid sleep back: end the session.
                self.end_lid_session(reason, now);
                return None;
            }
            if let Some(a) = &mut self.active {
                a.next_poll = now + POLL_SECS;
            }
            let v = self.lid.sleep_disabled();
            if v != self.state.lid_sleep_disabled {
                self.state.lid_sleep_disabled = v;
                self.dirty = true;
            }
        }
        let a = self.active.as_ref()?;
        let mut next = a.session.ends_at.map(|e| e - now);
        if a.lid_held {
            let poll = (a.next_poll - now).max(0.0);
            next = Some(next.map_or(poll, |n| n.min(poll)));
        }
        next.map(|n| n.max(0.0))
    }

    /// App quit / module stop.
    pub fn shutdown(&mut self, now: Timestamp) {
        self.stop(StopReason::AppQuit, now);
    }

    pub fn lid_rule_status(&self) -> LidRuleStatus {
        self.state.lid_rule
    }
}

/// Why the session ends when the guard restored lid sleep.
fn stop_reason(r: GuardReason) -> StopReason {
    match r {
        GuardReason::Battery => StopReason::BatteryThreshold,
        GuardReason::Thermal => StopReason::Thermal,
        GuardReason::External => StopReason::HelperLost,
    }
}

fn guard_message(r: GuardReason) -> String {
    match r {
        GuardReason::Battery => "Batterie sous le seuil : veille capot fermé rétablie.".into(),
        GuardReason::Thermal => {
            "Température élevée : veille capot fermé rétablie par sécurité.".into()
        }
        GuardReason::External => "Le réglage de veille a été modifié en dehors de Nox.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::RestoreOutcome;
    use crate::lid::LidPolicy;
    use nox_core::Reading;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct AState {
        next: AssertionId,
        live: Vec<(AssertionId, AssertionKind)>,
        fail: bool,
    }
    #[derive(Clone, Default)]
    struct FakeAssertions(Arc<Mutex<AState>>);
    impl FakeAssertions {
        fn live(&self) -> Vec<AssertionKind> {
            self.0.lock().unwrap().live.iter().map(|l| l.1).collect()
        }
    }
    impl Assertions for FakeAssertions {
        fn create(&mut self, kind: AssertionKind) -> Result<AssertionId, String> {
            let mut s = self.0.lock().unwrap();
            if s.fail {
                return Err("denied".into());
            }
            s.next += 1;
            let id = s.next;
            s.live.push((id, kind));
            Ok(id)
        }
        fn release(&mut self, id: AssertionId) -> Result<(), String> {
            self.0.lock().unwrap().live.retain(|l| l.0 != id);
            Ok(())
        }
    }

    /// A pmset-like fake: `disabled` is the system setting.
    struct LState {
        status: LidRuleStatus,
        disabled: bool,
        held: bool,
        acquires: u32,
        releases: u32,
        polls: u32,
        /// What the guard reports at the next poll.
        guard_stop: Option<GuardReason>,
        acquire_error: Option<LidError>,
    }
    #[derive(Clone)]
    struct FakeLid(Arc<Mutex<LState>>);
    impl FakeLid {
        fn new(status: LidRuleStatus) -> Self {
            Self(Arc::new(Mutex::new(LState {
                status,
                disabled: false,
                held: false,
                acquires: 0,
                releases: 0,
                polls: 0,
                guard_stop: None,
                acquire_error: None,
            })))
        }
        fn with<R>(&self, f: impl FnOnce(&mut LState) -> R) -> R {
            f(&mut self.0.lock().unwrap())
        }
    }
    impl LidControl for FakeLid {
        fn rule_status(&mut self) -> LidRuleStatus {
            self.with(|s| s.status)
        }
        fn sleep_disabled(&mut self) -> Reading<bool> {
            Reading::Measured(self.with(|s| s.disabled))
        }
        fn acquire(&mut self, _policy: LidPolicy) -> Result<bool, LidError> {
            self.with(|s| {
                s.acquires += 1;
                if s.status != LidRuleStatus::Installed {
                    return Err(LidError::NotInstalled);
                }
                if let Some(e) = s.acquire_error.take() {
                    return Err(e);
                }
                s.held = true;
                s.disabled = true;
                Ok(true)
            })
        }
        fn take_guard_stop(&mut self) -> Option<GuardReason> {
            self.with(|s| {
                s.polls += 1;
                let r = s.guard_stop.take();
                if r.is_some() {
                    s.held = false;
                    s.disabled = false;
                }
                r
            })
        }
        fn release(&mut self) -> Result<RestoreOutcome, LidError> {
            self.with(|s| {
                s.releases += 1;
                let was = s.held;
                s.held = false;
                s.disabled = false;
                Ok(if was {
                    RestoreOutcome::Restored
                } else {
                    RestoreOutcome::Untouched
                })
            })
        }
    }

    type E = Engine<FakeAssertions, FakeLid>;

    fn engine(status: LidRuleStatus) -> (E, FakeAssertions, FakeLid) {
        let a = FakeAssertions::default();
        let l = FakeLid::new(status);
        let mut e = Engine::new(a.clone(), l.clone(), PowerSettings::default());
        e.on_power(
            Some(PowerSourceInfo {
                on_battery: false,
                battery_percent: Some(80),
            }),
            0.0,
        );
        e.refresh();
        (e, a, l)
    }

    const T0: f64 = 1_000_000.0;
    const M15: KeepAwakeDuration = KeepAwakeDuration::Minutes(15);

    #[test]
    fn system_mode_holds_one_system_assertion() {
        let (mut e, a, l) = engine(LidRuleStatus::NotInstalled);
        e.start(KeepAwakeMode::System, M15, T0);
        assert_eq!(a.live(), vec![AssertionKind::System]);
        assert_eq!(l.with(|s| s.acquires), 0, "no lid state for idle modes");
        let s = e.state().active.clone().unwrap();
        assert_eq!(s.ends_at, Some(T0 + 900.0));
        assert_eq!(e.take_alerts().len(), 1, "start alert");
        e.stop(StopReason::User, T0 + 1.0);
        assert!(a.live().is_empty());
        assert_eq!(e.state().last_stop, Some(StopReason::User));
        assert!(e.take_alerts().is_empty(), "no alert for a manual stop");
    }

    #[test]
    fn display_mode_holds_both() {
        let (mut e, a, _) = engine(LidRuleStatus::NotInstalled);
        e.start(
            KeepAwakeMode::SystemAndDisplay,
            KeepAwakeDuration::Unlimited,
            T0,
        );
        assert_eq!(
            a.live(),
            vec![AssertionKind::System, AssertionKind::Display]
        );
        assert_eq!(e.state().active.as_ref().unwrap().ends_at, None);
        assert_eq!(e.tick(T0 + 1e6), None, "unlimited: no deadline");
        assert!(e.is_active());
        e.shutdown(T0);
        assert!(a.live().is_empty());
        assert_eq!(e.state().last_stop, Some(StopReason::AppQuit));
    }

    #[test]
    fn expires_exactly() {
        let (mut e, a, _) = engine(LidRuleStatus::NotInstalled);
        e.start(KeepAwakeMode::System, KeepAwakeDuration::Minutes(30), T0);
        let wait = e.tick(T0 + 100.0).unwrap();
        assert!((wait - 1700.0).abs() < 1e-6);
        e.tick(T0 + 1799.9);
        assert!(e.is_active());
        e.take_alerts();
        e.tick(T0 + 1800.0);
        assert!(!e.is_active());
        assert!(a.live().is_empty());
        assert_eq!(e.state().last_stop, Some(StopReason::Expired));
        assert_eq!(e.take_alerts().len(), 1, "automatic stop alert");
    }

    #[test]
    fn closed_lid_needs_the_rule_and_leaks_nothing() {
        let (mut e, a, l) = engine(LidRuleStatus::NotInstalled);
        e.start(KeepAwakeMode::ClosedLid, M15, T0);
        assert!(!e.is_active());
        assert!(a.live().is_empty(), "assertion released on refusal");
        assert!(
            e.state()
                .last_error
                .as_deref()
                .unwrap()
                .contains("autorisation")
        );
        assert!(!l.with(|s| s.disabled));
    }

    #[test]
    fn closed_lid_sets_polls_and_restores_at_stop() {
        let (mut e, a, l) = engine(LidRuleStatus::Installed);
        e.start(KeepAwakeMode::ClosedLid, KeepAwakeDuration::Minutes(60), T0);
        assert_eq!(a.live(), vec![AssertionKind::System]);
        assert!(l.with(|s| s.disabled));
        assert_eq!(e.state().lid_sleep_disabled, Reading::Measured(true));
        let wait = e.tick(T0 + 1.0).unwrap();
        assert!(wait <= POLL_SECS);
        e.tick(T0 + 15.0);
        e.tick(T0 + 30.0);
        assert_eq!(l.with(|s| s.polls), 2);
        e.stop(StopReason::User, T0 + 31.0);
        assert!(!l.with(|s| s.disabled), "restored at stop");
        assert_eq!(l.with(|s| s.releases), 1);
        assert_eq!(e.state().lid_sleep_disabled, Reading::Measured(false));
    }

    #[test]
    fn guard_battery_stop_ends_session_with_reason() {
        let (mut e, a, l) = engine(LidRuleStatus::Installed);
        e.start(KeepAwakeMode::ClosedLid, M15, T0);
        l.with(|s| s.guard_stop = Some(GuardReason::Battery));
        e.tick(T0 + 15.0);
        assert!(!e.is_active());
        assert_eq!(e.state().last_stop, Some(StopReason::BatteryThreshold));
        assert!(a.live().is_empty());
        assert!(
            e.state()
                .last_error
                .as_deref()
                .unwrap()
                .contains("Batterie")
        );
        assert!(
            !l.with(|s| s.disabled),
            "restored (by the guard, then released)"
        );
    }

    #[test]
    fn guard_thermal_stop_and_external_change_map_to_their_reason() {
        let (mut e, _, l) = engine(LidRuleStatus::Installed);
        e.start(KeepAwakeMode::ClosedLid, M15, T0);
        l.with(|s| s.guard_stop = Some(GuardReason::Thermal));
        e.tick(T0 + 15.0);
        assert_eq!(e.state().last_stop, Some(StopReason::Thermal));

        e.start(KeepAwakeMode::ClosedLid, M15, T0 + 20.0);
        l.with(|s| s.guard_stop = Some(GuardReason::External));
        e.tick(T0 + 35.0);
        assert_eq!(e.state().last_stop, Some(StopReason::HelperLost));
    }

    #[test]
    fn closed_lid_floor_and_thermal_guards_hold_even_with_threshold_off() {
        let (mut e, _, l) = engine(LidRuleStatus::Installed);
        e.on_settings(
            PowerSettings {
                stop_below_percent: 0,
                ..Default::default()
            },
            T0,
        );
        let battery = |pct| {
            Some(PowerSourceInfo {
                on_battery: true,
                battery_percent: Some(pct),
            })
        };
        e.start(KeepAwakeMode::ClosedLid, M15, T0);
        e.on_sensors(battery(6), None, T0 + 1.0);
        assert!(e.is_active(), "above the 5 % floor");
        e.on_sensors(battery(5), None, T0 + 2.0);
        assert!(!e.is_active());
        assert_eq!(e.state().last_stop, Some(StopReason::BatteryThreshold));
        assert!(!l.with(|s| s.disabled), "restored by Nox at once");

        e.start(KeepAwakeMode::ClosedLid, M15, T0 + 3.0);
        e.on_sensors(None, Some(1), T0 + 4.0);
        assert!(e.is_active(), "moderate pressure is fine");
        e.on_sensors(None, Some(2), T0 + 5.0);
        assert_eq!(e.state().last_stop, Some(StopReason::Thermal));
        assert!(!e.is_active());
    }

    #[test]
    fn sensors_are_ignored_outside_closed_lid() {
        let (mut e, _, _) = engine(LidRuleStatus::Installed);
        e.start(KeepAwakeMode::System, M15, T0);
        e.on_sensors(
            Some(PowerSourceInfo {
                on_battery: true,
                battery_percent: Some(1),
            }),
            Some(4),
            T0 + 1.0,
        );
        assert!(e.is_active());
    }

    #[test]
    fn switching_closed_lid_sessions_keeps_the_state() {
        let (mut e, a, l) = engine(LidRuleStatus::Installed);
        e.start(KeepAwakeMode::ClosedLid, M15, T0);
        e.start(
            KeepAwakeMode::ClosedLid,
            KeepAwakeDuration::Minutes(120),
            T0 + 5.0,
        );
        assert_eq!(l.with(|s| s.releases), 0, "no 1→0→1 toggle");
        assert!(l.with(|s| s.disabled));
        assert_eq!(a.live().len(), 1);
        e.start(KeepAwakeMode::System, M15, T0 + 6.0);
        assert_eq!(l.with(|s| s.releases), 1);
        assert!(!l.with(|s| s.disabled));
        assert_eq!(a.live(), vec![AssertionKind::System]);
    }

    #[test]
    fn only_on_ac() {
        let (mut e, a, _) = engine(LidRuleStatus::NotInstalled);
        e.on_settings(
            PowerSettings {
                only_on_ac: true,
                ..Default::default()
            },
            T0,
        );
        e.start(KeepAwakeMode::System, M15, T0);
        assert!(e.is_active());
        e.on_power(
            Some(PowerSourceInfo {
                on_battery: true,
                battery_percent: Some(90),
            }),
            T0 + 1.0,
        );
        assert!(!e.is_active());
        assert_eq!(e.state().last_stop, Some(StopReason::OnBattery));
        assert!(a.live().is_empty());
        e.start(KeepAwakeMode::System, M15, T0 + 2.0);
        assert!(!e.is_active(), "refused on battery");
        assert!(e.state().last_error.is_some());
    }

    #[test]
    fn battery_threshold() {
        let (mut e, _, _) = engine(LidRuleStatus::NotInstalled);
        e.start(KeepAwakeMode::System, M15, T0);
        let p = |on_battery, pct| {
            Some(PowerSourceInfo {
                on_battery,
                battery_percent: Some(pct),
            })
        };
        e.on_power(p(false, 10), T0 + 1.0);
        assert!(e.is_active(), "on AC the threshold does not apply");
        e.on_power(p(true, 21), T0 + 2.0);
        assert!(e.is_active());
        e.on_power(p(true, 20), T0 + 3.0);
        assert!(!e.is_active());
        assert_eq!(e.state().last_stop, Some(StopReason::BatteryThreshold));
        e.start(KeepAwakeMode::System, M15, T0 + 4.0);
        assert!(!e.is_active(), "refused below threshold");
        // Threshold 0 = off.
        e.on_settings(
            PowerSettings {
                stop_below_percent: 0,
                ..Default::default()
            },
            T0 + 5.0,
        );
        e.start(KeepAwakeMode::System, M15, T0 + 6.0);
        assert!(e.is_active());
    }

    #[test]
    fn enabling_only_on_ac_while_on_battery_stops() {
        let (mut e, _, _) = engine(LidRuleStatus::NotInstalled);
        e.on_power(
            Some(PowerSourceInfo {
                on_battery: true,
                battery_percent: Some(90),
            }),
            T0,
        );
        e.start(KeepAwakeMode::System, M15, T0);
        assert!(e.is_active());
        e.on_settings(
            PowerSettings {
                only_on_ac: true,
                ..Default::default()
            },
            T0 + 1.0,
        );
        assert_eq!(e.state().last_stop, Some(StopReason::OnBattery));
    }

    #[test]
    fn assertion_failure_is_reported_not_hidden() {
        let (mut e, a, _) = engine(LidRuleStatus::NotInstalled);
        a.0.lock().unwrap().fail = true;
        e.start(KeepAwakeMode::SystemAndDisplay, M15, T0);
        assert!(!e.is_active());
        assert!(e.state().active.is_none());
        assert!(e.state().last_error.as_deref().unwrap().contains("denied"));
    }

    #[test]
    fn toggle_uses_defaults() {
        let (mut e, a, _) = engine(LidRuleStatus::NotInstalled);
        e.toggle(T0);
        let s = e.state().active.clone().unwrap();
        assert_eq!(s.mode, KeepAwakeMode::System);
        assert_eq!(s.duration, KeepAwakeDuration::Minutes(60));
        e.toggle(T0 + 1.0);
        assert!(a.live().is_empty());
    }
}
