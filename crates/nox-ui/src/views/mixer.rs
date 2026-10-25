//! Per-app volume mixer (E9): one row per app with a 0–200 % slider, mute and
//! reset. Values are what the user asked for; nothing is measured here.

use super::{
    Ctx, ListLayout, bar, empty, icon_button, label, module_missing, permission_gate, pill,
    secondary, tertiary,
};
use crate::format;
use crate::geom::{Color, Rect};
use crate::scene::{Action, Align, Scene, Weight};
use nox_core::model::mixer::{MixerApp, MixerCommand, MixerSettings, MixerState};
use nox_core::permissions::{Permission, PermissionState};
use nox_core::{Command, Section};

/// Not at 100 % or muted: the app has a setting Nox must keep applying.
fn customized(a: &MixerApp) -> bool {
    a.muted || (a.volume - 1.0).abs() > f32::EPSILON
}

pub fn page(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let m = match cx.state.mixer.as_ref() {
        Some(m) if enabled => m,
        _ => return module_missing(s, r, Section::Mixer, enabled),
    };
    if let Some(u) = &m.unavailable {
        return empty(
            s,
            r,
            "exclamationmark.triangle",
            "Mixeur indisponible",
            &format::unavailable_full(u),
        );
    }
    // Only a refused permission gates the page: NotDetermined is answered by
    // macOS at the first adjusted app, so sliders must stay usable until then.
    if matches!(
        m.permission,
        PermissionState::Denied | PermissionState::Restricted
    ) {
        permission_gate(
            s,
            cx,
            r,
            Permission::SystemAudio,
            m.permission,
            "Silence reçu : accès probablement refusé.",
            None,
        );
        return;
    }
    if m.apps.is_empty() {
        return empty(
            s,
            r,
            "slider.horizontal.3",
            "Aucune app audio",
            "Les apps qui jouent du son apparaissent ici.",
        );
    }
    list(s, cx, r, m);
}

fn list(s: &mut Scene, cx: &Ctx, r: Rect, m: &MixerState) {
    let (head, body) = r.split_top(20.0, 6.0);
    let adjusted = m.apps.iter().filter(|a| customized(a)).count();
    s.text(
        head.split_right(120.0, 0.0).0,
        format!("{} app(s) · {adjusted} réglée(s)", m.apps.len()),
        tertiary(10.5),
    );
    if adjusted > 0 {
        pill(
            s,
            cx,
            head.split_right(110.0, 0.0).1,
            "Tout réinitialiser",
            false,
            Action::Send(Command::Mixer(MixerCommand::ResetAll)),
        );
    }
    let mut list = ListLayout::begin(s, body, cx.scroll);
    for a in &m.apps {
        let Some(row) = list.row(52.0, 6.0) else {
            continue;
        };
        s.fill(
            row,
            10.0,
            if cx.hovered(row) {
                Color::CARD_HOVER
            } else {
                Color::CARD
            },
        );
        s.text(
            Rect::new(row.x + 10.0, row.y + 6.0, row.w - 190.0, 15.0),
            a.name.clone(),
            label(12.0).weight(Weight::Medium),
        );
        let mut status = vec![];
        if a.playing {
            status.push("joue");
        }
        if a.muted {
            status.push("muet");
        }
        if a.engaged {
            status.push("réglage actif");
        }
        s.text(
            Rect::new(row.max_x() - 230.0, row.y + 8.0, 150.0, 13.0),
            status.join(" · "),
            secondary(9.5).align(Align::Right).color(if a.engaged {
                Color::ACCENT
            } else {
                Color::SECONDARY
            }),
        );
        if customized(a) {
            icon_button(
                s,
                cx,
                Rect::new(row.max_x() - 54.0, row.y + 5.0, 22.0, 22.0),
                "arrow.counterclockwise",
                10.0,
                Action::Send(Command::Mixer(MixerCommand::Reset { app: a.id.clone() })),
                "Remettre à 100 %",
            );
        }
        icon_button(
            s,
            cx,
            Rect::new(row.max_x() - 28.0, row.y + 5.0, 22.0, 22.0),
            if a.muted {
                "speaker.slash.fill"
            } else {
                "speaker.wave.2.fill"
            },
            11.0,
            Action::Send(Command::Mixer(MixerCommand::SetMuted {
                app: a.id.clone(),
                muted: !a.muted,
            })),
            if a.muted {
                "Rétablir le son"
            } else {
                "Couper le son"
            },
        );
        slider(s, row, a);
    }
    list.end(s);
}

/// Keeps `settings.mixer` in step with a command from the page, so a volume
/// survives a restart. Only apps with a bundle id are remembered, and only by
/// bundle id. Returns true when the settings changed.
pub fn remember(mixer: &mut MixerSettings, apps: &[MixerApp], cmd: &MixerCommand) -> bool {
    let bundle = |id: &str| {
        apps.iter()
            .find(|a| a.id == id)
            .and_then(|a| a.bundle_id.clone())
    };
    let before = mixer.clone();
    match cmd {
        MixerCommand::SetVolume { app, volume } => {
            if let Some(b) = bundle(app) {
                if (*volume - 1.0).abs() > f32::EPSILON {
                    mixer.volumes.insert(b, *volume);
                } else {
                    mixer.volumes.remove(&b);
                }
            }
        }
        MixerCommand::SetMuted { app, muted } => {
            if let Some(b) = bundle(app) {
                mixer.muted.retain(|m| *m != b);
                if *muted {
                    mixer.muted.push(b);
                }
            }
        }
        MixerCommand::Reset { app } => {
            if let Some(b) = bundle(app) {
                mixer.volumes.remove(&b);
                mixer.muted.retain(|m| *m != b);
            }
        }
        MixerCommand::ResetAll => {
            mixer.volumes.clear();
            mixer.muted.clear();
        }
        MixerCommand::Visible(_) => {}
    }
    *mixer != before
}

/// 0–200 % slider on the second line of a row; 100 % is marked.
fn slider(s: &mut Scene, row: Rect, a: &MixerApp) {
    let track = Rect::new(row.x + 10.0, row.y + 36.0, row.w - 82.0, 6.0);
    let color = if a.muted {
        Color::SECONDARY
    } else if a.engaged {
        Color::ACCENT
    } else {
        Color::WHITE.with_alpha(0.85)
    };
    bar(s, track, Some(f64::from(a.volume) / 2.0), color);
    s.fill(
        Rect::new(track.mid_x() - 0.5, track.y - 3.0, 1.0, track.h + 6.0),
        0.5,
        Color::gray(1.0, 0.4),
    );
    s.hit(
        Rect::new(track.x, track.y - 8.0, track.w, track.h + 16.0),
        Action::SetAppVolume {
            app: a.id.clone(),
            x: track.x,
            w: track.w,
        },
        "Régler le volume",
    );
    s.text(
        Rect::new(track.max_x() + 8.0, row.y + 31.0, 60.0, 15.0),
        format::percent(f64::from(a.volume) * 100.0),
        label(11.0).align(Align::Right).color(if a.muted {
            Color::TERTIARY
        } else {
            Color::TEXT
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use nox_core::reading::Unavailable;
    use nox_core::{IslandState, Settings};

    fn app(id: &str, name: &str, volume: f32, engaged: bool) -> MixerApp {
        MixerApp {
            id: id.into(),
            name: name.into(),
            bundle_id: Some(id.into()),
            pid: 100,
            playing: true,
            volume,
            muted: false,
            engaged,
        }
    }

    fn enabled() -> Settings {
        let mut s = Settings::default();
        s.set_module_enabled(nox_core::module::ModuleId::Mixer, true);
        s
    }

    #[test]
    fn module_off_shows_missing_state_and_no_controls() {
        let st = IslandState {
            mixer: Some(MixerState {
                apps: vec![app("com.example.a", "Alpha", 0.5, true)],
                ..Default::default()
            }),
            ..Default::default()
        };
        let settings = Settings::default();
        let mut s = Scene::default();
        super::super::page(&mut s, &ctx(&st, &settings), page_rect(), Section::Mixer);
        assert!(s.has_text("Module désactivé"));
        assert!(!s.has_text("Alpha"));
        assert!(s.actions().is_empty());
    }

    #[test]
    fn no_data_yet_is_an_honest_wait() {
        let settings = enabled();
        let st = IslandState::default();
        let mut s = Scene::default();
        super::super::page(&mut s, &ctx(&st, &settings), page_rect(), Section::Mixer);
        assert!(s.has_text("En attente"));
        assert!(s.actions().is_empty());
    }

    #[test]
    fn two_apps_one_engaged_with_commands() {
        let settings = enabled();
        let st = IslandState {
            mixer: Some(MixerState {
                apps: vec![
                    app("com.example.a", "Alpha", 0.3, true),
                    app("com.example.b", "Bravo", 1.0, false),
                ],
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        page(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("Alpha"));
        assert!(s.has_text("Bravo"));
        assert!(s.has_text("réglage actif"), "{:?}", s.texts());
        assert!(s.has_text("30\u{202f}%"), "{:?}", s.texts());
        let a = s.actions();
        assert!(
            a.iter()
                .any(|x| matches!(x, Action::SetAppVolume { app, .. } if app == "com.example.a"))
        );
        assert!(
            a.contains(&&Action::Send(Command::Mixer(MixerCommand::SetMuted {
                app: "com.example.a".into(),
                muted: true,
            })))
        );
        assert!(
            a.contains(&&Action::Send(Command::Mixer(MixerCommand::Reset {
                app: "com.example.a".into(),
            })))
        );
        assert!(a.contains(&&Action::Send(Command::Mixer(MixerCommand::ResetAll))));
        // Bravo sits at 100 % and unmuted: no reset button for it.
        assert!(
            !a.contains(&&Action::Send(Command::Mixer(MixerCommand::Reset {
                app: "com.example.b".into(),
            })))
        );
    }

    #[test]
    fn unavailable_shows_the_full_reason() {
        let settings = enabled();
        let st = IslandState {
            mixer: Some(MixerState {
                apps: vec![app("com.example.a", "Alpha", 1.0, false)],
                unavailable: Some(Unavailable::Error("tap refusé par Core Audio".into())),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        page(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("Mixeur indisponible"));
        assert!(s.has_text("tap refusé par Core Audio"));
        assert!(!s.has_text("Alpha"));
    }

    #[test]
    fn denied_permission_explains_and_links_to_settings() {
        let settings = enabled();
        let st = IslandState {
            mixer: Some(MixerState {
                apps: vec![app("com.example.a", "Alpha", 0.5, true)],
                permission: PermissionState::Denied,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        page(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("Autorisation requise"));
        assert!(!s.has_text("Alpha"));
        let a = s.actions();
        assert!(a.contains(&&Action::OpenUrl(
            Permission::SystemAudio.settings_url().into()
        )));
        assert!(!a.iter().any(|x| matches!(x, Action::SetAppVolume { .. })));
    }

    #[test]
    fn remember_keeps_only_bundle_ids_and_clears_on_reset() {
        let apps = vec![
            app("com.example.a", "Alpha", 1.0, false),
            MixerApp {
                bundle_id: None,
                ..app("pid:42", "Helper", 1.0, false)
            },
        ];
        let mut m = MixerSettings::default();
        let set = |m: &mut MixerSettings, cmd: MixerCommand| remember(m, &apps, &cmd);
        assert!(set(
            &mut m,
            MixerCommand::SetVolume {
                app: "com.example.a".into(),
                volume: 0.4,
            }
        ));
        assert_eq!(m.volumes.get("com.example.a"), Some(&0.4));
        assert!(set(
            &mut m,
            MixerCommand::SetMuted {
                app: "com.example.a".into(),
                muted: true,
            }
        ));
        assert_eq!(m.muted, vec!["com.example.a".to_string()]);
        // An app without a bundle id is never written to disk.
        assert!(!set(
            &mut m,
            MixerCommand::SetVolume {
                app: "pid:42".into(),
                volume: 0.4,
            }
        ));
        assert_eq!(m.volumes.len(), 1);
        // Back to 100 % removes the entry instead of storing 1.0.
        assert!(set(
            &mut m,
            MixerCommand::SetVolume {
                app: "com.example.a".into(),
                volume: 1.0,
            }
        ));
        assert!(m.volumes.is_empty());
        // Still muted, so resetting the app does change the settings.
        assert!(set(
            &mut m,
            MixerCommand::Reset {
                app: "com.example.a".into(),
            }
        ));
        assert!(!set(&mut m, MixerCommand::ResetAll));
        m.volumes.insert("com.example.a".into(), 0.4);
        m.muted.push("com.example.a".into());
        assert!(set(&mut m, MixerCommand::ResetAll));
        assert_eq!(m, MixerSettings::default());
        assert!(!set(&mut m, MixerCommand::Visible(true)));
    }
}
