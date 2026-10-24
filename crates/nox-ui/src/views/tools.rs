//! Region watch, camera preview and quick shortcuts pages.

use super::{Ctx, empty, label, module_missing, permission_gate, pill, secondary, tertiary};
use crate::format;
use crate::geom::{Color, Rect};
use crate::scene::{Action, Align, Op, Scene, Weight};
use nox_core::model::camera::CameraCommand;
use nox_core::model::shortcuts::{QuickAction, ShortcutCommand};
use nox_core::model::watch::{WatchCommand, WatchPhase, WatchTrigger};
use nox_core::permissions::Permission;
use nox_core::{Command, Section};

pub fn watch(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(w) = cx.state.watch.as_ref() else {
        return module_missing(s, r, Section::Watch, enabled);
    };
    if !permission_gate(
        s,
        cx,
        r,
        Permission::ScreenRecording,
        w.access,
        "La surveillance compare une zone de l’écran choisie par vous ; aucune image n’est gardée.",
        Some(Command::Watch(WatchCommand::RequestAccess)),
    ) {
        return;
    }
    let (status, rest) = r.split_top(56.0, 10.0);
    s.fill(status, 12.0, Color::CARD);
    let inner = status.inset(12.0, 8.0);
    let (sym, col, title) = match w.phase {
        WatchPhase::Idle => ("eye.slash", Color::SECONDARY, "Aucune zone surveillée"),
        WatchPhase::Selecting => ("viewfinder", Color::ACCENT, "Sélection de la zone…"),
        WatchPhase::Watching => ("eye", Color::GREEN, "Surveillance en cours"),
        WatchPhase::Changing => (
            "eye.trianglebadge.exclamationmark",
            Color::ORANGE,
            "La zone change",
        ),
        WatchPhase::Triggered => ("bell.badge", Color::RED, "Alerte déclenchée"),
    };
    s.symbol(
        Rect::new(inner.x, inner.y + 6.0, 26.0, 26.0),
        sym,
        17.0,
        col,
    );
    s.text(
        Rect::new(inner.x + 34.0, inner.y + 1.0, inner.w - 140.0, 16.0),
        title,
        label(12.5).weight(Weight::Semibold),
    );
    let mut detail = vec![];
    if let Some(t) = &w.target {
        detail.push(t.clone());
    }
    if let Some(reg) = &w.region {
        detail.push(format!("{}×{} pt", reg.w.round(), reg.h.round()));
    }
    if let Some(t) = w.last_change_at {
        detail.push(format!(
            "dernier changement {}",
            format::relative(t, cx.now)
        ));
        s.refresh_in(30.0);
    }
    if let Some(d) = w.last_diff {
        detail.push(format!("écart {}", format::percent(d * 100.0)));
    }
    s.text(
        Rect::new(inner.x + 34.0, inner.y + 19.0, inner.w - 140.0, 14.0),
        detail.join(" · "),
        secondary(10.5),
    );
    let active = matches!(
        w.phase,
        WatchPhase::Watching | WatchPhase::Changing | WatchPhase::Triggered
    );
    if active {
        pill(
            s,
            cx,
            Rect::new(inner.max_x() - 90.0, inner.y + 6.0, 90.0, 24.0),
            "Arrêter",
            true,
            Action::Send(Command::Watch(WatchCommand::Stop)),
        );
    }
    let (row1, row2) = rest.split_top(24.0, 10.0);
    s.text(
        Rect::new(row1.x, row1.y + 5.0, 110.0, 14.0),
        "Alerter quand :",
        secondary(11.0),
    );
    let half = (row1.w - 120.0 - 6.0) / 2.0;
    pill(
        s,
        cx,
        Rect::new(row1.x + 120.0, row1.y, half, 24.0),
        "Ça change",
        w.trigger == WatchTrigger::AnyChange,
        Action::BeginWatchSelection(WatchTrigger::AnyChange),
    );
    pill(
        s,
        cx,
        Rect::new(row1.x + 126.0 + half, row1.y, half, 24.0),
        "Ça s’arrête de changer",
        w.trigger == WatchTrigger::Settled,
        Action::BeginWatchSelection(WatchTrigger::Settled),
    );
    s.text(
        Rect::new(row2.x, row2.y, row2.w, 30.0),
        format!(
            "Choisissez un mode puis tracez la zone à l’écran (Échap pour annuler). Stabilité : {} s.",
            cx.settings.watch.settle_secs
        ),
        tertiary(10.5).lines(2),
    );
}

pub fn camera(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(c) = cx.state.camera.as_ref() else {
        return module_missing(s, r, Section::Camera, enabled);
    };
    if !permission_gate(
        s,
        cx,
        r,
        Permission::Camera,
        c.access,
        "L’aperçu s’allume seulement quand cette page est ouverte ; rien n’est enregistré.",
        Some(Command::Camera(CameraCommand::RequestAccess)),
    ) {
        return;
    }
    s.shows_camera = true;
    let w = r.w.min(r.h * 16.0 / 9.0);
    let h = r.h.min(r.w * 3.0 / 4.0).min(w * 9.0 / 16.0 * 1.34);
    let pr = Rect::new(r.mid_x() - w / 2.0, r.y, w, h.min(r.h - 26.0));
    s.fill(pr, 14.0, Color::gray(0.08, 1.0));
    if cx.camera_attached && c.running {
        s.ops.push(Op::CameraHost {
            rect: pr,
            radius: 14.0,
        });
    } else {
        let msg = if c.devices.is_empty() {
            "Aucune caméra détectée"
        } else if !cx.camera_attached {
            "Aperçu en attente du module caméra"
        } else {
            "Démarrage de la caméra…"
        };
        s.symbol(
            Rect::new(pr.mid_x() - 16.0, pr.mid_y() - 22.0, 32.0, 28.0),
            "web.camera",
            22.0,
            Color::TERTIARY,
        );
        s.text(
            Rect::new(pr.x, pr.mid_y() + 10.0, pr.w, 16.0),
            msg,
            secondary(11.5).align(Align::Center),
        );
    }
    // Device picker.
    let row = Rect::new(r.x, r.max_y() - 22.0, r.w, 22.0);
    let mut x = row.x;
    for d in c.devices.iter().take(3) {
        let wpx = (16.0 + d.chars().count() as f64 * 6.0).min(row.w / 3.0 - 6.0);
        pill(
            s,
            cx,
            Rect::new(x, row.y, wpx, 22.0),
            d,
            c.selected.as_deref() == Some(d.as_str()),
            Action::Send(Command::Camera(CameraCommand::Select(d.clone()))),
        );
        x += wpx + 6.0;
    }
}

fn quick_label(a: &QuickAction) -> String {
    match a {
        QuickAction::LockScreen => "Verrouiller".into(),
        QuickAction::Screenshot => "Capture".into(),
        QuickAction::ToggleDarkMode => "Mode sombre".into(),
        QuickAction::ToggleDoNotDisturb => "Ne pas déranger".into(),
        QuickAction::OpenApp(a) => a.clone(),
        QuickAction::RunShortcut(n) => n.clone(),
        QuickAction::EmptyTrash => "Vider la corbeille".into(),
        QuickAction::StartScreenSaver => "Économiseur".into(),
        QuickAction::OpenUrl(u) => u.clone(),
    }
}

pub fn shortcuts(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(st) = cx.state.shortcuts.as_ref() else {
        return module_missing(s, r, Section::Shortcuts, enabled);
    };
    let mut tiles: Vec<(String, String, QuickAction)> = cx
        .settings
        .shortcuts
        .entries
        .iter()
        .map(|e| (e.label.clone(), e.symbol.clone(), e.action.clone()))
        .collect();
    for name in st.user_shortcuts.iter().take(8) {
        tiles.push((
            name.clone(),
            "square.2.layers.3d".into(),
            QuickAction::RunShortcut(name.clone()),
        ));
    }
    if tiles.is_empty() {
        return empty(
            s,
            r,
            "square.grid.2x2",
            "Aucun raccourci",
            "Ajoutez des actions rapides dans les réglages.",
        );
    }
    let (grid, foot) = r.split_top(r.h - 18.0, 2.0);
    let tw = 76.0;
    let th = 66.0;
    let cols = ((grid.w + 8.0) / (tw + 8.0)).floor().max(1.0) as usize;
    let rows = ((grid.h + 8.0) / (th + 8.0)).floor().max(1.0) as usize;
    let used_w = cols as f64 * (tw + 8.0) - 8.0;
    let x0 = grid.mid_x() - used_w / 2.0;
    for (i, (lbl, sym, act)) in tiles.iter().take(cols * rows).enumerate() {
        let t = Rect::new(
            x0 + (i % cols) as f64 * (tw + 8.0),
            grid.y + (i / cols) as f64 * (th + 8.0),
            tw,
            th,
        );
        s.fill(
            t,
            12.0,
            if cx.hovered(t) {
                Color::CARD_HOVER
            } else {
                Color::CARD
            },
        );
        s.symbol(
            Rect::new(t.mid_x() - 15.0, t.y + 9.0, 30.0, 28.0),
            sym,
            19.0,
            Color::TEXT,
        );
        let text = if lbl.is_empty() {
            quick_label(act)
        } else {
            lbl.clone()
        };
        s.text(
            Rect::new(t.x + 4.0, t.y + 42.0, t.w - 8.0, 14.0),
            &text,
            label(10.0).align(Align::Center),
        );
        s.hit(
            t,
            Action::Send(Command::Shortcuts(ShortcutCommand::Run(act.clone()))),
            text,
        );
    }
    let last = match &st.last_result {
        Some(Ok(m)) => format!("✓ {m}"),
        Some(Err(e)) => format!("Échec : {e}"),
        None => String::new(),
    };
    s.text(foot, last, tertiary(10.0).align(Align::Center));
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use nox_core::model::camera::CameraState;
    use nox_core::model::shortcuts::ShortcutsState;
    use nox_core::model::watch::WatchState;
    use nox_core::permissions::PermissionState;
    use nox_core::{IslandState, Settings};

    #[test]
    fn camera_page_only_requests_preview_when_granted() {
        let settings = Settings::default();
        let mut st = IslandState {
            camera: Some(CameraState {
                access: PermissionState::NotDetermined,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        camera(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(!s.shows_camera);
        st.camera = Some(CameraState {
            access: PermissionState::Granted,
            running: true,
            devices: vec!["Caméra FaceTime HD".into()],
            selected: None,
        });
        let mut s = Scene::default();
        camera(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.shows_camera);
        assert!(s.has_text("en attente du module"), "honest without a layer");
        let mut cx = ctx(&st, &settings);
        cx.camera_attached = true;
        let mut s = Scene::default();
        camera(&mut s, &cx, page_rect(), true);
        assert!(s.ops.iter().any(|o| matches!(o, Op::CameraHost { .. })));
    }

    #[test]
    fn watch_selection_and_shortcuts() {
        let settings = Settings::default();
        let st = IslandState {
            watch: Some(WatchState {
                access: PermissionState::Granted,
                ..Default::default()
            }),
            shortcuts: Some(ShortcutsState {
                user_shortcuts: vec!["Mon flux".into()],
                last_result: Some(Err("refusé".into())),
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        watch(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(
            s.actions()
                .contains(&&Action::BeginWatchSelection(WatchTrigger::AnyChange))
        );
        let mut s = Scene::default();
        shortcuts(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("Mon flux") && s.has_text("Échec : refusé"));
        assert!(
            s.actions()
                .contains(&&Action::Send(Command::Shortcuts(ShortcutCommand::Run(
                    QuickAction::LockScreen
                ))))
        );
    }
}
