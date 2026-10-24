//! Live sessions: one per (provider, session id), driven by parser entries,
//! and their phase at a given moment. For Claude Code the registry status
//! (measured) wins over log heuristics.

use super::parse::Entry;
use super::pricing::Tokens;
use super::registry::{self, Status};
use nox_core::Reading;
use nox_core::model::Timestamp;
use nox_core::model::agents::{AgentProvider, AgentSession, SessionPhase};
use std::collections::HashMap;

/// A turn silent this long is not shown as working.
pub const IDLE_TURN: f64 = 10.0 * 60.0;
/// A finished turn shows as `Finished` this long.
pub const FINISHED_SHOW: f64 = 5.0 * 60.0;
/// Sessions quiet longer than this disappear from the list (unless their
/// process is known alive).
pub const VISIBLE_FOR: f64 = 30.0 * 60.0;
/// Kept in memory this long after the last event.
const FORGET_AFTER: f64 = 6.0 * 3600.0;
/// A turn that ended longer ago is history found late, not news.
const LATE_END: f64 = 5.0 * 60.0;

#[derive(Debug, Clone)]
pub struct Live {
    pub provider: AgentProvider,
    pub account: usize,
    pub id: String,
    pub project: String,
    pub model: String,
    pub first_seen: Timestamp,
    pub last_event: Timestamp,
    pub turn_open: bool,
    pub turn_started: Option<Timestamp>,
    pub turn_tokens: Tokens,
    /// `None` once a response of the turn had no known price.
    pub turn_cost: Option<f64>,
    pub activity: Option<String>,
    pub question: Option<String>,
    pub ended_at: Option<Timestamp>,
    pub ended_completed: bool,
    /// Last phase reported, to detect transitions.
    pub last_phase: Option<SessionPhase>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Finished {
        provider: AgentProvider,
        project: String,
        duration: f64,
        tokens: u64,
        cost: Option<f64>,
    },
    NeedsInput {
        provider: AgentProvider,
        project: String,
        what: String,
    },
}

#[derive(Default)]
pub struct Tracker {
    pub sessions: HashMap<(AgentProvider, String), Live>,
}

pub struct Ctx<'a> {
    pub provider: AgentProvider,
    pub account: usize,
    pub session: &'a str,
    pub project: &'a str,
    pub model: &'a str,
    /// False while history is first read, so it never replays as news.
    pub reports: bool,
    pub now: Timestamp,
}

impl Tracker {
    fn entry(&mut self, c: &Ctx, at: Timestamp) -> &mut Live {
        let key = (c.provider, c.session.to_string());
        let l = self.sessions.entry(key).or_insert_with(|| Live {
            provider: c.provider,
            account: c.account,
            id: c.session.to_string(),
            project: String::new(),
            model: String::new(),
            first_seen: at,
            last_event: at,
            turn_open: false,
            turn_started: None,
            turn_tokens: Tokens::default(),
            turn_cost: Some(0.0),
            activity: None,
            question: None,
            ended_at: None,
            ended_completed: false,
            last_phase: None,
        });
        if !c.project.is_empty() {
            l.project = c.project.to_string();
        }
        if !c.model.is_empty() {
            l.model = c.model.to_string();
        }
        l.last_event = l.last_event.max(at);
        l
    }

    /// Applies one entry; returns an event worth an alert, if any.
    pub fn apply(&mut self, e: &Entry, c: &Ctx) -> Option<Event> {
        if c.session.is_empty() {
            return None;
        }
        match e {
            Entry::TurnBegan(at) => {
                let l = self.entry(c, *at);
                l.turn_open = true;
                l.turn_started = Some(*at);
                l.turn_tokens = Tokens::default();
                l.turn_cost = Some(0.0);
                l.activity = None;
                l.question = None;
                l.ended_at = None;
                None
            }
            Entry::TurnActive(at) => {
                let key = (c.provider, c.session.to_string());
                if let Some(l) = self.sessions.get_mut(&key) {
                    l.last_event = l.last_event.max(*at);
                }
                None
            }
            Entry::Tool { at, name } => {
                let l = self.entry(c, *at);
                l.activity = Some(name.clone());
                l.question = None;
                None
            }
            Entry::ToolsDone(at) => {
                let l = self.entry(c, *at);
                l.question = None;
                None
            }
            Entry::Question { at, what } => {
                let reports = c.reports && c.now - at < LATE_END;
                let l = self.entry(c, *at);
                let label = match what.as_str() {
                    "ExitPlanMode" => "Plan à valider",
                    _ => "Question posée",
                };
                l.question = Some(label.into());
                l.activity = Some(label.into());
                reports.then(|| Event::NeedsInput {
                    provider: c.provider,
                    project: l.project.clone(),
                    what: label.into(),
                })
            }
            Entry::TurnEnded {
                at,
                completed,
                duration,
            } => {
                let l = self.entry(c, *at);
                let was_open = l.turn_open;
                l.turn_open = false;
                l.question = None;
                l.ended_at = Some(*at);
                l.ended_completed = *completed;
                let dur = duration.or(l.turn_started.map(|s| (at - s).max(0.0)))?;
                (was_open && *completed && c.reports && c.now - at <= LATE_END).then(|| {
                    Event::Finished {
                        provider: c.provider,
                        project: l.project.clone(),
                        duration: dur,
                        tokens: l.turn_tokens.total(),
                        cost: l.turn_cost,
                    }
                })
            }
            _ => None,
        }
    }

    /// Counts a response toward the session's open turn.
    pub fn add_usage(&mut self, c: &Ctx, at: Timestamp, tokens: &Tokens, cost: Option<f64>) {
        let key = (c.provider, c.session.to_string());
        if let Some(l) = self.sessions.get_mut(&key)
            && l.turn_open
            && l.turn_started.is_none_or(|s| at >= s - 1.0)
        {
            l.turn_tokens.add(tokens);
            l.turn_cost = match (l.turn_cost, cost) {
                (Some(a), Some(b)) => Some(a + b),
                _ => None,
            };
            l.last_event = l.last_event.max(at);
        }
    }

    pub fn forget_old(&mut self, now: Timestamp, keep: &dyn Fn(&Live) -> bool) {
        self.sessions
            .retain(|_, l| now - l.last_event < FORGET_AFTER || keep(l));
    }
}

/// Phase of a session at `now`; `reg` is the Claude registry entry if any.
pub fn phase(
    l: &Live,
    reg: Option<&registry::Entry>,
    now: Timestamp,
) -> (SessionPhase, Option<String>) {
    let recently_ended = l.ended_at.is_some_and(|e| now - e < FINISHED_SHOW);
    if let Some(r) = reg {
        return match r.status {
            Status::Waiting => (
                SessionPhase::WaitingForUser,
                Some(registry::waiting_label(r.waiting_for.as_deref())),
            ),
            Status::Busy => (SessionPhase::Working, l.activity.clone()),
            Status::Idle if recently_ended && !l.turn_open => (SessionPhase::Finished, None),
            Status::Idle => (SessionPhase::Idle, None),
            Status::Unknown => log_phase(l, now, recently_ended),
        };
    }
    log_phase(l, now, recently_ended)
}

fn log_phase(l: &Live, now: Timestamp, recently_ended: bool) -> (SessionPhase, Option<String>) {
    if l.turn_open {
        if let Some(q) = &l.question {
            return (SessionPhase::WaitingForUser, Some(q.clone()));
        }
        if now - l.last_event < IDLE_TURN {
            return (SessionPhase::Working, l.activity.clone());
        }
        return (SessionPhase::Idle, None);
    }
    if recently_ended {
        (SessionPhase::Finished, None)
    } else {
        (SessionPhase::Idle, None)
    }
}

pub fn to_model(
    l: &Live,
    phase: SessionPhase,
    activity: Option<String>,
    account: Option<String>,
) -> AgentSession {
    AgentSession {
        id: l.id.clone(),
        provider: l.provider,
        project: (!l.project.is_empty()).then(|| l.project.clone()),
        model: (!l.model.is_empty()).then(|| super::pricing::display_name(&l.model)),
        phase,
        activity,
        started_at: l.first_seen,
        last_event_at: l.last_event,
        turn_started_at: l.turn_started,
        tokens: l.turn_tokens.to_usage(),
        cost_usd: match l.turn_cost {
            Some(c) => Reading::Estimated(c),
            None => Reading::Unavailable(nox_core::Unavailable::Error(
                "modèle sans prix connu".into(),
            )),
        },
        account,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(now: f64, reports: bool) -> Ctx<'static> {
        Ctx {
            provider: AgentProvider::Claude,
            account: 0,
            session: "s1",
            project: "demo",
            model: "claude-opus-5-5",
            reports,
            now,
        }
    }

    #[test]
    fn state_machine_and_events() {
        let mut t = Tracker::default();
        let c = ctx(1000.0, true);
        assert_eq!(t.apply(&Entry::TurnBegan(1000.0), &c), None);
        t.apply(
            &Entry::Tool {
                at: 1001.0,
                name: "Bash".into(),
            },
            &c,
        );
        t.add_usage(
            &c,
            1002.0,
            &Tokens {
                input: 10,
                output: 5,
                ..Default::default()
            },
            Some(0.5),
        );
        let l = &t.sessions[&(AgentProvider::Claude, "s1".to_string())];
        assert_eq!(
            phase(l, None, 1005.0),
            (SessionPhase::Working, Some("Bash".into()))
        );
        assert_eq!(
            phase(l, None, 1001.0 + IDLE_TURN + 1.0).0,
            SessionPhase::Idle
        );

        let ev = t.apply(
            &Entry::Question {
                at: 1010.0,
                what: "AskUserQuestion".into(),
            },
            &ctx(1010.0, true),
        );
        assert!(matches!(ev, Some(Event::NeedsInput { .. })));
        let l = &t.sessions[&(AgentProvider::Claude, "s1".to_string())];
        assert_eq!(phase(l, None, 1011.0).0, SessionPhase::WaitingForUser);
        t.apply(&Entry::ToolsDone(1020.0), &c);

        let ev = t.apply(
            &Entry::TurnEnded {
                at: 1100.0,
                completed: true,
                duration: None,
            },
            &ctx(1100.0, true),
        );
        assert_eq!(
            ev,
            Some(Event::Finished {
                provider: AgentProvider::Claude,
                project: "demo".into(),
                duration: 100.0,
                tokens: 15,
                cost: Some(0.5)
            })
        );
        let l = &t.sessions[&(AgentProvider::Claude, "s1".to_string())];
        assert_eq!(phase(l, None, 1101.0).0, SessionPhase::Finished);
        assert_eq!(
            phase(l, None, 1100.0 + FINISHED_SHOW + 1.0).0,
            SessionPhase::Idle
        );
    }

    #[test]
    fn history_does_not_alert() {
        let mut t = Tracker::default();
        let c = ctx(10_000.0, false);
        t.apply(&Entry::TurnBegan(100.0), &c);
        let ev = t.apply(
            &Entry::TurnEnded {
                at: 200.0,
                completed: true,
                duration: Some(100.0),
            },
            &c,
        );
        assert_eq!(ev, None);
        // Live reading of a turn that ended long ago: late, no alert either.
        t.apply(&Entry::TurnBegan(300.0), &c);
        let ev = t.apply(
            &Entry::TurnEnded {
                at: 400.0,
                completed: true,
                duration: None,
            },
            &ctx(10_000.0, true),
        );
        assert_eq!(ev, None);
    }

    #[test]
    fn registry_wins() {
        let mut t = Tracker::default();
        let c = ctx(0.0, false);
        t.apply(&Entry::TurnBegan(0.0), &c);
        let l = &t.sessions[&(AgentProvider::Claude, "s1".to_string())];
        let reg = registry::Entry {
            pid: 1,
            status: Status::Waiting,
            waiting_for: Some("permission".into()),
            cwd: None,
            status_updated_ms: None,
        };
        assert_eq!(
            phase(l, Some(&reg), 1.0),
            (
                SessionPhase::WaitingForUser,
                Some("Autorisation demandée".into())
            )
        );
        let idle = registry::Entry {
            status: Status::Idle,
            ..reg
        };
        assert_eq!(phase(l, Some(&idle), 1.0).0, SessionPhase::Idle);
    }
}
