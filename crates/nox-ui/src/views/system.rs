//! System page (live tiles + mini graphs) and keep-awake page.

use super::{
    Ctx, card, empty, label, module_missing, pill, pill_row, secondary, sparkline, tertiary,
};
use crate::format;
use crate::geom::{Color, Rect};
use crate::scene::{Action, Align, Scene, Weight};
use nox_core::model::metrics::{MemoryPressure, MetricsSnapshot, Series};
use nox_core::model::power::{HelperStatus, KeepAwakeDuration, KeepAwakeMode, PowerCommand};
use nox_core::{Command, Reading, Section};

struct Tile {
    title: &'static str,
    /// The main reading has a value (otherwise `value` holds the reason).
    available: bool,
    value: String,
    detail: String,
    series: Option<(Series, Option<f64>)>,
    color: Color,
}

fn tiles(m: &MetricsSnapshot) -> Vec<Tile> {
    let mem = &m.memory;
    let bat = &m.battery;
    let net = &m.network;
    vec![
        Tile {
            title: "CPU",
            available: m.cpu_total.value().is_some(),
            value: format::reading(&m.cpu_total, |v| format::percent(*v)),
            detail: format::reading(&m.cpu_temp, |v| format::celsius(*v)),
            series: Some((Series::Cpu, Some(100.0))),
            color: Color::ACCENT,
        },
        Tile {
            title: "GPU",
            available: m.gpu_usage.value().is_some(),
            value: format::reading(&m.gpu_usage, |v| format::percent(*v)),
            detail: format::reading(&m.gpu_temp, |v| format::celsius(*v)),
            series: Some((Series::Gpu, Some(100.0))),
            color: Color::PURPLE,
        },
        Tile {
            title: "Mémoire",
            available: mem.value().is_some(),
            value: format::reading(mem, |v| {
                format!(
                    "{} / {}",
                    format::bytes(v.used as f64),
                    format::bytes(v.total as f64)
                )
            }),
            detail: match mem.value().map(|v| &v.pressure) {
                Some(p) => format::reading(p, |p| {
                    match p {
                        MemoryPressure::Normal => "Pression normale",
                        MemoryPressure::Warning => "Pression élevée",
                        MemoryPressure::Critical => "Pression critique",
                    }
                    .to_string()
                }),
                None => String::new(),
            },
            series: Some((Series::Memory, Some(100.0))),
            color: Color::GREEN,
        },
        Tile {
            title: "Batterie",
            available: bat.value().is_some(),
            value: format::reading(bat, |b| format::percent(b.percent)),
            detail: match bat.value() {
                Some(b) => {
                    let state = if b.charging {
                        "En charge"
                    } else if b.on_ac {
                        "Sur secteur"
                    } else {
                        "Sur batterie"
                    };
                    match b.time_remaining_min.value() {
                        Some(min) => format!(
                            "{state} · {}",
                            format::reading(&b.time_remaining_min, |_| format::duration(
                                *min as f64 * 60.0
                            ))
                        ),
                        None => state.to_string(),
                    }
                }
                None => String::new(),
            },
            series: None,
            color: Color::YELLOW,
        },
        Tile {
            title: "Réseau",
            available: net.value().is_some(),
            value: format::reading(net, |n| format!("↓ {}", format::rate(n.rx_bytes_per_sec))),
            detail: format::reading(net, |n| format!("↑ {}", format::rate(n.tx_bytes_per_sec))),
            series: Some((Series::NetRx, None)),
            color: Color::ORANGE,
        },
        Tile {
            title: "Puissance",
            available: m
                .power
                .value()
                .and_then(|p| p.system_watts.value())
                .is_some(),
            value: format::reading(&m.power, |p| {
                format::reading(&p.system_watts, |w| format::watts(*w))
            }),
            detail: format::reading(&m.power, |p| {
                format!(
                    "CPU {}",
                    format::reading(&p.cpu_watts, |w| format::watts(*w))
                )
            }),
            series: Some((Series::SystemPower, None)),
            color: Color::RED,
        },
    ]
}

pub fn page(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    s.shows_metrics = true;
    let Some(m) = cx.state.metrics.as_ref() else {
        return module_missing(s, r, Section::System, enabled);
    };
    let all = tiles(&m.snapshot);
    let cols = 3usize;
    let rows = all.len().div_ceil(cols);
    let gap = 8.0;
    let tw = (r.w - gap * (cols as f64 - 1.0)) / cols as f64;
    let th = ((r.h - 28.0 - gap * (rows as f64 - 1.0)) / rows as f64).max(40.0);
    for (i, t) in all.iter().enumerate() {
        let (cx_i, cy_i) = (i % cols, i / cols);
        let tr = Rect::new(
            r.x + cx_i as f64 * (tw + gap),
            r.y + cy_i as f64 * (th + gap),
            tw,
            th,
        );
        card(s, tr);
        let inner = tr.inset(10.0, 7.0);
        s.text(
            Rect::new(inner.x, inner.y, inner.w, 12.0),
            t.title,
            tertiary(9.5).weight(Weight::Semibold),
        );
        if t.available {
            let value_w = if t.series.is_some() {
                inner.w * 0.55
            } else {
                inner.w
            };
            s.text(
                Rect::new(inner.x, inner.y + 12.0, value_w, 18.0),
                &t.value,
                label(13.5).weight(Weight::Semibold).mono(),
            );
            s.text(
                Rect::new(inner.x, inner.y + 30.0, inner.w, 12.0),
                &t.detail,
                secondary(9.5),
            );
        } else {
            // The reason, never a zero; no graph without a value.
            s.text(
                Rect::new(inner.x, inner.y + 14.0, inner.w, 26.0),
                &t.value,
                secondary(10.5).lines(2),
            );
        }
        if let (true, Some((series, max))) = (t.available, t.series) {
            let g = Rect::new(
                inner.x + inner.w * 0.55,
                inner.y + 2.0,
                inner.w * 0.45,
                24.0,
            );
            sparkline(s, g, &m.history, series, max, t.color);
        }
    }
    let foot = Rect::new(r.x, r.max_y() - 22.0, r.w, 22.0);
    pill(
        s,
        cx,
        foot.split_right(170.0, 0.0).1,
        "Ouvrir le moniteur système",
        false,
        Action::OpenMonitor,
    );
    s.text(
        Rect::new(foot.x, foot.y + 5.0, foot.w - 180.0, 13.0),
        format!(
            "Mesure toutes les {} s",
            format::decimal(cx.settings.metrics.interval_ms as f64 / 1000.0, 1)
        )
        .replace(",0 s", " s"),
        tertiary(9.5),
    );
}

pub fn mode_short(m: KeepAwakeMode) -> &'static str {
    match m {
        KeepAwakeMode::System => "Mac actif",
        KeepAwakeMode::SystemAndDisplay => "Mac + écran",
        KeepAwakeMode::ClosedLid => "Capot fermé",
    }
}

pub fn keep_awake(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(p) = cx.state.power.as_ref() else {
        return module_missing(s, r, Section::KeepAwake, enabled);
    };
    let ps = &cx.settings.power;
    // Status card.
    let (status, rest) = r.split_top(48.0, 10.0);
    card(s, status);
    let inner = status.inset(12.0, 8.0);
    match &p.active {
        Some(a) => {
            s.symbol(
                Rect::new(inner.x, inner.y + 4.0, 24.0, 24.0),
                &format!("{}.fill", a.mode.symbol()),
                16.0,
                Color::ORANGE,
            );
            s.text(
                Rect::new(inner.x + 32.0, inner.y, inner.w - 140.0, 16.0),
                a.mode.label_fr(),
                label(12.5).weight(Weight::Semibold),
            );
            let left = match a.remaining_secs(cx.now) {
                Some(rem) => {
                    s.refresh_in(30.0);
                    format!("encore {}", format::duration(rem))
                }
                None => "durée illimitée".into(),
            };
            s.text(
                Rect::new(inner.x + 32.0, inner.y + 17.0, inner.w - 140.0, 14.0),
                left,
                secondary(11.0),
            );
            pill(
                s,
                cx,
                Rect::new(inner.max_x() - 90.0, inner.y + 4.0, 90.0, 24.0),
                "Arrêter",
                true,
                Action::Send(Command::Power(PowerCommand::Stop)),
            );
        }
        None => {
            s.symbol(
                Rect::new(inner.x, inner.y + 4.0, 24.0, 24.0),
                "cup.and.saucer",
                16.0,
                Color::SECONDARY,
            );
            s.text(
                Rect::new(inner.x + 32.0, inner.y, inner.w - 40.0, 16.0),
                "Inactif — le Mac suit ses réglages de veille",
                label(12.5).weight(Weight::Semibold),
            );
            let last = match (p.last_stop, &p.last_error, &p.notice) {
                (_, Some(e), _) => format!("Dernière erreur : {e}"),
                (_, None, Some(n)) => n.clone(),
                (Some(r), None, None) => format!("Dernier arrêt : {}", r.label_fr()),
                _ => {
                    let mut c = vec![];
                    if ps.only_on_ac {
                        c.push("uniquement sur secteur".to_string());
                    }
                    if ps.stop_below_percent > 0 {
                        c.push(format!(
                            "arrêt sous {}",
                            format::percent(ps.stop_below_percent as f64)
                        ));
                    }
                    c.join(" · ")
                }
            };
            s.text(
                Rect::new(inner.x + 32.0, inner.y + 17.0, inner.w - 40.0, 14.0),
                last,
                secondary(11.0),
            );
        }
    }
    // Start: one row per mode, durations as pills.
    let modes = [
        KeepAwakeMode::System,
        KeepAwakeMode::SystemAndDisplay,
        KeepAwakeMode::ClosedLid,
    ];
    let row_h = 24.0;
    let mut y = rest.y;
    for mode in modes {
        let row = Rect::new(rest.x, y, rest.w, row_h);
        let (name, pills) = row.split_left(96.0, 8.0);
        s.text(
            Rect::new(name.x, name.y + 5.0, name.w, 14.0),
            mode_short(mode),
            secondary(11.0).weight(Weight::Medium),
        );
        if mode.needs_helper() && !p.helper.usable() {
            let (txt, btn) = pills.split_right(170.0, 6.0);
            s.text(
                Rect::new(txt.x, txt.y + 5.0, txt.w, 14.0),
                p.helper.label_fr(),
                tertiary(10.0),
            );
            if p.helper_busy {
                // No action while macOS shows the administrator prompt.
                s.fill(btn, btn.h / 2.0, Color::CARD);
                s.text(
                    btn.inset(6.0, 5.0),
                    "Installation en cours…",
                    tertiary(11.0).align(Align::Center),
                );
            } else {
                pill(
                    s,
                    cx,
                    btn,
                    helper_button(p.helper),
                    false,
                    Action::InstallHelper,
                );
            }
        } else {
            let items: Vec<(String, bool, Action)> = KeepAwakeDuration::PRESETS
                .iter()
                .map(|d| {
                    (
                        d.label_fr(),
                        false,
                        Action::Send(Command::Power(PowerCommand::Start { mode, duration: *d })),
                    )
                })
                .collect();
            pill_row(s, cx, pills, &items, 5.0);
        }
        y += row_h + 6.0;
    }
    let lid = match &p.lid_sleep_disabled {
        Reading::Measured(true) | Reading::Estimated(true) => {
            "veille capot désactivée en ce moment".to_string()
        }
        Reading::Measured(false) | Reading::Estimated(false) => String::new(),
        Reading::Unavailable(u) => format!("veille capot : {}", format::unavailable(u)),
    };
    let line = match (p.helper.usable(), lid.is_empty()) {
        (true, true) => p.helper.label_fr().to_string(),
        (true, false) => format!("{} · {lid}", p.helper.label_fr()),
        (false, _) => lid,
    };
    s.text(
        Rect::new(rest.x, y + 2.0, rest.w, 13.0),
        line,
        tertiary(9.5).align(Align::Right),
    );
}

/// Label of the helper button for a status that is not usable yet.
pub fn helper_button(h: HelperStatus) -> &'static str {
    match h {
        HelperStatus::NotInstalled => "Installer l’assistant…",
        HelperStatus::Outdated => "Mettre à jour l’assistant…",
        HelperStatus::Unreachable | HelperStatus::ClientRejected | HelperStatus::Installed => {
            "Réinstaller l’assistant…"
        }
    }
}

/// Empty metrics view helper reused by the monitor window.
pub fn metrics_unavailable(s: &mut Scene, r: Rect) {
    empty(
        s,
        r,
        "gauge.with.dots.needle.50percent",
        "Mesures indisponibles",
        "Le module Système est désactivé ou n’a pas encore publié de mesure.",
    );
}

/// Whether a battery tile should be tinted (used by compact strips too).
pub fn battery_color(percent: f64, charging: bool) -> Color {
    if charging {
        Color::GREEN
    } else if percent <= 10.0 {
        Color::RED
    } else if percent <= 20.0 {
        Color::ORANGE
    } else {
        Color::TEXT
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use nox_core::model::metrics::MetricsState;
    use nox_core::model::power::{KeepAwakeSession, PowerState};
    use nox_core::{IslandState, Settings, Unavailable};

    #[test]
    fn system_tiles_show_reasons_not_zero() {
        let settings = Settings::default();
        let mut m = MetricsState::default();
        m.snapshot.cpu_total = Reading::Measured(37.2);
        m.snapshot.gpu_usage = Reading::Unavailable(Unavailable::NotSupported);
        m.snapshot.power = Reading::Unavailable(Unavailable::PermissionDenied("SMC".into()));
        let st = IslandState {
            metrics: Some(m),
            ..Default::default()
        };
        let mut s = Scene::default();
        page(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.shows_metrics);
        assert!(s.has_text("37\u{202F}%"));
        assert!(s.has_text("Indisponible"));
        assert!(s.has_text("Autorisation requise : SMC"));
        assert!(!s.has_text("0\u{202F}%"), "{:?}", s.texts());
        assert!(s.has_text("Mesure toutes les 2 s"));
    }

    #[test]
    fn keep_awake_offers_modes_durations_and_helper() {
        let settings = Settings::default();
        let mut st = IslandState {
            power: Some(PowerState::default()),
            ..Default::default()
        };
        let mut s = Scene::default();
        keep_awake(&mut s, &ctx(&st, &settings), page_rect(), true);
        let a = s.actions();
        assert!(a.contains(&&Action::InstallHelper));
        assert!(
            a.contains(&&Action::Send(Command::Power(PowerCommand::Start {
                mode: KeepAwakeMode::SystemAndDisplay,
                duration: KeepAwakeDuration::Minutes(120),
            })))
        );
        assert!(
            !a.iter().any(|a| matches!(
                a,
                Action::Send(Command::Power(PowerCommand::Start {
                    mode: KeepAwakeMode::ClosedLid,
                    ..
                }))
            )),
            "closed lid needs the helper first"
        );
        st.power = Some(PowerState {
            active: Some(KeepAwakeSession {
                mode: KeepAwakeMode::System,
                duration: KeepAwakeDuration::Minutes(30),
                started_at: 0.0,
                ends_at: Some(1_800_000_000.0 + 1800.0),
            }),
            helper: HelperStatus::Installed,
            ..Default::default()
        });
        let mut s = Scene::default();
        keep_awake(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("encore 30\u{202F}min"));
        assert!(
            s.actions()
                .contains(&&Action::Send(Command::Power(PowerCommand::Stop)))
        );
        assert!(s.actions().iter().any(|a| matches!(
            a,
            Action::Send(Command::Power(PowerCommand::Start {
                mode: KeepAwakeMode::ClosedLid,
                ..
            }))
        )));
    }
}
