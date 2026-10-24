//! Which live activity the closed island shows around the camera.

use nox_core::IslandState;
use nox_core::model::agents::{AgentProvider, SessionPhase};
use nox_core::model::companion::CompanionMood;
use nox_core::model::downloads::DownloadStatus;
use nox_core::model::power::KeepAwakeMode;
use nox_core::model::timers::{FocusPhase, TimerRun};
use nox_core::settings::{IdleContent, Settings};

#[derive(Debug, Clone, PartialEq)]
pub enum Activity {
    AgentWaiting {
        provider: AgentProvider,
        project: Option<String>,
    },
    Music {
        artwork_id: u64,
        has_artwork: bool,
        playing: bool,
        title: String,
        levels: Vec<f32>,
        levels_measured: bool,
    },
    Timer {
        label: String,
        remaining: f64,
        total: f64,
        paused: bool,
        focus: Option<FocusPhase>,
    },
    Stopwatch {
        elapsed: f64,
    },
    AgentWorking {
        provider: AgentProvider,
        sessions: usize,
        activity: Option<String>,
    },
    KeepAwake {
        mode: KeepAwakeMode,
        remaining: Option<f64>,
    },
    Download {
        name: String,
        progress: Option<f64>,
    },
    Battery {
        percent: f64,
        charging: bool,
        low: bool,
    },
    Companion {
        mood: CompanionMood,
        caption: Option<String>,
    },
}

impl Activity {
    /// Width of each wing beside the camera.
    pub fn wing(&self) -> f64 {
        match self {
            Activity::Music { .. } => 46.0,
            Activity::Timer { .. } | Activity::Stopwatch { .. } => 64.0,
            Activity::AgentWaiting { .. } | Activity::AgentWorking { .. } => 58.0,
            Activity::KeepAwake { .. } => 56.0,
            Activity::Download { .. } => 50.0,
            Activity::Battery { .. } => 54.0,
            Activity::Companion { .. } => 40.0,
        }
    }

    /// How often the strip must be redrawn (seconds), `None` = only on data change.
    pub fn refresh_interval(&self) -> Option<f64> {
        match self {
            Activity::Timer { paused: false, .. } | Activity::Stopwatch { .. } => Some(1.0),
            Activity::KeepAwake {
                remaining: Some(_), ..
            } => Some(30.0),
            Activity::Music {
                playing: true,
                levels_measured: false,
                ..
            } => Some(0.1),
            _ => None,
        }
    }
}

fn countdown_remaining(remaining: f64, observed_at: f64, running: bool, now: f64) -> f64 {
    if running {
        (remaining - (now - observed_at).max(0.0)).max(0.0)
    } else {
        remaining.max(0.0)
    }
}

/// Picks the highest-priority activity. Order: agent waiting for input,
/// music, timer, agent working, keep-awake, download, low battery, companion,
/// then the configured idle content.
pub fn select(state: &IslandState, settings: &Settings, now: f64) -> Option<Activity> {
    let sessions = || {
        state
            .agents
            .iter()
            .flat_map(|a| a.providers.iter())
            .flat_map(|p| p.sessions.iter())
    };

    if let Some(s) = sessions().find(|s| s.phase == SessionPhase::WaitingForUser) {
        return Some(Activity::AgentWaiting {
            provider: s.provider,
            project: s.project.clone(),
        });
    }

    let music = state.music.as_ref().and_then(|m| {
        m.now_playing.as_ref().map(|np| Activity::Music {
            artwork_id: np.artwork_id,
            has_artwork: np.artwork.is_some(),
            playing: np.playing,
            title: np.title.clone(),
            levels: m.levels.clone(),
            levels_measured: m.levels_measured,
        })
    });
    if let Some(a @ Activity::Music { playing: true, .. }) = &music {
        return Some(a.clone());
    }

    if let Some(t) = &state.timers {
        if let Some(f) = &t.focus {
            return Some(Activity::Timer {
                label: String::new(),
                remaining: countdown_remaining(f.remaining_secs, f.observed_at, !f.paused, now),
                total: f.phase_secs,
                paused: f.paused,
                focus: Some(f.phase),
            });
        }
        if let Some(c) = t
            .timers
            .iter()
            .filter(|c| c.run != TimerRun::Finished)
            .min_by(|a, b| {
                let ra = countdown_remaining(
                    a.remaining_secs,
                    a.observed_at,
                    a.run == TimerRun::Running,
                    now,
                );
                let rb = countdown_remaining(
                    b.remaining_secs,
                    b.observed_at,
                    b.run == TimerRun::Running,
                    now,
                );
                ra.total_cmp(&rb)
            })
        {
            return Some(Activity::Timer {
                label: c.label.clone(),
                remaining: countdown_remaining(
                    c.remaining_secs,
                    c.observed_at,
                    c.run == TimerRun::Running,
                    now,
                ),
                total: c.duration_secs,
                paused: c.run == TimerRun::Paused,
                focus: None,
            });
        }
        if t.stopwatch.running {
            return Some(Activity::Stopwatch {
                elapsed: t.stopwatch.elapsed_secs + (now - t.stopwatch.observed_at).max(0.0),
            });
        }
    }

    let working: Vec<_> = sessions()
        .filter(|s| s.phase == SessionPhase::Working)
        .collect();
    if let Some(first) = working.first() {
        return Some(Activity::AgentWorking {
            provider: first.provider,
            sessions: working.len(),
            activity: first.activity.clone(),
        });
    }

    if let Some(p) = &state.power
        && let Some(s) = &p.active
    {
        return Some(Activity::KeepAwake {
            mode: s.mode,
            remaining: s.ends_at.map(|e| (e - now).max(0.0)),
        });
    }

    if let Some(d) = state.downloads.as_ref().and_then(|d| {
        d.items
            .iter()
            .filter(|i| i.status == DownloadStatus::InProgress)
            .max_by(|a, b| a.updated_at.total_cmp(&b.updated_at))
    }) {
        return Some(Activity::Download {
            name: d.file_name.clone(),
            progress: d.progress,
        });
    }

    let battery = state
        .metrics
        .as_ref()
        .and_then(|m| m.snapshot.battery.value())
        .map(|b| {
            let threshold = settings.metrics.low_battery_alert as f64;
            Activity::Battery {
                percent: b.percent,
                charging: b.charging,
                low: threshold > 0.0 && !b.on_ac && b.percent <= threshold,
            }
        });
    if let Some(b @ Activity::Battery { low: true, .. }) = &battery {
        return Some(b.clone());
    }

    if settings.companion.enabled_in_compact
        && let Some(c) = &state.companion
        && c.mood != CompanionMood::Idle
    {
        return Some(Activity::Companion {
            mood: c.mood,
            caption: c.caption.clone(),
        });
    }

    match settings.island.idle_content {
        IdleContent::Nothing => None,
        IdleContent::Battery => battery,
        IdleContent::Music => music,
        IdleContent::Agents => sessions().next().map(|s| Activity::AgentWorking {
            provider: s.provider,
            sessions: 0,
            activity: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_core::Reading;
    use nox_core::model::agents::{AgentSession, AgentsState, ProviderStatus};
    use nox_core::model::metrics::{BatteryInfo, MetricsState};
    use nox_core::model::music::{MusicState, NowPlaying};
    use nox_core::model::power::{KeepAwakeSession, PowerState};
    use nox_core::model::timers::{CountdownTimer, TimersState};

    fn agents(phase: SessionPhase) -> AgentsState {
        AgentsState {
            providers: vec![ProviderStatus {
                provider: AgentProvider::Claude,
                detected: true,
                sessions: vec![AgentSession {
                    id: "s".into(),
                    provider: AgentProvider::Claude,
                    project: Some("nox".into()),
                    model: None,
                    phase,
                    activity: Some("Edit".into()),
                    started_at: 0.0,
                    last_event_at: 0.0,
                }],
                limits: Reading::pending(),
                tokens_today: Reading::pending(),
                cost_today_usd: Reading::pending(),
                note: None,
            }],
        }
    }

    fn playing() -> MusicState {
        MusicState {
            now_playing: Some(NowPlaying {
                title: "Song".into(),
                playing: true,
                rate: 1.0,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn battery(percent: f64, on_ac: bool) -> MetricsState {
        let mut m = MetricsState::default();
        m.snapshot.battery = Reading::Measured(BatteryInfo {
            percent,
            on_ac,
            charging: on_ac,
            cycle_count: Reading::pending(),
            health_percent: Reading::pending(),
            condition: Reading::pending(),
            time_remaining_min: Reading::pending(),
            battery_watts: Reading::pending(),
            adapter_watts: Reading::pending(),
            temperature_c: Reading::pending(),
        });
        m
    }

    #[test]
    fn nothing_running_shows_nothing() {
        let mut s = Settings::default();
        s.island.idle_content = IdleContent::Nothing;
        assert_eq!(select(&IslandState::default(), &s, 0.0), None);
    }

    #[test]
    fn priority_order() {
        let s = Settings::default();
        let mut st = IslandState {
            music: Some(playing()),
            agents: Some(agents(SessionPhase::Working)),
            power: Some(PowerState {
                active: Some(KeepAwakeSession {
                    mode: KeepAwakeMode::System,
                    started_at: 0.0,
                    ends_at: Some(600.0),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(matches!(select(&st, &s, 0.0), Some(Activity::Music { .. })));
        st.agents = Some(agents(SessionPhase::WaitingForUser));
        assert!(matches!(
            select(&st, &s, 0.0),
            Some(Activity::AgentWaiting { .. })
        ));
        st.agents = Some(agents(SessionPhase::Working));
        st.music = None;
        assert!(matches!(
            select(&st, &s, 0.0),
            Some(Activity::AgentWorking { sessions: 1, .. })
        ));
        st.agents = None;
        assert_eq!(
            select(&st, &s, 100.0),
            Some(Activity::KeepAwake {
                mode: KeepAwakeMode::System,
                remaining: Some(500.0)
            })
        );
    }

    #[test]
    fn running_timer_extrapolates() {
        let s = Settings::default();
        let st = IslandState {
            timers: Some(TimersState {
                timers: vec![CountdownTimer {
                    id: 1,
                    label: "Thé".into(),
                    duration_secs: 300.0,
                    remaining_secs: 120.0,
                    observed_at: 1000.0,
                    run: TimerRun::Running,
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        match select(&st, &s, 1030.0) {
            Some(a @ Activity::Timer { .. }) => {
                assert_eq!(a.refresh_interval(), Some(1.0));
                if let Activity::Timer { remaining, .. } = a {
                    assert_eq!(remaining, 90.0);
                }
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn battery_only_when_low_on_battery_power() {
        let mut s = Settings::default();
        s.island.idle_content = IdleContent::Nothing;
        let mut st = IslandState {
            metrics: Some(battery(12.0, false)),
            ..Default::default()
        };
        assert!(matches!(
            select(&st, &s, 0.0),
            Some(Activity::Battery { low: true, .. })
        ));
        st.metrics = Some(battery(12.0, true));
        assert_eq!(select(&st, &s, 0.0), None);
        s.island.idle_content = IdleContent::Battery;
        assert!(matches!(
            select(&st, &s, 0.0),
            Some(Activity::Battery { low: false, .. })
        ));
    }

    #[test]
    fn paused_music_is_idle_content_only() {
        let mut s = Settings::default();
        let mut m = playing();
        m.now_playing.as_mut().unwrap().playing = false;
        let st = IslandState {
            music: Some(m),
            ..Default::default()
        };
        assert!(matches!(
            select(&st, &s, 0.0),
            Some(Activity::Music { playing: false, .. })
        ));
        s.island.idle_content = IdleContent::Nothing;
        assert_eq!(select(&st, &s, 0.0), None);
    }
}
