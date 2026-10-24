//! Closed island strips (activity around the camera), transient notices and
//! the animated companion.

use super::music::{bar_levels, equalizer};
use super::{label, secondary, tertiary};
use crate::compact::Activity;
use crate::format;
use crate::geom::{Color, Point, Rect, Size};
use crate::geometry::{NotchGeometry, shoulder};
use crate::scene::{Action, Align, Op, Scene, Weight};
use nox_core::Alert;
use nox_core::model::agents::AgentProvider;
use nox_core::model::companion::{CompanionMood, CompanionSettings, CompanionSide};
use nox_core::model::timers::FocusPhase;
use nox_core::{AlertKind, Section};

/// Wing rectangles left and right of the camera in a strip of `size`.
pub fn wings(g: &NotchGeometry, size: Size) -> (Rect, Rect) {
    let s = shoulder(size.h);
    let cam_w = g.camera_width.min(size.w);
    let cam_x = (size.w - cam_w) / 2.0;
    let pad = 6.0;
    let inset = if g.floats() { size.h / 2.0 } else { s + 4.0 };
    let left = Rect::new(inset, 0.0, (cam_x - pad - inset).max(0.0), size.h);
    let right = Rect::new(
        cam_x + cam_w + pad,
        0.0,
        (size.w - inset - cam_x - cam_w - pad).max(0.0),
        size.h,
    );
    (left, right)
}

pub fn provider_symbol(p: AgentProvider) -> &'static str {
    match p {
        AgentProvider::Claude => "sparkle",
        AgentProvider::Codex => "chevron.left.forwardslash.chevron.right",
        AgentProvider::OpenCode => "terminal",
        AgentProvider::Copilot => "airplane",
    }
}

pub fn section_of(a: &Activity) -> Option<Section> {
    Some(match a {
        Activity::Music { .. } => Section::Music,
        Activity::Timer { .. } | Activity::Stopwatch { .. } => Section::Timers,
        Activity::AgentWaiting { .. } | Activity::AgentWorking { .. } => Section::Agents,
        Activity::KeepAwake { .. } => Section::KeepAwake,
        Activity::Download { .. } => Section::Downloads,
        Activity::Battery { .. } => Section::System,
        Activity::Companion { .. } => return None,
    })
}

/// The closed strip for `activity` in a surface of `size`.
pub fn strip(
    s: &mut Scene,
    g: &NotchGeometry,
    size: Size,
    activity: &Activity,
    now: f64,
    look: &CompanionSettings,
) {
    let (l, r) = wings(g, size);
    let icon = |s: &mut Scene, rect: Rect, name: &str, color: Color| {
        let d = rect.h.min(rect.w).min(20.0);
        s.symbol(
            Rect::new(rect.x, rect.mid_y() - d / 2.0, d, d),
            name,
            d * 0.66,
            color,
        );
    };
    let value = |s: &mut Scene, rect: Rect, text: String, color: Color| {
        s.text(
            Rect::new(rect.x, rect.mid_y() - 8.0, rect.w, 16.0),
            text,
            label(12.0)
                .weight(Weight::Semibold)
                .mono()
                .align(Align::Right)
                .color(color),
        );
    };
    match activity {
        Activity::Music {
            artwork_id,
            has_artwork,
            playing,
            levels,
            levels_measured,
            ..
        } => {
            let d = (size.h - 10.0).clamp(14.0, 24.0);
            let art = Rect::new(l.x, l.mid_y() - d / 2.0, d, d);
            if *has_artwork {
                s.ops.push(Op::Artwork {
                    rect: art,
                    id: *artwork_id,
                    radius: 5.0,
                });
            } else {
                s.fill(art, 5.0, Color::CARD_HOVER);
                s.symbol(art, "music.note", d * 0.5, Color::SECONDARY);
            }
            let eq = Rect::new(r.max_x() - 22.0, r.mid_y() - 7.0, 20.0, 14.0);
            let lv = bar_levels(levels, *levels_measured, *playing, 4, now);
            equalizer(s, eq, &lv, Color::ACCENT);
            if *playing && !levels_measured {
                s.refresh_in(0.1);
            }
        }
        Activity::Timer {
            remaining,
            paused,
            focus,
            ..
        } => {
            let (sym, col) = match focus {
                Some(FocusPhase::Work) => ("brain.head.profile", Color::RED),
                Some(_) => ("leaf", Color::GREEN),
                None => ("timer", Color::ORANGE),
            };
            icon(s, l, sym, col);
            value(
                s,
                r,
                format::clock(remaining.ceil()),
                if *paused { Color::SECONDARY } else { col },
            );
            if !paused {
                s.refresh_in(remaining.fract().max(0.05));
            }
        }
        Activity::Stopwatch { elapsed } => {
            icon(s, l, "stopwatch", Color::ORANGE);
            value(s, r, format::clock(*elapsed), Color::ORANGE);
            s.refresh_in((1.0 - elapsed.fract()).max(0.05));
        }
        Activity::AgentWaiting { provider, .. } => {
            icon(s, l, provider_symbol(*provider), Color::ORANGE);
            icon(
                s,
                Rect::new(r.max_x() - 20.0, r.y, 20.0, r.h),
                "questionmark.bubble.fill",
                Color::ORANGE,
            );
        }
        Activity::AgentWorking {
            provider, sessions, ..
        } => {
            icon(s, l, provider_symbol(*provider), Color::PURPLE);
            if *sessions > 1 {
                value(s, r, sessions.to_string(), Color::PURPLE);
            } else if *sessions == 1 {
                // Three dots, static (no timer for a working agent).
                for i in 0..3 {
                    s.ellipse(
                        Rect::new(r.max_x() - 22.0 + i as f64 * 7.0, r.mid_y() - 2.5, 5.0, 5.0),
                        Color::PURPLE.with_alpha(0.5 + 0.25 * i as f64),
                    );
                }
            } else {
                icon(
                    s,
                    Rect::new(r.max_x() - 20.0, r.y, 20.0, r.h),
                    "checkmark",
                    Color::SECONDARY,
                );
            }
        }
        Activity::KeepAwake { mode, remaining } => {
            icon(s, l, mode.symbol(), Color::ORANGE);
            let t = match remaining {
                Some(rem) => format::duration(*rem),
                None => "∞".into(),
            };
            value(s, r, t, Color::ORANGE);
        }
        Activity::Download { progress, .. } => {
            icon(s, l, "arrow.down.circle.fill", Color::ACCENT);
            let c = Point::new(r.max_x() - 10.0, r.mid_y());
            s.ops.push(Op::Ring {
                center: c,
                radius: 7.0,
                width: 2.5,
                fraction: progress.unwrap_or(0.25).clamp(0.0, 1.0),
                color: Color::ACCENT,
                track: Color::TRACK,
            });
        }
        Activity::Battery {
            percent,
            charging,
            low,
        } => {
            let col = if *charging {
                Color::GREEN
            } else if *low {
                Color::RED
            } else {
                Color::TEXT
            };
            let sym = if *charging {
                "battery.100percent.bolt"
            } else if *percent <= 25.0 {
                "battery.25percent"
            } else {
                "battery.75percent"
            };
            icon(s, l, sym, col);
            value(s, r, format::percent(*percent), col);
        }
        Activity::Companion { mood, reacting, .. } => {
            let d = (size.h - 6.0).min(26.0);
            let at = match look.side {
                CompanionSide::Left => Rect::new(l.x, l.mid_y() - d / 2.0, d, d),
                CompanionSide::Right => Rect::new(r.max_x() - d, r.mid_y() - d / 2.0, d, d),
            };
            companion(s, at, *mood, look, *reacting, now);
        }
    }
}

/// Size of a transient notice.
pub fn notice_size(g: &NotchGeometry) -> Size {
    Size::new(
        (g.camera_width + 2.0 * 110.0)
            .max(360.0)
            .min(g.screen.w - 24.0),
        g.camera_height + 50.0,
    )
}

pub fn alert_symbol_color(kind: AlertKind) -> Color {
    match kind {
        AlertKind::DownloadFinished | AlertKind::AgentFinished => Color::GREEN,
        AlertKind::AgentNeedsInput | AlertKind::AgentLimit | AlertKind::WatchTriggered => {
            Color::ORANGE
        }
        AlertKind::LowBattery | AlertKind::AccessoryLowBattery => Color::RED,
        AlertKind::TimerDone | AlertKind::FocusPhase => Color::ORANGE,
        AlertKind::CalendarSoon | AlertKind::TrackChanged => Color::ACCENT,
        AlertKind::KeepAwake | AlertKind::PowerSource => Color::YELLOW,
        AlertKind::AccessoryConnected | AlertKind::Notification | AlertKind::Info => Color::TEXT,
    }
}

pub fn alert_section(kind: AlertKind) -> Option<Section> {
    Some(match kind {
        AlertKind::DownloadFinished => Section::Downloads,
        AlertKind::AgentFinished | AlertKind::AgentNeedsInput | AlertKind::AgentLimit => {
            Section::Agents
        }
        AlertKind::TimerDone | AlertKind::FocusPhase => Section::Timers,
        AlertKind::LowBattery | AlertKind::PowerSource => Section::System,
        AlertKind::AccessoryConnected | AlertKind::AccessoryLowBattery => Section::Accessories,
        AlertKind::WatchTriggered => Section::Watch,
        AlertKind::CalendarSoon => Section::Calendar,
        AlertKind::Notification => Section::Notifications,
        AlertKind::KeepAwake => Section::KeepAwake,
        AlertKind::TrackChanged => Section::Music,
        AlertKind::Info => return None,
    })
}

pub fn notice(s: &mut Scene, g: &NotchGeometry, size: Size, a: &Alert) {
    let sh = shoulder(size.h);
    let top = g.camera_height + if g.floats() { 2.0 } else { 0.0 };
    let body = Rect::new(
        sh + 16.0,
        top + 4.0,
        size.w - 2.0 * (sh + 16.0),
        size.h - top - 12.0,
    );
    let col = alert_symbol_color(a.kind);
    s.fill(
        Rect::new(body.x, body.mid_y() - 15.0, 30.0, 30.0),
        15.0,
        col.with_alpha(0.18),
    );
    s.symbol(
        Rect::new(body.x, body.mid_y() - 15.0, 30.0, 30.0),
        &a.symbol,
        15.0,
        col,
    );
    let tx = body.x + 40.0;
    let tw = body.w - 40.0 - if a.progress.is_some() { 34.0 } else { 0.0 };
    let (ty, single) = match &a.subtitle {
        Some(_) => (body.mid_y() - 16.0, false),
        None => (body.mid_y() - 8.0, true),
    };
    s.text(
        Rect::new(tx, ty, tw, 16.0),
        &a.title,
        label(13.0).weight(Weight::Semibold),
    );
    if !single && let Some(sub) = &a.subtitle {
        s.text(Rect::new(tx, ty + 17.0, tw, 15.0), sub, secondary(11.0));
    }
    if let Some(p) = a.progress {
        s.ops.push(Op::Ring {
            center: Point::new(body.max_x() - 12.0, body.mid_y()),
            radius: 10.0,
            width: 3.0,
            fraction: p.clamp(0.0, 1.0),
            color: col,
            track: Color::TRACK,
        });
    }
    s.hit(
        Rect::new(0.0, 0.0, size.w, size.h),
        Action::Open(alert_section(a.kind)),
        "Ouvrir",
    );
}

fn mood_color(m: CompanionMood) -> Color {
    match m {
        CompanionMood::Idle => Color::rgba(0.62, 0.70, 1.0, 1.0),
        CompanionMood::Sleepy => Color::rgba(0.55, 0.58, 0.75, 1.0),
        CompanionMood::Happy => Color::rgba(0.48, 0.86, 0.62, 1.0),
        CompanionMood::Dancing => Color::rgba(0.95, 0.55, 0.85, 1.0),
        CompanionMood::Busy => Color::rgba(0.72, 0.52, 1.0, 1.0),
        CompanionMood::Worried => Color::rgba(1.0, 0.62, 0.35, 1.0),
        CompanionMood::Celebrating => Color::rgba(1.0, 0.84, 0.30, 1.0),
    }
}

/// Body colour of a palette name; unknown names fall back to the mood colour.
pub fn palette_color(name: &str) -> Option<Color> {
    Some(match name {
        "pearl" => Color::rgba(0.93, 0.93, 0.96, 1.0),
        "lemon" => Color::rgba(1.0, 0.92, 0.45, 1.0),
        "peach" => Color::rgba(1.0, 0.76, 0.60, 1.0),
        "rose" => Color::rgba(1.0, 0.62, 0.74, 1.0),
        "lilac" => Color::rgba(0.79, 0.69, 1.0, 1.0),
        "sky" => Color::rgba(0.56, 0.78, 1.0, 1.0),
        "mint" => Color::rgba(0.56, 0.92, 0.76, 1.0),
        _ => return None,
    })
}

/// Original companion drawn from primitives; bounces only when the mood is
/// lively or a reaction plays (and then asks for a short refresh).
pub fn companion(
    s: &mut Scene,
    r: Rect,
    mood: CompanionMood,
    look: &CompanionSettings,
    reacting: bool,
    now: f64,
) {
    let character = look.character.as_str();
    let lively = reacting || matches!(mood, CompanionMood::Dancing | CompanionMood::Celebrating);
    let bounce = if lively {
        s.refresh_in(if reacting { 1.0 / 30.0 } else { 0.1 });
        -((now * 7.0).sin().abs()) * r.h * 0.12
    } else {
        0.0
    };
    let c = palette_color(&look.palette).unwrap_or_else(|| mood_color(mood));
    let body = Rect::new(r.x, r.y + r.h * 0.12 + bounce, r.w, r.h * 0.86);
    match character {
        "ghost" => {
            s.fill(
                Rect::new(body.x, body.y, body.w, body.h * 0.9),
                body.w / 2.0,
                c,
            );
            for i in 0..3 {
                let w3 = body.w / 3.0;
                s.ellipse(
                    Rect::new(
                        body.x + i as f64 * w3,
                        body.max_y() - w3 * 0.8,
                        w3,
                        w3 * 0.8,
                    ),
                    c,
                );
            }
        }
        "cat" => {
            let ear = body.w * 0.3;
            s.ops.push(Op::Area {
                points: vec![
                    Point::new(body.x + ear * 0.1, body.y + ear * 0.9),
                    Point::new(body.x + ear * 0.3, body.y - ear * 0.2),
                    Point::new(body.x + ear * 1.1, body.y + ear * 0.5),
                ],
                color: c,
            });
            s.ops.push(Op::Area {
                points: vec![
                    Point::new(body.max_x() - ear * 0.1, body.y + ear * 0.9),
                    Point::new(body.max_x() - ear * 0.3, body.y - ear * 0.2),
                    Point::new(body.max_x() - ear * 1.1, body.y + ear * 0.5),
                ],
                color: c,
            });
            s.ellipse(body, c);
        }
        _ => s.ellipse(body, c),
    }
    // Face.
    let eye_y = body.y + body.h * 0.38;
    let ew = body.w * 0.14;
    let ex = [body.x + body.w * 0.30, body.x + body.w * 0.70];
    for x in ex {
        match mood {
            CompanionMood::Sleepy => s.fill(
                Rect::new(x - ew / 2.0, eye_y + ew * 0.4, ew, ew * 0.25),
                ew * 0.12,
                Color::BLACK,
            ),
            CompanionMood::Happy | CompanionMood::Celebrating | CompanionMood::Dancing => {
                s.ops.push(Op::Line {
                    points: vec![
                        Point::new(x - ew / 2.0, eye_y + ew * 0.5),
                        Point::new(x, eye_y),
                        Point::new(x + ew / 2.0, eye_y + ew * 0.5),
                    ],
                    color: Color::BLACK,
                    width: (ew * 0.3).max(1.0),
                })
            }
            _ => s.ellipse(Rect::new(x - ew / 2.0, eye_y, ew, ew * 1.2), Color::BLACK),
        }
    }
    let my = body.y + body.h * 0.66;
    let mw = body.w * 0.22;
    let mx = body.mid_x();
    let curve = match mood {
        CompanionMood::Worried => -0.25,
        CompanionMood::Sleepy | CompanionMood::Busy | CompanionMood::Idle => 0.0,
        _ => 0.35,
    };
    s.ops.push(Op::Line {
        points: vec![
            Point::new(mx - mw / 2.0, my),
            Point::new(mx, my + mw * curve),
            Point::new(mx + mw / 2.0, my),
        ],
        color: Color::BLACK,
        width: (ew * 0.28).max(1.0),
    });
}

/// Caption under the companion when the island has room (expanded corner).
pub fn companion_caption(s: &mut Scene, r: Rect, caption: &str) {
    s.text(r, caption, tertiary(10.0).align(Align::Left));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::ScreenParams;
    use nox_core::model::power::KeepAwakeMode;
    use nox_core::settings::IslandSilhouette;

    fn g() -> NotchGeometry {
        NotchGeometry::new(
            &ScreenParams {
                frame: Rect::new(0.0, 0.0, 1470.0, 956.0),
                visible_frame: Rect::new(0.0, 0.0, 1470.0, 919.0),
                safe_area_top: 32.0,
                aux_left: Some(Rect::new(0.0, 924.0, 642.5, 32.0)),
                aux_right: Some(Rect::new(827.5, 924.0, 642.5, 32.0)),
                status_bar_thickness: 24.0,
            },
            true,
            IslandSilhouette::Capsule,
        )
    }

    #[test]
    fn wings_clear_the_camera() {
        let g = g();
        let size = g.compact_size(60.0);
        let (l, r) = wings(&g, size);
        let cam = g.camera_rect_in(size.w);
        assert!(l.max_x() <= cam.x && r.x >= cam.max_x());
        assert!(l.w > 20.0 && r.w > 20.0);
    }

    #[test]
    fn strips_draw_outside_camera_and_refresh_only_when_needed() {
        let g = g();
        let size = g.compact_size(64.0);
        let cam = g.camera_rect_in(size.w);
        let mut s = Scene::default();
        strip(
            &mut s,
            &g,
            size,
            &Activity::Timer {
                label: "x".into(),
                remaining: 75.5,
                total: 100.0,
                paused: false,
                focus: None,
            },
            0.0,
            &CompanionSettings::default(),
        );
        assert!(s.has_text("1:16"));
        assert!(s.refresh.is_some());
        for op in &s.ops {
            if let Op::Text { rect, .. } | Op::Symbol { rect, .. } = op {
                assert!(!rect.intersects(&cam), "{rect:?} under camera");
            }
        }
        let mut s = Scene::default();
        strip(
            &mut s,
            &g,
            size,
            &Activity::KeepAwake {
                mode: KeepAwakeMode::System,
                remaining: None,
            },
            0.0,
            &CompanionSettings::default(),
        );
        assert_eq!(s.refresh, None);
        assert!(s.has_text("∞"));
    }

    #[test]
    fn notice_opens_its_section() {
        let g = g();
        let a =
            Alert::new(AlertKind::DownloadFinished, "Terminé", "checkmark").subtitle("archive.zip");
        let mut s = Scene::default();
        notice(&mut s, &g, notice_size(&g), &a);
        assert!(s.has_text("Terminé") && s.has_text("archive.zip"));
        assert_eq!(s.actions(), vec![&Action::Open(Some(Section::Downloads))]);
    }

    #[test]
    fn companion_animates_only_when_lively() {
        let r = Rect::new(0.0, 0.0, 24.0, 24.0);
        let cat = CompanionSettings {
            character: "cat".into(),
            ..Default::default()
        };
        let mut s = Scene::default();
        companion(&mut s, r, CompanionMood::Sleepy, &cat, false, 0.0);
        assert_eq!(s.refresh, None);
        let ghost = CompanionSettings {
            character: "ghost".into(),
            palette: "mint".into(),
            ..Default::default()
        };
        let mut s = Scene::default();
        companion(&mut s, r, CompanionMood::Dancing, &ghost, false, 0.0);
        assert_eq!(s.refresh, Some(0.1));
        let mut s = Scene::default();
        companion(&mut s, r, CompanionMood::Idle, &ghost, true, 0.0);
        assert!(s.refresh.is_some_and(|x| x < 0.1), "a reaction animates");
        assert!(palette_color("mint").is_some() && palette_color("???").is_none());
    }
}
