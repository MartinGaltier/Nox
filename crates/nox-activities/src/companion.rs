//! Companion (D11): a mood derived from what the island already knows —
//! music → dancing, an agent working → busy, a task finished → celebrating,
//! low battery → worried, no input for a while → sleepy — with a short caption
//! and one-shot reactions. The UI (nox-ui) draws and animates it.
//!
//! Idea adapted from vorssaint-utils (GPL-3.0-or-later),
//! Services/Notch/NotchMascotSupport.swift (moods, reactions), commit e916961.

use nox_core::model::Timestamp;
use nox_core::model::agents::SessionPhase;
use nox_core::model::companion::{
    CompanionMood, CompanionReaction, CompanionReactionKind, CompanionSettings, CompanionState,
    MoodCause,
};
use nox_core::model::timers::{FocusPhase, TimerRun};
use nox_core::model::watch::WatchOutcome;
use nox_core::{
    Command, Hub, IslandState, Module, ModuleContext, ModuleError, ModuleId, Settings,
    Subscription, Worker,
};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A celebration lasts this long.
const CELEBRATE_SECS: f64 = 20.0;
/// Longest sleep: a backstop; changes arrive from the hub and deadlines are exact.
const MAX_SLEEP: f64 = 60.0;

/// The facts the mood is derived from (read from the hub).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inputs {
    pub music_playing: bool,
    /// (provider label, phase, session id + turn start) per visible session.
    pub sessions: Vec<(String, SessionPhase, String)>,
    pub finished_timers: usize,
    pub focus_work: bool,
    pub watch_outcome: Option<WatchOutcome>,
    /// Battery percent when on battery.
    pub battery_on_battery: Option<f64>,
    pub low_battery: f64,
    pub idle_secs: f64,
}

impl Inputs {
    pub fn read(s: &IslandState, low_battery: f64, idle_secs: f64) -> Self {
        let sessions = s
            .agents
            .as_ref()
            .map(|a| {
                a.providers
                    .iter()
                    .flat_map(|p| {
                        p.sessions.iter().map(|x| {
                            (
                                p.provider.label().to_string(),
                                x.phase,
                                format!("{}:{:?}", x.id, x.turn_started_at.map(|t| t as i64)),
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            music_playing: s
                .music
                .as_ref()
                .and_then(|m| m.now_playing.as_ref())
                .is_some_and(|n| n.playing),
            sessions,
            finished_timers: s.timers.as_ref().map_or(0, |t| {
                t.timers
                    .iter()
                    .filter(|x| x.run == TimerRun::Finished)
                    .count()
            }),
            focus_work: s.timers.as_ref().is_some_and(|t| {
                t.focus
                    .as_ref()
                    .is_some_and(|f| f.phase == FocusPhase::Work && !f.paused)
            }),
            watch_outcome: s.watch.as_ref().and_then(|w| w.outcome),
            battery_on_battery: s
                .metrics
                .as_ref()
                .and_then(|m| m.snapshot.battery.value())
                .filter(|b| !b.on_ac)
                .map(|b| b.percent),
            low_battery,
            idle_secs,
        }
    }
}

#[derive(Default)]
pub struct Engine {
    state: CompanionState,
    celebrate_until: Timestamp,
    celebrate_caption: Option<String>,
    seen_finished: HashSet<String>,
    finished_timers: usize,
    watch_outcome: Option<WatchOutcome>,
    next_reaction: u64,
    primed: bool,
}

impl Engine {
    fn react(&mut self, kind: CompanionReactionKind, now: Timestamp, s: &CompanionSettings) {
        if !s.reactions {
            return;
        }
        self.next_reaction += 1;
        self.state.reaction = Some(CompanionReaction {
            kind,
            id: self.next_reaction,
            at: now,
        });
    }

    /// New state if anything visible changed.
    pub fn update(
        &mut self,
        i: &Inputs,
        s: &CompanionSettings,
        now: Timestamp,
    ) -> Option<CompanionState> {
        let before = self.state.clone();
        // Events (edges), ignored on the very first look so history is not news.
        let finished: Vec<&(String, SessionPhase, String)> = i
            .sessions
            .iter()
            .filter(|x| x.1 == SessionPhase::Finished)
            .collect();
        for f in &finished {
            if self.seen_finished.insert(f.2.clone()) && self.primed {
                self.celebrate_until = now + CELEBRATE_SECS;
                self.celebrate_caption = Some(format!("{} a terminé !", f.0));
                self.react(CompanionReactionKind::Celebrate, now, s);
            }
        }
        if self.seen_finished.len() > 256 {
            self.seen_finished.clear();
        }
        if i.finished_timers > self.finished_timers && self.primed {
            self.celebrate_until = now + CELEBRATE_SECS;
            self.celebrate_caption = Some("Minuteur terminé".into());
            self.react(CompanionReactionKind::Celebrate, now, s);
        }
        self.finished_timers = i.finished_timers;
        if i.watch_outcome != self.watch_outcome && self.primed {
            match i.watch_outcome {
                Some(WatchOutcome::Lost) => self.react(CompanionReactionKind::Confused, now, s),
                Some(_) => self.react(CompanionReactionKind::Perk, now, s),
                None => {}
            }
        }
        self.watch_outcome = i.watch_outcome;

        let working: Vec<&String> = i
            .sessions
            .iter()
            .filter(|x| x.1 == SessionPhase::Working)
            .map(|x| &x.0)
            .collect();
        let waiting = i
            .sessions
            .iter()
            .find(|x| x.1 == SessionPhase::WaitingForUser)
            .map(|x| &x.0);
        let sleepy = s.sleepy_after_secs > 0 && i.idle_secs >= s.sleepy_after_secs as f64;

        let (mood, cause, caption, intensity) =
            if let Some(b) = i.battery_on_battery.filter(|b| *b <= i.low_battery) {
                (
                    CompanionMood::Worried,
                    MoodCause::Battery,
                    Some(format!("Batterie faible ({b:.0} %)")),
                    (1.0 - b / i.low_battery.max(1.0)).clamp(0.3, 1.0),
                )
            } else if now < self.celebrate_until {
                (
                    CompanionMood::Celebrating,
                    if self.celebrate_caption.as_deref() == Some("Minuteur terminé") {
                        MoodCause::Timer
                    } else {
                        MoodCause::AgentFinished
                    },
                    self.celebrate_caption.clone(),
                    1.0,
                )
            } else if let Some(p) = waiting {
                (
                    CompanionMood::Busy,
                    MoodCause::AgentWaiting,
                    Some(format!("{p} vous attend")),
                    0.4,
                )
            } else if !working.is_empty() {
                let caption = if working.len() == 1 {
                    format!("{} travaille…", working[0])
                } else {
                    format!("{} agents travaillent…", working.len())
                };
                (
                    CompanionMood::Busy,
                    MoodCause::Agent,
                    Some(caption),
                    (working.len() as f64 / 3.0).clamp(0.34, 1.0),
                )
            } else if i.music_playing && !sleepy {
                (CompanionMood::Dancing, MoodCause::Music, None, 0.7)
            } else if i.focus_work {
                (
                    CompanionMood::Busy,
                    MoodCause::Focus,
                    Some("Concentration".into()),
                    0.3,
                )
            } else if sleepy {
                (CompanionMood::Sleepy, MoodCause::Inactivity, None, 0.5)
            } else {
                (CompanionMood::Idle, MoodCause::Nothing, None, 0.2)
            };
        if self.primed && before.mood != mood {
            if mood == CompanionMood::Sleepy {
                self.react(CompanionReactionKind::Yawn, now, s);
            } else if before.mood == CompanionMood::Sleepy {
                self.react(CompanionReactionKind::WakeUp, now, s);
            }
        }
        if before.mood != mood || before.cause != cause {
            self.state.since = now;
        }
        self.state.mood = mood;
        self.state.cause = cause;
        self.state.caption = caption;
        self.state.intensity = intensity;
        self.state.tempo_bpm = None;
        let first = !self.primed;
        self.primed = true;
        (first || self.state != before).then(|| self.state.clone())
    }
}

/// How long the loop may sleep: the mood can only change without a hub change
/// when inactivity reaches its threshold or a celebration runs out.
fn next_check(idle: f64, s: &CompanionSettings, celebrate_until: f64, now: f64) -> Duration {
    let mut wait = MAX_SLEEP;
    if s.sleepy_after_secs > 0 && idle < s.sleepy_after_secs as f64 {
        wait = wait.min(s.sleepy_after_secs as f64 - idle);
    }
    if celebrate_until > now {
        wait = wait.min(celebrate_until - now);
    }
    // Never spin on rounding at a threshold.
    Duration::from_secs_f64(wait.max(1.0))
}

/// Seconds since the last keyboard/mouse/trackpad event (no permission needed).
fn idle_secs() -> f64 {
    // kCGAnyInputEventType = ~0
    CGEventSource::seconds_since_last_event_type(
        CGEventSourceStateID::CombinedSessionState,
        CGEventType(u32::MAX),
    )
}

pub struct CompanionModule {
    worker: Option<Worker>,
    hub: Option<Hub>,
    /// Wakes the loop on every hub change (no polling).
    subscription: Option<Subscription>,
    settings: Arc<Mutex<Arc<Settings>>>,
}

impl CompanionModule {
    pub fn new() -> Self {
        Self {
            worker: None,
            hub: None,
            subscription: None,
            settings: Arc::new(Mutex::new(Arc::new(Settings::default()))),
        }
    }
}

impl Default for CompanionModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for CompanionModule {
    fn id(&self) -> ModuleId {
        ModuleId::Companion
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = ctx.settings.clone();
        let hub = ctx.hub.clone();
        let settings = self.settings.clone();
        let worker = Worker::spawn("companion", move |token| {
            let mut engine = Engine::default();
            let mut last_gen = u64::MAX;
            let mut last_sleepy = None;
            loop {
                let s = settings.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let idle = idle_secs();
                let sleepy = idle >= s.companion.sleepy_after_secs as f64;
                let generation = hub.generation();
                // Only look again when the island changed, inactivity crossed
                // its threshold or a celebration is running out.
                if generation != last_gen
                    || last_sleepy != Some(sleepy)
                    || engine.celebrate_until > 0.0
                {
                    let inputs = hub
                        .read(|st| Inputs::read(st, f64::from(s.metrics.low_battery_alert), idle));
                    let now = nox_core::model::now();
                    if let Some(state) = engine.update(&inputs, &s.companion, now) {
                        hub.update(|st| st.companion = Some(state));
                    }
                    if engine.celebrate_until <= now {
                        engine.celebrate_until = 0.0;
                    }
                    last_gen = hub.generation();
                    last_sleepy = Some(sleepy);
                }
                let wait = next_check(
                    idle,
                    &s.companion,
                    engine.celebrate_until,
                    nox_core::model::now(),
                );
                if token.sleep(wait) {
                    break;
                }
            }
        });
        let token = worker.token().clone();
        self.subscription = Some(ctx.hub.subscribe(move || token.nudge()));
        self.worker = Some(worker);
        self.hub = Some(ctx.hub);
        Ok(())
    }

    fn stop(&mut self) {
        self.subscription.take();
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        if let Some(hub) = self.hub.take() {
            hub.update(|s| s.companion = None);
        }
    }

    fn command(&mut self, _cmd: Command) {}

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = settings.clone();
        if let Some(w) = &self.worker {
            w.token().nudge();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_check_sleeps_to_the_nearest_deadline() {
        let s = CompanionSettings {
            sleepy_after_secs: 30,
            ..Default::default()
        };
        let secs = |d: Duration| d.as_secs_f64().round() as u64;
        // Inactivity reaches its threshold in 20 s.
        assert_eq!(secs(next_check(10.0, &s, 0.0, 1_000.0)), 20);
        // A celebration ends in 5 s, before the threshold.
        assert_eq!(secs(next_check(10.0, &s, 1_005.0, 1_000.0)), 5);
        // Already sleepy with nothing running: only the backstop.
        assert_eq!(secs(next_check(400.0, &s, 0.0, 1_000.0)), 60);
        let off = CompanionSettings {
            sleepy_after_secs: 0,
            ..Default::default()
        };
        assert_eq!(secs(next_check(0.0, &off, 0.0, 1_000.0)), 60);
    }

    fn base() -> Inputs {
        Inputs {
            low_battery: 15.0,
            ..Default::default()
        }
    }

    #[test]
    fn moods_by_priority() {
        let s = CompanionSettings::default();
        let mut e = Engine::default();
        let st = e.update(&base(), &s, 0.0).unwrap();
        assert_eq!(st.mood, CompanionMood::Idle);
        assert!(
            e.update(&base(), &s, 1.0).is_none(),
            "no change, no publish"
        );

        let music = Inputs {
            music_playing: true,
            ..base()
        };
        assert_eq!(
            e.update(&music, &s, 2.0).unwrap().mood,
            CompanionMood::Dancing
        );

        let busy = Inputs {
            sessions: vec![("Claude".into(), SessionPhase::Working, "a".into())],
            ..music.clone()
        };
        let st = e.update(&busy, &s, 3.0).unwrap();
        assert_eq!(st.mood, CompanionMood::Busy);
        assert_eq!(st.caption.as_deref(), Some("Claude travaille…"));

        let low = Inputs {
            battery_on_battery: Some(9.0),
            ..busy.clone()
        };
        let st = e.update(&low, &s, 4.0).unwrap();
        assert_eq!(st.mood, CompanionMood::Worried);
        assert_eq!(st.cause, MoodCause::Battery);

        let sleepy = Inputs {
            idle_secs: 999.0,
            ..base()
        };
        let st = e.update(&sleepy, &s, 5.0).unwrap();
        assert_eq!(st.mood, CompanionMood::Sleepy);
        assert_eq!(st.reaction.unwrap().kind, CompanionReactionKind::Yawn);
        let st = e.update(&base(), &s, 6.0).unwrap();
        assert_eq!(st.reaction.unwrap().kind, CompanionReactionKind::WakeUp);
    }

    #[test]
    fn finishing_celebrates_once_then_calms_down() {
        let s = CompanionSettings::default();
        let mut e = Engine::default();
        // A session already finished at first look is history.
        let old = Inputs {
            sessions: vec![("Codex".into(), SessionPhase::Finished, "old".into())],
            ..base()
        };
        assert_eq!(e.update(&old, &s, 0.0).unwrap().mood, CompanionMood::Idle);
        let new = Inputs {
            sessions: vec![
                ("Codex".into(), SessionPhase::Finished, "old".into()),
                ("Claude".into(), SessionPhase::Finished, "new".into()),
            ],
            ..base()
        };
        let st = e.update(&new, &s, 10.0).unwrap();
        assert_eq!(st.mood, CompanionMood::Celebrating);
        assert_eq!(st.caption.as_deref(), Some("Claude a terminé !"));
        let r = st.reaction.unwrap();
        assert_eq!(r.kind, CompanionReactionKind::Celebrate);
        // Same inputs later: no new reaction, back to idle after the celebration.
        let st = e.update(&new, &s, 10.0 + CELEBRATE_SECS).unwrap();
        assert_eq!(st.mood, CompanionMood::Idle);
        assert_eq!(st.reaction.unwrap().id, r.id);

        // Timer rings and watch triggers react too; reactions can be turned off.
        let quiet = CompanionSettings {
            reactions: false,
            ..Default::default()
        };
        let timer = Inputs {
            finished_timers: 1,
            ..base()
        };
        let st = e.update(&timer, &quiet, 100.0).unwrap();
        assert_eq!(st.cause, MoodCause::Timer);
        assert_eq!(st.reaction.unwrap().id, r.id, "no reaction when disabled");
    }

    #[test]
    fn idle_input_is_readable() {
        assert!(idle_secs() >= 0.0);
    }
}
