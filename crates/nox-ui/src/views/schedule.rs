//! Calendar and timers/stopwatch/focus pages.

use super::{
    Ctx, ListLayout, bar, card, empty, icon_button, label, module_missing, permission_gate, pill,
    secondary, tertiary,
};
use crate::format;
use crate::geom::{Color, Rect};
use crate::scene::{Action, Align, Scene, Weight};
use nox_core::model::calendar::CalendarCommand;
use nox_core::model::timers::{FocusPhase, TimerCommand, TimerRun};
use nox_core::permissions::Permission;
use nox_core::{Command, Section};

pub fn calendar(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(c) = cx.state.calendar.as_ref() else {
        return module_missing(s, r, Section::Calendar, enabled);
    };
    if !permission_gate(
        s,
        cx,
        r,
        Permission::Calendars,
        c.access,
        "Nox lit vos événements à venir ; rien ne quitte ce Mac.",
        Some(Command::Calendar(CalendarCommand::RequestAccess)),
    ) {
        return;
    }
    let cs = &cx.settings.calendar;
    let horizon = cx.now + cs.days_ahead.max(1) as f64 * 86_400.0;
    let mut events: Vec<_> = c
        .events
        .iter()
        .filter(|e| e.end >= cx.now && e.start <= horizon)
        .filter(|e| !cs.hidden_calendars.contains(&e.calendar))
        .filter(|e| cs.show_reminders || !e.is_reminder)
        .collect();
    events.sort_by(|a, b| a.start.total_cmp(&b.start));
    if events.is_empty() {
        return empty(
            s,
            r,
            "calendar",
            "Aucun événement",
            &format!("Rien de prévu dans les {} prochains jours.", cs.days_ahead),
        );
    }
    let mut list = ListLayout::begin(s, r, cx.scroll);
    let mut last_day: Option<f64> = None;
    for e in events {
        if last_day.is_none_or(|d| !format::same_day(d, e.start.max(cx.now))) {
            last_day = Some(e.start.max(cx.now));
            if let Some(h) = list.row(16.0, 2.0) {
                s.text(
                    h,
                    format::day_label(e.start.max(cx.now), cx.now),
                    tertiary(10.5).weight(Weight::Semibold),
                );
            }
        }
        let Some(row) = list.row(38.0, 4.0) else {
            continue;
        };
        let bg = if cx.hovered(row) {
            Color::CARD_HOVER
        } else {
            Color::CARD
        };
        s.fill(row, 9.0, bg);
        let [cr, cg, cb] = e.color;
        s.fill(
            Rect::new(row.x + 8.0, row.y + 8.0, 3.0, row.h - 16.0),
            1.5,
            Color::rgba(cr as f64, cg as f64, cb as f64, 1.0),
        );
        let when = if e.all_day {
            "Journée".to_string()
        } else {
            format!(
                "{} – {}",
                format::hour_minute(e.start),
                format::hour_minute(e.end)
            )
        };
        let join = e.url.is_some();
        let tw = row.w - 20.0 - if join { 84.0 } else { 0.0 } - 90.0;
        s.text(
            Rect::new(row.x + 18.0, row.y + 4.0, tw, 16.0),
            &e.title,
            label(12.5).weight(Weight::Medium),
        );
        let mut sub = when;
        if let Some(l) = &e.location
            && !l.is_empty()
        {
            sub.push_str(" · ");
            sub.push_str(l);
        }
        s.text(
            Rect::new(row.x + 18.0, row.y + 20.0, tw, 14.0),
            sub,
            secondary(10.5),
        );
        let status = if e.start <= cx.now {
            format!("encore {}", format::duration(e.end - cx.now))
        } else {
            format::relative(e.start, cx.now)
        };
        let status_x = row.max_x() - 90.0 - if join { 84.0 } else { 0.0 };
        s.text(
            Rect::new(status_x, row.y + 12.0, 84.0, 14.0),
            status,
            tertiary(10.5).align(Align::Right),
        );
        s.hit(
            row,
            Action::Send(Command::Calendar(CalendarCommand::OpenEvent(e.id.clone()))),
            "Ouvrir dans Calendrier",
        );
        if join {
            pill(
                s,
                cx,
                Rect::new(row.max_x() - 82.0, row.y + 8.0, 74.0, 22.0),
                "Rejoindre",
                true,
                Action::Send(Command::Calendar(CalendarCommand::JoinCall(e.id.clone()))),
            );
        }
        // Countdown changes every minute.
        s.refresh_in(60.0);
    }
    list.end(s);
}

fn remaining(rem: f64, observed: f64, running: bool, now: f64) -> f64 {
    if running {
        (rem - (now - observed).max(0.0)).max(0.0)
    } else {
        rem.max(0.0)
    }
}

pub fn focus_label(p: FocusPhase) -> &'static str {
    match p {
        FocusPhase::Work => "Concentration",
        FocusPhase::ShortBreak => "Pause courte",
        FocusPhase::LongBreak => "Pause longue",
    }
}

pub fn timers(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(t) = cx.state.timers.as_ref() else {
        return module_missing(s, r, Section::Timers, enabled);
    };
    let send = |c: TimerCommand| Action::Send(Command::Timers(c));
    let col_w = (r.w - 16.0) / 3.0;
    let cols = [
        Rect::new(r.x, r.y, col_w, r.h),
        Rect::new(r.x + col_w + 8.0, r.y, col_w, r.h),
        Rect::new(r.x + 2.0 * (col_w + 8.0), r.y, col_w, r.h),
    ];

    // Countdowns.
    let c0 = cols[0];
    card(s, c0);
    s.text(
        Rect::new(c0.x + 10.0, c0.y + 8.0, c0.w - 20.0, 14.0),
        "Minuteurs",
        tertiary(10.5).weight(Weight::Semibold),
    );
    let active: Vec<_> = t
        .timers
        .iter()
        .filter(|c| c.run != TimerRun::Finished)
        .collect();
    let mut y = c0.y + 26.0;
    for c in active.iter().take(2) {
        let running = c.run == TimerRun::Running;
        let rem = remaining(c.remaining_secs, c.observed_at, running, cx.now);
        if running {
            s.refresh_in((rem.fract()).max(0.05));
        }
        s.text(
            Rect::new(c0.x + 10.0, y, c0.w - 20.0, 22.0),
            format::clock(rem.ceil()),
            label(18.0).weight(Weight::Semibold).mono(),
        );
        if !c.label.is_empty() {
            s.text(
                Rect::new(c0.x + 10.0, y + 22.0, c0.w - 80.0, 13.0),
                &c.label,
                secondary(10.0),
            );
        }
        let frac = if c.duration_secs > 0.0 {
            Some(1.0 - rem / c.duration_secs)
        } else {
            None
        };
        bar(
            s,
            Rect::new(c0.x + 10.0, y + 37.0, c0.w - 20.0, 3.0),
            frac,
            Color::ORANGE,
        );
        let bx = c0.max_x() - 10.0 - 22.0;
        icon_button(
            s,
            cx,
            Rect::new(bx, y + 1.0, 22.0, 22.0),
            "xmark",
            10.0,
            send(TimerCommand::Cancel(c.id)),
            "Annuler",
        );
        icon_button(
            s,
            cx,
            Rect::new(bx - 24.0, y + 1.0, 22.0, 22.0),
            if running { "pause.fill" } else { "play.fill" },
            10.0,
            send(if running {
                TimerCommand::Pause(c.id)
            } else {
                TimerCommand::Resume(c.id)
            }),
            if running { "Pause" } else { "Reprendre" },
        );
        icon_button(
            s,
            cx,
            Rect::new(bx - 48.0, y + 1.0, 22.0, 22.0),
            "plus",
            10.0,
            send(TimerCommand::AddTime {
                id: c.id,
                secs: 60.0,
            }),
            "+1 min",
        );
        y += 48.0;
    }
    if active.len() > 2 {
        s.text(
            Rect::new(c0.x + 10.0, y, c0.w - 20.0, 13.0),
            format!("+ {} autres", active.len() - 2),
            tertiary(10.0),
        );
    }
    // Presets along the bottom.
    let presets: Vec<u64> = cx
        .settings
        .timers
        .presets_secs
        .iter()
        .copied()
        .take(4)
        .collect();
    if !presets.is_empty() {
        let pw = (c0.w - 20.0 - 4.0 * (presets.len() as f64 - 1.0)) / presets.len() as f64;
        for (i, p) in presets.iter().enumerate() {
            let pr = Rect::new(
                c0.x + 10.0 + i as f64 * (pw + 4.0),
                c0.max_y() - 30.0,
                pw,
                22.0,
            );
            let text = if *p >= 3600 {
                format!("{} h", p / 3600)
            } else if *p >= 60 {
                format!("{} min", p / 60)
            } else {
                format!("{p} s")
            };
            pill(
                s,
                cx,
                pr,
                &text,
                false,
                send(TimerCommand::Start {
                    label: String::new(),
                    secs: *p as f64,
                }),
            );
        }
    }

    // Stopwatch.
    let c1 = cols[1];
    card(s, c1);
    s.text(
        Rect::new(c1.x + 10.0, c1.y + 8.0, c1.w - 20.0, 14.0),
        "Chronomètre",
        tertiary(10.5).weight(Weight::Semibold),
    );
    let sw = &t.stopwatch;
    let el = sw.elapsed_secs
        + if sw.running {
            (cx.now - sw.observed_at).max(0.0)
        } else {
            0.0
        };
    if sw.running {
        s.refresh_in((1.0 - el.fract()).max(0.05));
    }
    s.text(
        Rect::new(c1.x + 10.0, c1.y + 28.0, c1.w - 20.0, 26.0),
        format::clock(el),
        label(22.0).weight(Weight::Semibold).mono(),
    );
    for (i, lap) in sw.laps.iter().rev().take(3).enumerate() {
        s.text(
            Rect::new(
                c1.x + 10.0,
                c1.y + 60.0 + i as f64 * 14.0,
                c1.w - 20.0,
                13.0,
            ),
            format!("Tour {} · {}", sw.laps.len() - i, format::clock(*lap)),
            secondary(10.0).mono(),
        );
    }
    let by = c1.max_y() - 30.0;
    let third = (c1.w - 28.0) / 3.0;
    pill(
        s,
        cx,
        Rect::new(c1.x + 10.0, by, third, 22.0),
        if sw.running { "Arrêter" } else { "Démarrer" },
        sw.running,
        send(TimerCommand::StopwatchToggle),
    );
    pill(
        s,
        cx,
        Rect::new(c1.x + 14.0 + third, by, third, 22.0),
        "Tour",
        false,
        send(TimerCommand::StopwatchLap),
    );
    pill(
        s,
        cx,
        Rect::new(c1.x + 18.0 + 2.0 * third, by, third, 22.0),
        "Zéro",
        false,
        send(TimerCommand::StopwatchReset),
    );

    // Focus.
    let c2 = cols[2];
    card(s, c2);
    s.text(
        Rect::new(c2.x + 10.0, c2.y + 8.0, c2.w - 20.0, 14.0),
        "Pomodoro",
        tertiary(10.5).weight(Weight::Semibold),
    );
    match &t.focus {
        Some(f) => {
            let rem = remaining(f.remaining_secs, f.observed_at, !f.paused, cx.now);
            if !f.paused {
                s.refresh_in(rem.fract().max(0.05));
            }
            s.text(
                Rect::new(c2.x + 10.0, c2.y + 26.0, c2.w - 20.0, 14.0),
                format!("{} · {}/{}", focus_label(f.phase), f.round, f.rounds_total),
                secondary(10.5),
            );
            s.text(
                Rect::new(c2.x + 10.0, c2.y + 42.0, c2.w - 20.0, 26.0),
                format::clock(rem.ceil()),
                label(22.0).weight(Weight::Semibold).mono(),
            );
            let frac = (f.phase_secs > 0.0).then(|| 1.0 - rem / f.phase_secs);
            let color = if f.phase == FocusPhase::Work {
                Color::RED
            } else {
                Color::GREEN
            };
            bar(
                s,
                Rect::new(c2.x + 10.0, c2.y + 74.0, c2.w - 20.0, 3.0),
                frac,
                color,
            );
            let bw = (c2.w - 28.0) / 3.0;
            let by = c2.max_y() - 30.0;
            pill(
                s,
                cx,
                Rect::new(c2.x + 10.0, by, bw, 22.0),
                if f.paused { "Reprendre" } else { "Pause" },
                false,
                send(TimerCommand::FocusPauseResume),
            );
            pill(
                s,
                cx,
                Rect::new(c2.x + 14.0 + bw, by, bw, 22.0),
                "Passer",
                false,
                send(TimerCommand::FocusSkip),
            );
            pill(
                s,
                cx,
                Rect::new(c2.x + 18.0 + 2.0 * bw, by, bw, 22.0),
                "Fin",
                false,
                send(TimerCommand::FocusStop),
            );
        }
        None => {
            let ts = &cx.settings.timers;
            s.text(
                Rect::new(c2.x + 10.0, c2.y + 30.0, c2.w - 20.0, 44.0),
                format!(
                    "{} min de travail, pauses de {} et {} min, {} sessions.",
                    ts.focus_work_min,
                    ts.focus_short_break_min,
                    ts.focus_long_break_min,
                    ts.focus_rounds
                ),
                secondary(10.5).lines(3),
            );
            pill(
                s,
                cx,
                Rect::new(c2.x + 10.0, c2.max_y() - 30.0, c2.w - 20.0, 22.0),
                "Commencer",
                true,
                send(TimerCommand::FocusStart),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use nox_core::model::calendar::{CalendarEvent, CalendarState};
    use nox_core::model::timers::{CountdownTimer, TimersState};
    use nox_core::permissions::PermissionState;
    use nox_core::{IslandState, Settings};

    #[test]
    fn calendar_permission_and_events() {
        let settings = Settings::default();
        let mut st = IslandState {
            calendar: Some(CalendarState {
                access: PermissionState::NotDetermined,
                events: vec![],
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        calendar(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.actions().contains(&&Action::Send(Command::Calendar(
            CalendarCommand::RequestAccess
        ))));
        let now = 1_800_000_000.0;
        st.calendar = Some(CalendarState {
            access: PermissionState::Granted,
            events: vec![CalendarEvent {
                id: "e1".into(),
                title: "Point d’équipe".into(),
                start: now + 600.0,
                end: now + 2400.0,
                all_day: false,
                calendar: "Travail".into(),
                color: [0.2, 0.5, 1.0],
                location: None,
                url: Some("https://meet.example/x".into()),
                is_reminder: false,
            }],
        });
        let mut s = Scene::default();
        calendar(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("Point d’équipe"));
        assert!(s.has_text("dans 10"));
        assert!(s.actions().contains(&&Action::Send(Command::Calendar(
            CalendarCommand::JoinCall("e1".into())
        ))));
        let mut denied = st.clone();
        denied.calendar.as_mut().unwrap().access = PermissionState::Denied;
        let mut s = Scene::default();
        calendar(&mut s, &ctx(&denied, &settings), page_rect(), true);
        assert!(s.has_text("Autorisation requise"));
        assert!(s.actions().iter().any(|a| matches!(a, Action::OpenUrl(_))));
    }

    #[test]
    fn timers_show_remaining_and_refresh_only_when_running() {
        let settings = Settings::default();
        let now = 1_800_000_000.0;
        let mut st = IslandState {
            timers: Some(TimersState {
                timers: vec![CountdownTimer {
                    id: 7,
                    label: "Pâtes".into(),
                    duration_secs: 600.0,
                    remaining_secs: 300.0,
                    observed_at: now - 30.0,
                    run: TimerRun::Running,
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        timers(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("4:30"), "{:?}", s.texts());
        assert!(s.refresh.is_some());
        assert!(
            s.actions()
                .contains(&&Action::Send(Command::Timers(TimerCommand::Pause(7))))
        );
        st.timers.as_mut().unwrap().timers[0].run = TimerRun::Paused;
        let mut s = Scene::default();
        timers(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("5:00"));
        assert_eq!(s.refresh, None);
    }
}
