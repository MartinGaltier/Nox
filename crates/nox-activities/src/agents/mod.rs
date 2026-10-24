//! AI agents module (D4–D9): live sessions, tokens, API-equivalent value,
//! plan limits and alerts for Claude Code, Codex, OpenCode and GitHub Copilot,
//! read from their local files.

pub mod claude;
pub mod codex;
pub mod collector;
pub mod copilot;
mod focus;
pub mod limits;
pub mod live;
mod oauth;
pub mod opencode;
pub mod parse;
pub mod pricing;
pub mod registry;
pub mod tail;
pub mod usage;

use collector::{AgentEvent, Collector};
use live::Event;
use nox_core::model::agents::{AgentCommand, AgentSettings};
use nox_core::{
    Alert, AlertKind, AlertPriority, Command, Module, ModuleContext, ModuleError, ModuleId,
    Settings, Worker,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Poll period while an agent works or waits, and otherwise.
const BUSY_POLL: Duration = Duration::from_secs(2);
const IDLE_POLL: Duration = Duration::from_secs(5);

pub fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min {:02} s", s / 60, s % 60),
        _ => format!("{} h {:02}", s / 3600, (s % 3600) / 60),
    }
}

pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1} k", n as f64 / 1e3).replace(".0 k", " k"),
        _ => format!("{:.1} M", n as f64 / 1e6).replace(".0 M", " M"),
    }
}

/// The alert for an event, if the settings want one.
pub fn alert_for(e: &AgentEvent, s: &AgentSettings) -> Option<Alert> {
    match e {
        AgentEvent::Live(Event::Finished {
            provider,
            project,
            duration,
            tokens,
            ..
        }) => {
            if !s.finish_alert || *duration < s.finish_alert_min_secs as f64 {
                return None;
            }
            let mut parts = Vec::new();
            if !project.is_empty() {
                parts.push(project.clone());
            }
            parts.push(fmt_duration(*duration));
            if *tokens > 0 {
                parts.push(format!("{} jetons", fmt_tokens(*tokens)));
            }
            Some(
                Alert::new(
                    AlertKind::AgentFinished,
                    format!("{} a terminé", provider.label()),
                    "checkmark.circle",
                )
                .subtitle(parts.join(" · "))
                .ttl(6.0),
            )
        }
        AgentEvent::Live(Event::NeedsInput {
            provider,
            project,
            what,
        }) => s.needs_input_alert.then(|| {
            let sub = if project.is_empty() {
                what.clone()
            } else {
                format!("{project} · {what}")
            };
            Alert::new(
                AlertKind::AgentNeedsInput,
                format!("{} attend votre réponse", provider.label()),
                "hand.raised",
            )
            .subtitle(sub)
            .priority(AlertPriority::High)
            .ttl(8.0)
        }),
        AgentEvent::Limit {
            provider,
            account,
            window,
            used,
        } => s.limit_alert.then(|| {
            let sub = match account {
                Some(a) => format!("{window} · {a}"),
                None => window.clone(),
            };
            Alert::new(
                AlertKind::AgentLimit,
                format!("Limite {} à {used:.0} %", provider.label()),
                "gauge.with.dots.needle.67percent",
            )
            .subtitle(sub)
            .priority(AlertPriority::High)
            .ttl(8.0)
        }),
        AgentEvent::Budget { spent, budget } => Some(
            Alert::new(
                AlertKind::AgentLimit,
                "Budget IA du jour atteint",
                "dollarsign.circle",
            )
            .subtitle(format!(
                "≈ {spent:.2} $ (valeur API estimée) / {budget:.2} $"
            ))
            .ttl(8.0),
        ),
    }
}

struct Shared {
    collector: Mutex<Collector>,
    settings: Mutex<Arc<Settings>>,
}

pub struct AgentsModule {
    shared: Option<Arc<Shared>>,
    worker: Option<Worker>,
    home: Option<PathBuf>,
    hub: Option<nox_core::Hub>,
}

impl AgentsModule {
    pub fn new() -> Self {
        Self {
            shared: None,
            worker: None,
            home: None,
            hub: None,
        }
    }

    #[cfg(test)]
    fn with_home(home: PathBuf) -> Self {
        Self {
            home: Some(home),
            ..Self::new()
        }
    }
}

impl AgentsModule {
    fn nudge(&self) {
        if let Some(w) = &self.worker {
            w.token().nudge();
        }
    }
}

impl Default for AgentsModule {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Module for AgentsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Agents
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let home = self.home.clone().unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
        });
        let shared = Arc::new(Shared {
            collector: Mutex::new(Collector::new(home)),
            settings: Mutex::new(ctx.settings.clone()),
        });
        let hub = ctx.hub.clone();
        let sh = shared.clone();
        self.worker = Some(Worker::spawn("agents", move |token| {
            let mut last: Option<nox_core::model::agents::AgentsState> = None;
            loop {
                let settings = lock(&sh.settings).clone();
                let now = nox_core::model::now();
                let (state, events, busy) = {
                    let mut c = lock(&sh.collector);
                    let mut events = c.tick(&settings.agents, now);
                    if token.is_cancelled() {
                        break;
                    }
                    let (state, more) = c.snapshot(&settings.agents, now);
                    events.extend(more);
                    (state, events, c.busy(now))
                };
                for e in &events {
                    if let Some(a) = alert_for(e, &settings.agents) {
                        hub.push_alert(a);
                    }
                }
                if last.as_ref() != Some(&state) {
                    hub.update(|s| s.agents = Some(state.clone()));
                    last = Some(state);
                }
                if token.sleep(if busy { BUSY_POLL } else { IDLE_POLL }) {
                    break;
                }
            }
        }));
        self.shared = Some(shared);
        self.hub = Some(ctx.hub);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        self.shared = None;
        if let Some(hub) = self.hub.take() {
            hub.update(|s| s.agents = None);
        }
    }

    fn command(&mut self, cmd: Command) {
        let Command::Agents(cmd) = cmd else { return };
        let Some(sh) = &self.shared else { return };
        match cmd {
            AgentCommand::Refresh => self.nudge(),
            AgentCommand::FocusSession(id) => {
                if let Some(pid) = lock(&sh.collector).session_pid(&id) {
                    focus::focus(pid);
                } else {
                    log::info!("agents: no live process known for that session");
                }
            }
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(sh) = &self.shared {
            *lock(&sh.settings) = settings.clone();
            self.nudge();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_core::Hub;

    #[test]
    fn module_publishes_and_clears_on_stop() {
        let now = nox_core::model::now();
        let home = collector::tests::fake_home("module", now);
        let hub = Hub::new();
        let mut m = AgentsModule::with_home(home.clone());
        m.start(ModuleContext {
            hub: hub.clone(),
            settings: Arc::new(Settings::default()),
        })
        .unwrap();
        let mut ok = false;
        for _ in 0..200 {
            if hub.read(|s| s.agents.as_ref().is_some_and(|a| !a.providers.is_empty())) {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ok, "state published");
        let t0 = std::time::Instant::now();
        m.stop();
        assert!(t0.elapsed() < Duration::from_secs(3), "stop joins quickly");
        assert!(hub.read(|s| s.agents.is_none()));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn alert_rules() {
        let s = AgentSettings::default();
        let fin = |d: f64| {
            AgentEvent::Live(Event::Finished {
                provider: nox_core::model::agents::AgentProvider::Claude,
                project: "demo".into(),
                duration: d,
                tokens: 45_000,
                cost: None,
            })
        };
        assert!(alert_for(&fin(10.0), &s).is_none(), "below the minimum");
        let a = alert_for(&fin(192.0), &s).unwrap();
        assert_eq!(a.title, "Claude a terminé");
        assert_eq!(
            a.subtitle.as_deref(),
            Some("demo · 3 min 12 s · 45 k jetons")
        );
        let off = AgentSettings {
            finish_alert: false,
            ..Default::default()
        };
        assert!(alert_for(&fin(192.0), &off).is_none());
        assert_eq!(fmt_tokens(1_250_000), "1.2 M");
        assert_eq!(fmt_duration(3720.0), "1 h 02");
    }
}
