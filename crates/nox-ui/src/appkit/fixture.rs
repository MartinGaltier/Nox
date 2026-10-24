//! DEBUG-ONLY preview data (`NOX_UI_FIXTURE=1`), never compiled in release
//! (`#[cfg(debug_assertions)]` on the module). It only fills slots that no
//! real module publishes, and every visible string is tagged "[aperçu]".

#![cfg(debug_assertions)]

use nox_core::IslandState;
use nox_core::Reading;
use nox_core::model::agents::*;
use nox_core::model::calendar::*;
use nox_core::model::downloads::*;
use nox_core::model::metrics::*;
use nox_core::model::music::*;
use nox_core::model::power::*;
use nox_core::model::timers::*;
use nox_core::permissions::PermissionState;

const TAG: &str = "[aperçu]";

pub fn overlay(s: &mut IslandState, now: f64) {
    if s.music.is_none() {
        s.music = Some(MusicState {
            unavailable: None,
            players: Default::default(),
            automation: Default::default(),
            now_playing: Some(NowPlaying {
                player: Some(PlayerInfo {
                    bundle_id: "preview".into(),
                    name: format!("Lecteur {TAG}"),
                }),
                title: format!("{TAG} Morceau d’exemple"),
                artist: "Artiste fictif".into(),
                album: "Album fictif".into(),
                duration: Some(214.0),
                elapsed: 63.0,
                observed_at: now,
                playing: true,
                rate: 1.0,
                shuffle: Some(false),
                repeat: Some(RepeatMode::Off),
                can_seek: true,
                can_skip: true,
                ..Default::default()
            }),
            queue: Reading::Estimated(vec![QueueItem {
                title: format!("{TAG} Titre suivant"),
                artist: "Artiste fictif".into(),
                duration: Some(180.0),
            }]),
            lyrics: Reading::Measured(Lyrics {
                synced: true,
                lines: (0..12)
                    .map(|i| LyricLine {
                        time: Some(i as f64 * 12.0),
                        text: format!("{TAG} ligne de paroles {}", i + 1),
                    })
                    .collect(),
                source: "aperçu".into(),
            }),
            // `levels` and fields added later (e.g. `players`) take defaults.
            ..Default::default()
        });
    }
    if s.calendar.is_none() {
        s.calendar = Some(CalendarState {
            reminders_access: Default::default(),
            calendars: vec![],
            access: PermissionState::Granted,
            events: vec![CalendarEvent {
                call: Some(nox_core::model::calendar::VideoCall {
                    service: nox_core::model::calendar::CallService::Meet,
                    url: "https://meet.example/x".into(),
                }),
                id: "p1".into(),
                title: format!("{TAG} Réunion d’exemple"),
                start: now + 25.0 * 60.0,
                end: now + 55.0 * 60.0,
                all_day: false,
                calendar: "Aperçu".into(),
                color: [0.35, 0.6, 1.0],
                location: Some("Salle fictive".into()),
                url: Some("https://example.invalid".into()),
                is_reminder: false,
            }],
        });
    }
    if s.agents.is_none() {
        s.agents = Some(AgentsState {
            period: Default::default(),
            budget: None,
            prices_updated: String::new(),
            card_order: vec![],
            providers: vec![ProviderStatus {
                tokens_period: nox_core::Reading::pending(),
                cost_period_usd: nox_core::Reading::pending(),
                unpriced_requests: 0,
                plan: None,
                banked_resets: nox_core::Reading::pending(),
                accounts: vec![],
                provider: AgentProvider::Claude,
                detected: true,
                sessions: vec![AgentSession {
                    turn_started_at: None,
                    tokens: Default::default(),
                    cost_usd: nox_core::Reading::pending(),
                    account: None,
                    id: "p".into(),
                    provider: AgentProvider::Claude,
                    project: Some(format!("{TAG} projet")),
                    model: None,
                    phase: SessionPhase::Working,
                    activity: Some("Edit".into()),
                    started_at: now - 300.0,
                    last_event_at: now,
                }],
                limits: Reading::Estimated(vec![LimitWindow {
                    kind: Default::default(),
                    window_minutes: None,
                    scope: None,
                    source: Default::default(),
                    observed_at: 0.0,
                    resets_estimated: false,
                    name: "5 h".into(),
                    used_percent: Reading::Estimated(37.0),
                    resets_at: Some(now + 7200.0),
                }]),
                tokens_today: Reading::Estimated(TokenUsage {
                    input: 12_000,
                    output: 48_000,
                    cache_read: 900_000,
                    cache_write: 20_000,
                }),
                cost_today_usd: Reading::Estimated(1.23),
                note: Some(format!("{TAG} données fictives")),
            }],
        });
    }
    if s.timers.is_none() {
        s.timers = Some(TimersState::default());
    }
    if s.downloads.is_none() {
        s.downloads = Some(DownloadsState {
            access: nox_core::permissions::PermissionState::Granted,
            items: vec![DownloadItem {
                id: "d".into(),
                file_name: format!("{TAG} fichier.zip"),
                path: "/dev/null".into(),
                progress: Some(0.42),
                bytes: 42_000_000,
                total: Some(100_000_000),
                rate: Some(3_200_000.0),
                status: DownloadStatus::InProgress,
                source: "Aperçu".into(),
                updated_at: now,
            }],
        });
    }
    if s.metrics.is_none() {
        let mut m = MetricsState {
            history: MetricsHistory::new(60),
            ..Default::default()
        };
        for i in 0..60 {
            let v = if i % 17 == 5 {
                None
            } else {
                Some(20.0 + 15.0 * ((i as f64) * 0.3).sin().abs())
            };
            m.history.push(Series::Cpu, v);
            m.history.push(Series::Gpu, v.map(|v| v / 3.0));
            m.history.push(Series::Memory, Some(61.0));
            m.history.push(Series::NetRx, v.map(|v| v * 10_000.0));
        }
        m.snapshot.cpu_total = Reading::Measured(23.0);
        m.snapshot.gpu_usage = Reading::Measured(7.0);
        m.snapshot.cpu_temp = Reading::Measured(48.0);
        m.snapshot.power = Reading::Unavailable(nox_core::Unavailable::NotSupported);
        m.snapshot.fans_rpm = Reading::Unavailable(nox_core::Unavailable::NotSupported);
        m.snapshot.cpu_cores = Reading::Measured(
            (0..8)
                .map(|i| CoreUsage {
                    index: i,
                    kind: if i < 4 {
                        CoreKind::Efficiency
                    } else {
                        CoreKind::Performance
                    },
                    usage: (i as f64 * 11.0) % 90.0,
                })
                .collect(),
        );
        s.metrics = Some(m);
    }
    if s.power.is_none() {
        s.power = Some(PowerState::default());
    }
}
