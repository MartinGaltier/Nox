//! Music page: artwork, title, interpolated timeline with seek, transport,
//! shuffle/repeat, synced lyrics or queue, visualiser.

use super::{Ctx, empty, icon_button, label, module_missing, pill, secondary, tertiary};
use crate::format;
use crate::geom::{Color, Rect};
use crate::scene::{Action, Align, MusicPanel, Op, Scene, Weight};
use nox_core::model::music::{LyricLine, MusicCommand, NowPlaying, RepeatMode};
use nox_core::{Command, Reading, Section};

/// Index of the active synced lyric line at `t`.
pub fn active_line(lines: &[LyricLine], t: f64) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.time.is_some_and(|lt| lt <= t))
        .map(|(i, _)| i)
        .next_back()
}

/// Visualiser bar heights 0..1. Real levels when measured; otherwise a
/// deterministic playback-driven animation (flagged "non mesuré" by callers).
pub fn bar_levels(levels: &[f32], measured: bool, playing: bool, n: usize, t: f64) -> Vec<f64> {
    if measured && !levels.is_empty() {
        return (0..n)
            .map(|i| {
                let j = i * levels.len() / n.max(1);
                f64::from(levels[j.min(levels.len() - 1)]).clamp(0.0, 1.0)
            })
            .collect();
    }
    if !playing {
        return vec![0.15; n];
    }
    (0..n)
        .map(|i| {
            let p = i as f64 * 1.7;
            0.35 + 0.3 * (t * 6.3 + p).sin().abs() + 0.25 * (t * 3.1 + p * 2.3).sin().abs()
        })
        .map(|v| v.clamp(0.1, 1.0))
        .collect()
}

pub fn equalizer(s: &mut Scene, r: Rect, heights: &[f64], color: Color) {
    let n = heights.len().max(1) as f64;
    let bw = (r.w / (n * 1.8)).max(1.5);
    let gap = (r.w - bw * n) / (n - 1.0).max(1.0);
    for (i, h) in heights.iter().enumerate() {
        let bh = (r.h * h).max(bw);
        let x = r.x + i as f64 * (bw + gap);
        s.fill(Rect::new(x, r.mid_y() - bh / 2.0, bw, bh), bw / 2.0, color);
    }
}

fn artwork(s: &mut Scene, r: Rect, np: &NowPlaying) {
    if np.artwork.is_some() {
        s.ops.push(Op::Artwork {
            rect: r,
            id: np.artwork_id,
            radius: 10.0,
        });
    } else {
        s.fill(r, 10.0, Color::CARD_HOVER);
        s.symbol(r, "music.note", r.h * 0.38, Color::TERTIARY);
    }
}

pub fn page(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(m) = cx.state.music.as_ref() else {
        return module_missing(s, r, Section::Music, enabled);
    };
    let Some(np) = m.now_playing.as_ref() else {
        return empty(
            s,
            r,
            "music.note",
            "Rien en lecture",
            "Lancez Musique, Spotify ou une vidéo dans le navigateur.",
        );
    };
    let ms = &cx.settings.music;
    let side_panel = (ms.lyrics || ms.queue) && r.w >= 420.0;
    let (main, side) = if side_panel {
        r.split_right(r.w * 0.40, 14.0)
    } else {
        (r, Rect::default())
    };

    // Head: artwork + text + visualiser.
    let art = Rect::new(main.x, main.y + 2.0, 64.0, 64.0);
    artwork(s, art, np);
    let tx = art.max_x() + 12.0;
    let tw = (main.max_x()
        - tx
        - match (ms.visualizer, m.levels_measured) {
            (true, false) => 52.0,
            (true, true) => 34.0,
            _ => 0.0,
        })
    .max(0.0);
    let title = if np.title.is_empty() {
        "Sans titre"
    } else {
        &np.title
    };
    s.text(
        Rect::new(tx, art.y + 4.0, tw, 18.0),
        title,
        label(14.0).weight(Weight::Semibold),
    );
    let sub: Vec<&str> = [np.artist.as_str(), np.album.as_str()]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect();
    s.text(
        Rect::new(tx, art.y + 23.0, tw, 15.0),
        sub.join(" — "),
        secondary(11.5),
    );
    if let Some(p) = &np.player {
        let pr = Rect::new(tx, art.y + 42.0, tw.min(160.0), 16.0);
        s.text(pr, &p.name, tertiary(10.5));
        s.hit(
            pr,
            Action::Send(Command::Music(MusicCommand::OpenPlayer)),
            "Ouvrir le lecteur",
        );
    }
    if ms.visualizer {
        let er = Rect::new(main.max_x() - 26.0, art.y + 8.0, 24.0, 20.0);
        let lv = bar_levels(&m.levels, m.levels_measured, np.playing, 5, cx.now);
        equalizer(s, er, &lv, Color::ACCENT);
        if !m.levels_measured {
            s.text(
                Rect::new(er.x - 24.0, er.max_y() + 3.0, 50.0, 11.0),
                "non mesuré",
                tertiary(7.5).align(Align::Right),
            );
            if np.playing {
                s.refresh_in(0.1);
            }
        }
    }

    // Timeline.
    let t = np.elapsed_now(cx.now);
    let ty = art.max_y() + 12.0;
    let tl = Rect::new(main.x, ty, main.w, 14.0);
    let (lt, rest) = tl.split_left(40.0, 4.0);
    let (track, rt) = rest.split_right(44.0, 4.0);
    s.text(lt, format::clock(t), tertiary(10.0).mono());
    let barr = Rect::new(track.x, track.mid_y() - 2.0, track.w, 4.0);
    match np.duration {
        Some(d) if d > 0.0 => {
            super::bar(s, barr, Some(t / d), Color::WHITE.with_alpha(0.85));
            s.text(
                rt,
                format!("-{}", format::clock(d - t)),
                tertiary(10.0).mono().align(Align::Right),
            );
            if np.can_seek {
                s.hit(
                    Rect::new(track.x, tl.y - 4.0, track.w, 22.0),
                    Action::Seek {
                        x: track.x,
                        w: track.w,
                        duration: d,
                    },
                    "Avancer / reculer",
                );
            }
        }
        _ => {
            s.fill(barr, 2.0, Color::TRACK);
            s.text(rt, "direct", tertiary(10.0).align(Align::Right));
        }
    }
    if np.playing {
        // Seconds tick over on whole seconds of playback.
        let frac = t.fract();
        s.refresh_in(((1.0 - frac) / np.rate.max(0.1)).max(0.05));
    }

    // Transport.
    let row = Rect::new(main.x, tl.max_y() + 8.0, main.w, 32.0);
    let c = row.mid_x();
    let send = |c: MusicCommand| Action::Send(Command::Music(c));
    if let Some(sh) = np.shuffle {
        icon_button(
            s,
            cx,
            Rect::new(c - 118.0, row.y + 4.0, 24.0, 24.0),
            "shuffle",
            11.0,
            send(MusicCommand::SetShuffle(!sh)),
            "Lecture aléatoire",
        );
        if sh {
            s.fill(
                Rect::new(c - 108.0, row.y + 29.0, 4.0, 4.0),
                2.0,
                Color::ACCENT,
            );
        }
    }
    icon_button(
        s,
        cx,
        Rect::new(c - 74.0, row.y + 2.0, 28.0, 28.0),
        "backward.fill",
        15.0,
        send(MusicCommand::Previous),
        "Précédent",
    );
    icon_button(
        s,
        cx,
        Rect::new(c - 18.0, row.y - 2.0, 36.0, 36.0),
        if np.playing {
            "pause.fill"
        } else {
            "play.fill"
        },
        20.0,
        send(MusicCommand::PlayPause),
        if np.playing { "Pause" } else { "Lecture" },
    );
    icon_button(
        s,
        cx,
        Rect::new(c + 46.0, row.y + 2.0, 28.0, 28.0),
        "forward.fill",
        15.0,
        send(MusicCommand::Next),
        "Suivant",
    );
    if let Some(rep) = np.repeat {
        let (sym, next) = match rep {
            RepeatMode::Off => ("repeat", RepeatMode::All),
            RepeatMode::All => ("repeat", RepeatMode::One),
            RepeatMode::One => ("repeat.1", RepeatMode::Off),
        };
        icon_button(
            s,
            cx,
            Rect::new(c + 94.0, row.y + 4.0, 24.0, 24.0),
            sym,
            11.0,
            send(MusicCommand::SetRepeat(next)),
            "Répétition",
        );
        if rep != RepeatMode::Off {
            s.fill(
                Rect::new(c + 104.0, row.y + 29.0, 4.0, 4.0),
                2.0,
                Color::ACCENT,
            );
        }
    }

    if side_panel {
        side_view(s, cx, side, m, t);
    }
}

fn side_view(s: &mut Scene, cx: &Ctx, r: Rect, m: &nox_core::model::music::MusicState, t: f64) {
    let ms = &cx.settings.music;
    let panel = match (ms.lyrics, ms.queue) {
        (true, false) => MusicPanel::Lyrics,
        (false, true) => MusicPanel::Queue,
        _ => cx.music_panel,
    };
    let (tabs, body) = r.split_top(20.0, 8.0);
    if ms.lyrics && ms.queue {
        let (a, b) = tabs.split_left(tabs.w / 2.0 - 3.0, 6.0);
        pill(
            s,
            cx,
            a,
            "Paroles",
            panel == MusicPanel::Lyrics,
            Action::MusicPanel(MusicPanel::Lyrics),
        );
        pill(
            s,
            cx,
            b,
            "File",
            panel == MusicPanel::Queue,
            Action::MusicPanel(MusicPanel::Queue),
        );
    }
    s.fill(body, 10.0, Color::CARD);
    let inner = body.inset(10.0, 8.0);
    match panel {
        MusicPanel::Lyrics => match &m.lyrics {
            Reading::Measured(l) | Reading::Estimated(l) if !l.lines.is_empty() => {
                let line_h = 17.0;
                let visible = (inner.h / line_h).floor().max(1.0) as usize;
                let active = if l.synced {
                    active_line(&l.lines, t)
                } else {
                    None
                };
                let first = active.map_or(0, |a| a.saturating_sub(1));
                for (k, line) in l.lines.iter().skip(first).take(visible).enumerate() {
                    let i = first + k;
                    let st = if Some(i) == active {
                        label(12.0).weight(Weight::Semibold)
                    } else if active.is_some_and(|a| i < a) {
                        tertiary(11.0)
                    } else {
                        secondary(11.0)
                    };
                    let text = if line.text.trim().is_empty() {
                        "♪"
                    } else {
                        &line.text
                    };
                    s.text(
                        Rect::new(inner.x, inner.y + k as f64 * line_h, inner.w, line_h),
                        text,
                        st,
                    );
                }
                s.text(
                    Rect::new(inner.x, body.max_y() - 14.0, inner.w, 12.0),
                    if l.synced {
                        format!("Synchronisées · {}", l.source)
                    } else {
                        format!("Non synchronisées · {}", l.source)
                    },
                    tertiary(8.5).align(Align::Right),
                );
            }
            Reading::Unavailable(u) => s.text(
                inner,
                format!("Paroles : {}", format::unavailable(u)),
                tertiary(11.0).lines(3),
            ),
            _ => s.text(inner, "Aucune parole trouvée.", tertiary(11.0)),
        },
        MusicPanel::Queue => match &m.queue {
            Reading::Measured(q) | Reading::Estimated(q) if !q.is_empty() => {
                let row_h = 30.0;
                let visible = (inner.h / row_h).floor().max(1.0) as usize;
                for (k, item) in q.iter().take(visible).enumerate() {
                    let y = inner.y + k as f64 * row_h;
                    s.text(
                        Rect::new(inner.x, y, inner.w - 36.0, 15.0),
                        &item.title,
                        label(11.5).weight(Weight::Medium),
                    );
                    s.text(
                        Rect::new(inner.x, y + 14.0, inner.w - 36.0, 13.0),
                        &item.artist,
                        tertiary(10.0),
                    );
                    if let Some(d) = item.duration {
                        s.text(
                            Rect::new(inner.max_x() - 34.0, y, 34.0, 14.0),
                            format::clock(d),
                            tertiary(10.0).mono().align(Align::Right),
                        );
                    }
                }
            }
            Reading::Unavailable(u) => s.text(
                inner,
                format!("File : {}", format::unavailable(u)),
                tertiary(11.0).lines(3),
            ),
            _ => s.text(inner, "File d’attente vide.", tertiary(11.0)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use nox_core::model::music::{Lyrics, MusicState, PlayerInfo};
    use nox_core::{IslandState, Settings, Unavailable};

    fn state(playing: bool) -> IslandState {
        IslandState {
            music: Some(MusicState {
                unavailable: None,
                automation: Default::default(),
                now_playing: Some(NowPlaying {
                    player: Some(PlayerInfo {
                        bundle_id: "com.spotify.client".into(),
                        name: "Spotify".into(),
                    }),
                    title: "Titre".into(),
                    artist: "Artiste".into(),
                    album: "Album".into(),
                    duration: Some(200.0),
                    elapsed: 50.0,
                    observed_at: 1_800_000_000.0 - 10.0,
                    playing,
                    rate: 1.0,
                    shuffle: Some(false),
                    repeat: Some(RepeatMode::Off),
                    can_seek: true,
                    can_skip: true,
                    ..Default::default()
                }),
                queue: Reading::Unavailable(Unavailable::NotSupported),
                lyrics: Reading::Measured(Lyrics {
                    synced: true,
                    lines: vec![
                        LyricLine {
                            time: Some(0.0),
                            text: "un".into(),
                        },
                        LyricLine {
                            time: Some(55.0),
                            text: "deux".into(),
                        },
                        LyricLine {
                            time: Some(70.0),
                            text: "trois".into(),
                        },
                    ],
                    source: "LRCLIB".into(),
                }),
                levels: vec![],
                levels_measured: false,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn interpolates_progress_and_offers_controls() {
        let st = state(true);
        let settings = Settings::default();
        let cx = ctx(&st, &settings);
        let mut s = Scene::default();
        page(&mut s, &cx, page_rect(), true);
        // 50 s at observation + 10 s of playback.
        assert!(s.has_text("1:00"), "{:?}", s.texts());
        assert!(s.has_text("-2:20"));
        assert!(s.has_text("Titre") && s.has_text("Artiste — Album"));
        assert!(s.has_text("non mesuré"), "animated bars are flagged");
        let a = s.actions();
        assert!(a.contains(&&Action::Send(Command::Music(MusicCommand::PlayPause))));
        assert!(
            a.contains(&&Action::Send(Command::Music(MusicCommand::SetShuffle(
                true
            ))))
        );
        assert!(
            a.iter()
                .any(|a| matches!(a, Action::Seek { duration, .. } if *duration == 200.0))
        );
        // Active lyric at 60 s is "deux".
        assert!(s.has_text("deux"));
        assert!(s.refresh.is_some(), "playing needs a refresh");
    }

    #[test]
    fn paused_needs_no_refresh_and_queue_reason_is_shown() {
        let st = state(false);
        let settings = Settings::default();
        let mut cx = ctx(&st, &settings);
        cx.music_panel = MusicPanel::Queue;
        let mut s = Scene::default();
        page(&mut s, &cx, page_rect(), true);
        assert_eq!(s.refresh, None, "paused: no timer");
        assert!(s.has_text("File : Indisponible"));
    }

    #[test]
    fn lyric_selection_and_levels() {
        let lines = vec![
            LyricLine {
                time: Some(1.0),
                text: "a".into(),
            },
            LyricLine {
                time: Some(5.0),
                text: "b".into(),
            },
        ];
        assert_eq!(active_line(&lines, 0.5), None);
        assert_eq!(active_line(&lines, 4.9), Some(0));
        assert_eq!(active_line(&lines, 9.0), Some(1));
        let real = bar_levels(&[0.2, 0.9], true, true, 4, 0.0);
        assert_eq!(
            real,
            vec![0.2f32 as f64, 0.2f32 as f64, 0.9f32 as f64, 0.9f32 as f64]
        );
        assert!(
            bar_levels(&[], false, false, 3, 0.0)
                .iter()
                .all(|v| *v == 0.15)
        );
    }
}
