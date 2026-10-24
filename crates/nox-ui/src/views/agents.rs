//! AI agents page: one card per provider (sessions, plan limits with reset,
//! tokens today, estimated API-equivalent cost).

use super::{Ctx, ListLayout, bar, empty, label, module_missing, secondary, tertiary};
use crate::format;
use crate::geom::{Color, Rect};
use crate::scene::{Action, Align, Scene, Weight};
use nox_core::model::agents::{AgentCommand, ProviderStatus, SessionPhase};
use nox_core::{Command, Reading, Section};

pub fn tokens(n: u64) -> String {
    let v = n as f64;
    if v >= 1e9 {
        format!("{}{}G", format::decimal(v / 1e9, 1), format::NNBSP)
    } else if v >= 1e6 {
        format!("{}{}M", format::decimal(v / 1e6, 1), format::NNBSP)
    } else if v >= 1e3 {
        format!("{}{}k", format::decimal(v / 1e3, 1), format::NNBSP)
    } else {
        n.to_string()
    }
}

pub fn phase_label(p: SessionPhase) -> (&'static str, Color) {
    match p {
        SessionPhase::Working => ("travaille", Color::GREEN),
        SessionPhase::WaitingForUser => ("attend votre réponse", Color::ORANGE),
        SessionPhase::Idle => ("inactif", Color::TERTIARY),
        SessionPhase::Finished => ("terminé", Color::SECONDARY),
    }
}

fn card_height(p: &ProviderStatus) -> f64 {
    let sessions = p.sessions.len().min(3) as f64;
    let limits = match &p.limits {
        Reading::Measured(l) | Reading::Estimated(l) => l.len().min(3) as f64,
        Reading::Unavailable(_) => 1.0,
    };
    30.0 + sessions * 16.0 + limits * 18.0 + 22.0
}

pub fn page(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(a) = cx.state.agents.as_ref() else {
        return module_missing(s, r, Section::Agents, enabled);
    };
    let shown: Vec<_> = a
        .providers
        .iter()
        .filter(|p| p.detected && cx.settings.agents.enabled.contains(&p.provider))
        .collect();
    if shown.is_empty() {
        return empty(
            s,
            r,
            "sparkles",
            "Aucun agent détecté",
            "Claude Code, Codex, OpenCode et Copilot sont lus depuis leurs journaux locaux.",
        );
    }
    let mut list = ListLayout::begin(s, r, cx.scroll);
    for p in shown {
        let h = card_height(p);
        let Some(c) = list.row(h, 8.0) else {
            continue;
        };
        s.fill(c, 12.0, Color::CARD);
        let inner = c.inset(12.0, 9.0);
        s.text(
            Rect::new(inner.x, inner.y, inner.w * 0.5, 16.0),
            p.provider.label(),
            label(13.0).weight(Weight::Semibold),
        );
        // Tokens + cost on the right of the title.
        let tok = format::reading(&p.tokens_today, |t| {
            format!("{} jetons aujourd’hui", tokens(t.total()))
        });
        let cost = match &p.cost_today_usd {
            Reading::Measured(v) | Reading::Estimated(v) => {
                format!("≈ {} $ (estimé)", format::decimal(*v, 2))
            }
            Reading::Unavailable(u) => format!("coût : {}", format::unavailable(u)),
        };
        s.text(
            Rect::new(inner.x + inner.w * 0.4, inner.y, inner.w * 0.6, 14.0),
            tok,
            secondary(10.5).align(Align::Right),
        );
        s.text(
            Rect::new(inner.x + inner.w * 0.4, inner.y + 14.0, inner.w * 0.6, 13.0),
            cost,
            tertiary(10.0).align(Align::Right),
        );
        let mut y = inner.y + 21.0;
        if p.sessions.is_empty() {
            s.text(
                Rect::new(inner.x, y, inner.w * 0.6, 14.0),
                "Aucune session en cours",
                tertiary(10.5),
            );
            y += 16.0;
        }
        for sess in p.sessions.iter().take(3) {
            let (txt, col) = phase_label(sess.phase);
            s.fill(Rect::new(inner.x, y + 4.0, 6.0, 6.0), 3.0, col);
            let mut line = sess.project.clone().unwrap_or_else(|| "Session".into());
            line.push_str(" — ");
            line.push_str(txt);
            if let Some(act) = &sess.activity {
                line.push_str(" · ");
                line.push_str(act);
            }
            if let Some(m) = &sess.model {
                line.push_str(" · ");
                line.push_str(m);
            }
            let rr = Rect::new(inner.x + 12.0, y, inner.w - 12.0, 14.0);
            s.text(rr, line, secondary(10.5));
            s.hit(
                rr,
                Action::Send(Command::Agents(AgentCommand::FocusSession(sess.id.clone()))),
                "Afficher la session",
            );
            y += 16.0;
        }
        if p.sessions.len() > 3 {
            s.text(
                Rect::new(inner.x + 12.0, y - 2.0, inner.w, 12.0),
                format!("+ {} sessions", p.sessions.len() - 3),
                tertiary(9.5),
            );
        }
        y += 4.0;
        match &p.limits {
            Reading::Measured(ls) | Reading::Estimated(ls) => {
                let est = p.limits.is_estimated();
                for l in ls.iter().take(3) {
                    let lr = Rect::new(inner.x, y, inner.w, 16.0);
                    let (name_r, rest) = lr.split_left(86.0, 6.0);
                    let (bar_r, txt_r) = rest.split_right((rest.w - 90.0).max(150.0), 8.0);
                    s.text(name_r, &l.name, secondary(10.5));
                    let used = l.used_percent.value().copied();
                    let color = match used {
                        Some(u) if u >= 90.0 => Color::RED,
                        Some(u) if u >= 70.0 => Color::ORANGE,
                        _ => Color::ACCENT,
                    };
                    bar(
                        s,
                        Rect::new(bar_r.x, bar_r.mid_y() - 2.5, bar_r.w, 5.0),
                        used.map(|u| u / 100.0),
                        color,
                    );
                    let mut t = format::reading(&l.used_percent, |u| {
                        format!("{} utilisés", format::percent(*u))
                    });
                    if est && !t.contains("estimé") {
                        t.push_str(" (estimé)");
                    }
                    if let Some(reset) = l.resets_at {
                        t.push_str(&format!(" · reset {}", format::relative(reset, cx.now)));
                        s.refresh_in(60.0);
                    }
                    s.text(txt_r, t, tertiary(10.0).align(Align::Right));
                    y += 18.0;
                }
            }
            Reading::Unavailable(u) => {
                let why = match u {
                    nox_core::Unavailable::Disabled => {
                        "Limites : désactivées (activez la lecture des limites dans les réglages)"
                            .to_string()
                    }
                    other => format!("Limites : {}", format::unavailable(other)),
                };
                s.text(Rect::new(inner.x, y, inner.w, 14.0), why, tertiary(10.0));
            }
        }
        if let Some(n) = &p.note {
            s.text(
                Rect::new(inner.x, c.max_y() - 20.0, inner.w, 13.0),
                n,
                tertiary(9.5),
            );
        }
    }
    list.end(s);
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use nox_core::model::agents::{
        AgentProvider, AgentSession, AgentsState, LimitWindow, TokenUsage,
    };
    use nox_core::{IslandState, Settings, Unavailable};

    #[test]
    fn card_marks_estimates_and_unavailable() {
        let settings = Settings::default();
        let st = IslandState {
            agents: Some(AgentsState {
                period: Default::default(),
                budget: None,
                prices_updated: String::new(),
                card_order: vec![],
                providers: vec![
                    ProviderStatus {
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
                            id: "a".into(),
                            provider: AgentProvider::Claude,
                            project: Some("nox".into()),
                            model: Some("opus".into()),
                            phase: SessionPhase::Working,
                            activity: Some("Bash".into()),
                            started_at: 0.0,
                            last_event_at: 0.0,
                        }],
                        limits: Reading::Measured(vec![LimitWindow {
                            kind: Default::default(),
                            window_minutes: None,
                            scope: None,
                            source: Default::default(),
                            observed_at: 0.0,
                            resets_estimated: false,
                            name: "5 h".into(),
                            used_percent: Reading::Measured(42.0),
                            resets_at: Some(1_800_000_000.0 + 3600.0),
                        }]),
                        tokens_today: Reading::Measured(TokenUsage {
                            input: 1000,
                            output: 2500,
                            cache_read: 1_000_000,
                            cache_write: 0,
                        }),
                        cost_today_usd: Reading::Estimated(3.456),
                        note: None,
                    },
                    ProviderStatus {
                        tokens_period: nox_core::Reading::pending(),
                        cost_period_usd: nox_core::Reading::pending(),
                        unpriced_requests: 0,
                        plan: None,
                        banked_resets: nox_core::Reading::pending(),
                        accounts: vec![],
                        provider: AgentProvider::Codex,
                        detected: true,
                        sessions: vec![],
                        limits: Reading::Unavailable(Unavailable::Disabled),
                        tokens_today: Reading::Unavailable(Unavailable::NotSupported),
                        cost_today_usd: Reading::Unavailable(Unavailable::NotSupported),
                        note: None,
                    },
                    ProviderStatus {
                        tokens_period: nox_core::Reading::pending(),
                        cost_period_usd: nox_core::Reading::pending(),
                        unpriced_requests: 0,
                        plan: None,
                        banked_resets: nox_core::Reading::pending(),
                        accounts: vec![],
                        provider: AgentProvider::Copilot,
                        detected: false,
                        sessions: vec![],
                        limits: Reading::pending(),
                        tokens_today: Reading::pending(),
                        cost_today_usd: Reading::pending(),
                        note: None,
                    },
                ],
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        page(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("≈ 3,46 $ (estimé)"), "{:?}", s.texts());
        assert!(s.has_text("1,0\u{202F}M jetons"));
        assert!(s.has_text("nox — travaille · Bash · opus"));
        assert!(s.has_text("42\u{202F}% utilisés · reset dans 1\u{202F}h\u{202F}00"));
        assert!(s.has_text("Limites : désactivées"));
        assert!(s.has_text("Indisponible"));
        assert!(!s.has_text("GitHub Copilot"), "undetected providers hidden");
    }
}
