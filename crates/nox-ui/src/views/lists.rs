//! List pages: notifications, downloads, file shelf, accessories.

use super::{
    Ctx, ListLayout, bar, empty, icon_button, label, module_missing, permission_gate, pill,
    secondary, tertiary,
};
use crate::format;
use crate::geom::{Color, Rect};
use crate::scene::{Action, Align, Scene, Weight};
use nox_core::model::accessories::AccessoryKind;
use nox_core::model::downloads::{DownloadCommand, DownloadStatus};
use nox_core::model::files::FileCommand;
use nox_core::model::notifications::NotificationCommand;
use nox_core::permissions::Permission;
use nox_core::{Command, Reading, Section};

pub fn notifications(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(n) = cx.state.notifications.as_ref() else {
        return module_missing(s, r, Section::Notifications, enabled);
    };
    if !permission_gate(
        s,
        cx,
        r,
        Permission::Accessibility,
        n.access,
        "Nox lit les bannières affichées à l’écran ; elles restent en mémoire.",
        Some(Command::Notifications(NotificationCommand::RequestAccess)),
    ) {
        return;
    }
    let ns = &cx.settings.notifications;
    let items: Vec<_> = n
        .items
        .iter()
        .filter(|i| !ns.excluded_apps.contains(&i.app_name))
        .take(ns.max_items.max(1))
        .collect();
    if items.is_empty() {
        return empty(
            s,
            r,
            "bell",
            "Aucune notification",
            "Les nouvelles bannières apparaîtront ici.",
        );
    }
    let (head, body) = r.split_top(20.0, 6.0);
    s.text(
        head.split_right(120.0, 0.0).0,
        format!("{} notification(s)", items.len()),
        tertiary(10.5),
    );
    pill(
        s,
        cx,
        head.split_right(100.0, 0.0).1,
        "Tout effacer",
        false,
        Action::Send(Command::Notifications(NotificationCommand::ClearAll)),
    );
    let mut list = ListLayout::begin(s, body, cx.scroll);
    for i in items {
        let h = if ns.hide_body || i.body.is_none() {
            38.0
        } else {
            52.0
        };
        let Some(row) = list.row(h, 6.0) else {
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
        s.hit(
            row,
            Action::Send(Command::Notifications(NotificationCommand::OpenApp(i.id))),
            "Ouvrir l’app",
        );
        let inner = row.inset(10.0, 6.0);
        s.text(
            Rect::new(inner.x, inner.y, inner.w - 110.0, 13.0),
            &i.app_name,
            tertiary(10.0).weight(Weight::Semibold),
        );
        s.text(
            Rect::new(inner.max_x() - 100.0, inner.y, 72.0, 13.0),
            format::relative(i.received_at, cx.now),
            tertiary(10.0).align(Align::Right),
        );
        let mut title = i.title.clone();
        if let Some(st) = &i.subtitle {
            title.push_str(" — ");
            title.push_str(st);
        }
        s.text(
            Rect::new(inner.x, inner.y + 13.0, inner.w - 30.0, 15.0),
            title,
            label(12.0).weight(Weight::Medium),
        );
        if !ns.hide_body
            && let Some(b) = &i.body
        {
            s.text(
                Rect::new(inner.x, inner.y + 28.0, inner.w - 30.0, 14.0),
                b,
                secondary(11.0),
            );
        }
        icon_button(
            s,
            cx,
            Rect::new(inner.max_x() - 22.0, inner.y - 2.0, 22.0, 22.0),
            "xmark",
            9.0,
            Action::Send(Command::Notifications(NotificationCommand::Dismiss(i.id))),
            "Effacer",
        );
        s.refresh_in(60.0);
    }
    list.end(s);
}

pub fn downloads(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(d) = cx.state.downloads.as_ref() else {
        return module_missing(s, r, Section::Downloads, enabled);
    };
    if d.items.is_empty() {
        return empty(
            s,
            r,
            "arrow.down.circle",
            "Aucun téléchargement",
            "Les fichiers en cours dans Téléchargements apparaissent ici.",
        );
    }
    let send = |c: DownloadCommand| Action::Send(Command::Downloads(c));
    let mut list = ListLayout::begin(s, r, cx.scroll);
    for it in &d.items {
        let Some(row) = list.row(46.0, 6.0) else {
            continue;
        };
        s.fill(row, 10.0, Color::CARD);
        let inner = row.inset(10.0, 6.0);
        let (sym, col) = match it.status {
            DownloadStatus::InProgress => ("arrow.down.circle", Color::ACCENT),
            DownloadStatus::Finished => ("checkmark.circle.fill", Color::GREEN),
            DownloadStatus::Failed => ("exclamationmark.circle.fill", Color::RED),
            DownloadStatus::Cancelled => ("xmark.circle", Color::TERTIARY),
        };
        s.symbol(
            Rect::new(inner.x, inner.y + 4.0, 22.0, 22.0),
            sym,
            16.0,
            col,
        );
        let tx = inner.x + 30.0;
        let tw = inner.w - 30.0 - 76.0;
        s.text(
            Rect::new(tx, inner.y, tw, 15.0),
            &it.file_name,
            label(12.0).weight(Weight::Medium),
        );
        let mut info = match it.total {
            Some(t) => format!(
                "{} sur {}",
                format::bytes(it.bytes as f64),
                format::bytes(t as f64)
            ),
            None => format::bytes(it.bytes as f64),
        };
        if it.status == DownloadStatus::InProgress
            && let Some(rate) = it.rate
        {
            info.push_str(&format!(" · {}", format::rate(rate)));
            if let (Some(t), true) = (it.total, rate > 0.0) {
                let left = (t.saturating_sub(it.bytes)) as f64 / rate;
                info.push_str(&format!(" · {} restantes (estimé)", format::duration(left)));
            }
        }
        info.push_str(&format!(" · {}", it.source));
        if it.status == DownloadStatus::InProgress {
            bar(
                s,
                Rect::new(tx, inner.y + 19.0, tw, 4.0),
                it.progress,
                Color::ACCENT,
            );
            s.text(
                Rect::new(tx, inner.y + 25.0, tw, 13.0),
                info,
                tertiary(10.0),
            );
        } else {
            s.text(
                Rect::new(tx, inner.y + 18.0, tw, 13.0),
                info,
                tertiary(10.0),
            );
        }
        let bx = inner.max_x() - 22.0;
        icon_button(
            s,
            cx,
            Rect::new(bx, inner.y + 6.0, 22.0, 22.0),
            "xmark",
            9.0,
            send(DownloadCommand::Dismiss(it.id.clone())),
            "Retirer",
        );
        icon_button(
            s,
            cx,
            Rect::new(bx - 24.0, inner.y + 6.0, 22.0, 22.0),
            "magnifyingglass",
            10.0,
            send(DownloadCommand::Reveal(it.id.clone())),
            "Afficher dans le Finder",
        );
        if it.status == DownloadStatus::Finished {
            icon_button(
                s,
                cx,
                Rect::new(bx - 48.0, inner.y + 6.0, 22.0, 22.0),
                "arrow.up.forward.app",
                10.0,
                send(DownloadCommand::Open(it.id.clone())),
                "Ouvrir",
            );
        }
    }
    list.end(s);
}

pub fn files(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(f) = cx.state.files.as_ref() else {
        return module_missing(s, r, Section::Files, enabled);
    };
    let send = |c: FileCommand| Action::Send(Command::Files(c));
    if f.items.is_empty() {
        s.stroke(r.inset(2.0, 2.0), 14.0, Color::gray(1.0, 0.25), 1.5);
        empty(
            s,
            r,
            "tray.and.arrow.down",
            "Déposez des fichiers ici",
            "Ils restent à leur place : l’étagère garde seulement un lien.",
        );
        s.hit(r, Action::PickFiles, "Choisir des fichiers");
        return;
    }
    let ids: Vec<u64> = f.items.iter().map(|i| i.id).collect();
    let (head, body) = r.split_top(22.0, 8.0);
    let actions = [
        ("AirDrop", send(FileCommand::AirDrop(ids.clone()))),
        ("Compresser", send(FileCommand::Compress(ids.clone()))),
        ("Copier les chemins", send(FileCommand::CopyPaths(ids))),
        ("Vider", send(FileCommand::Clear)),
        ("Ajouter…", Action::PickFiles),
    ];
    let mut x = head.x;
    for (t, a) in actions {
        let w = 14.0 + t.chars().count() as f64 * 6.2;
        pill(s, cx, Rect::new(x, head.y, w, 22.0), t, false, a);
        x += w + 6.0;
    }
    // Tiles.
    let tile_w = 92.0;
    let tile_h = 70.0;
    let cols = ((body.w + 8.0) / (tile_w + 8.0)).floor().max(1.0) as usize;
    let mut list = ListLayout::begin(s, body, cx.scroll);
    for chunk in f.items.chunks(cols) {
        let Some(row) = list.row(tile_h, 8.0) else {
            continue;
        };
        for (i, it) in chunk.iter().enumerate() {
            let t = Rect::new(row.x + i as f64 * (tile_w + 8.0), row.y, tile_w, tile_h);
            s.fill(
                t,
                10.0,
                if cx.hovered(t) {
                    Color::CARD_HOVER
                } else {
                    Color::CARD
                },
            );
            s.symbol(
                Rect::new(t.mid_x() - 14.0, t.y + 6.0, 28.0, 26.0),
                if it.is_dir { "folder.fill" } else { "doc.fill" },
                20.0,
                Color::SECONDARY,
            );
            s.text(
                Rect::new(t.x + 4.0, t.y + 34.0, t.w - 8.0, 14.0),
                &it.name,
                label(10.5).align(Align::Center),
            );
            if let Some(sz) = it.size {
                s.text(
                    Rect::new(t.x + 4.0, t.y + 48.0, t.w - 8.0, 12.0),
                    format::bytes(sz as f64),
                    tertiary(9.0).align(Align::Center),
                );
            }
            s.hit(t, send(FileCommand::Open(it.id)), "Ouvrir");
            icon_button(
                s,
                cx,
                Rect::new(t.max_x() - 20.0, t.y + 2.0, 18.0, 18.0),
                "xmark",
                8.0,
                send(FileCommand::Remove(it.id)),
                "Retirer de l’étagère",
            );
            icon_button(
                s,
                cx,
                Rect::new(t.x + 2.0, t.y + 2.0, 18.0, 18.0),
                "magnifyingglass",
                8.0,
                send(FileCommand::Reveal(it.id)),
                "Afficher dans le Finder",
            );
        }
    }
    list.end(s);
}

fn accessory_symbol(k: AccessoryKind) -> &'static str {
    match k {
        AccessoryKind::Headphones => "headphones",
        AccessoryKind::Keyboard => "keyboard",
        AccessoryKind::Mouse => "computermouse",
        AccessoryKind::Trackpad => "rectangle.and.hand.point.up.left",
        AccessoryKind::GameController => "gamecontroller",
        AccessoryKind::Phone => "iphone",
        AccessoryKind::Other => "dot.radiowaves.left.and.right",
    }
}

fn part_label(p: &str) -> &str {
    match p {
        "Left" => "G",
        "Right" => "D",
        "Case" => "Boîtier",
        other => other,
    }
}

pub fn accessories(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(a) = cx.state.accessories.as_ref() else {
        return module_missing(s, r, Section::Accessories, enabled);
    };
    let devices: Vec<_> = a.devices.iter().filter(|d| d.connected).collect();
    if devices.is_empty() {
        return empty(
            s,
            r,
            "headphones",
            "Aucun accessoire connecté",
            "AirPods, claviers, souris et manettes apparaissent ici.",
        );
    }
    let low = cx.settings.accessories.low_battery_percent;
    let mut list = ListLayout::begin(s, r, cx.scroll);
    for d in devices {
        let Some(row) = list.row(40.0, 6.0) else {
            continue;
        };
        s.fill(row, 10.0, Color::CARD);
        s.symbol(
            Rect::new(row.x + 8.0, row.y + 8.0, 24.0, 24.0),
            accessory_symbol(d.kind),
            15.0,
            Color::TEXT,
        );
        s.text(
            Rect::new(row.x + 40.0, row.y + 12.0, row.w * 0.45, 16.0),
            &d.name,
            label(12.0).weight(Weight::Medium),
        );
        let right = Rect::new(row.x + row.w * 0.5, row.y + 12.0, row.w * 0.5 - 12.0, 16.0);
        match &d.battery {
            Reading::Measured(parts) | Reading::Estimated(parts) if !parts.is_empty() => {
                let txt: Vec<String> = parts
                    .iter()
                    .map(|p| {
                        let mut t = String::new();
                        let pl = part_label(&p.part);
                        if !pl.is_empty() {
                            t.push_str(pl);
                            t.push(' ');
                        }
                        t.push_str(&format::percent(p.percent as f64));
                        if p.charging == Some(true) {
                            t.push_str(" ⚡︎");
                        }
                        t
                    })
                    .collect();
                let any_low = parts.iter().any(|p| low > 0 && p.percent <= low);
                let mut t = txt.join("   ");
                if d.battery.is_estimated() {
                    t.push_str(" (estimé)");
                }
                s.text(
                    right,
                    t,
                    label(11.5).mono().align(Align::Right).color(if any_low {
                        Color::RED
                    } else {
                        Color::TEXT
                    }),
                );
            }
            Reading::Unavailable(u) => s.text(
                right,
                format!("Batterie : {}", format::unavailable(u)),
                tertiary(10.5).align(Align::Right),
            ),
            _ => s.text(
                right,
                "Batterie inconnue",
                tertiary(10.5).align(Align::Right),
            ),
        }
    }
    list.end(s);
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use nox_core::model::accessories::{AccessoriesState, Accessory, BatteryPart};
    use nox_core::model::downloads::{DownloadItem, DownloadsState};
    use nox_core::model::files::{FilesState, ShelfItem};
    use nox_core::{IslandState, Settings, Unavailable};

    #[test]
    fn downloads_progress_and_actions() {
        let settings = Settings::default();
        let st = IslandState {
            downloads: Some(DownloadsState {
                access: nox_core::permissions::PermissionState::Granted,
                items: vec![DownloadItem {
                    id: "d".into(),
                    file_name: "archive.zip".into(),
                    path: "/tmp/archive.zip".into(),
                    progress: Some(0.5),
                    bytes: 50_000_000,
                    total: Some(100_000_000),
                    rate: Some(5_000_000.0),
                    status: DownloadStatus::InProgress,
                    source: "Safari".into(),
                    updated_at: 0.0,
                }],
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        downloads(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(
            s.has_text("50\u{202F}Mo sur 100\u{202F}Mo · 5\u{202F}Mo/s"),
            "{:?}",
            s.texts()
        );
        assert!(s.has_text("(estimé)"));
        assert!(
            s.actions()
                .contains(&&Action::Send(Command::Downloads(DownloadCommand::Reveal(
                    "d".into()
                ))))
        );
    }

    #[test]
    fn shelf_empty_is_a_drop_target_and_items_have_tools() {
        let settings = Settings::default();
        let mut st = IslandState {
            files: Some(FilesState::default()),
            ..Default::default()
        };
        let mut s = Scene::default();
        files(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("Déposez des fichiers"));
        st.files = Some(FilesState {
            compressing: false,
            last_result: None,
            items: vec![ShelfItem {
                id: 3,
                path: "/a/b.txt".into(),
                name: "b.txt".into(),
                is_dir: false,
                size: Some(1200),
            }],
        });
        let mut s = Scene::default();
        files(&mut s, &ctx(&st, &settings), page_rect(), true);
        let a = s.actions();
        assert!(
            a.contains(&&Action::Send(Command::Files(FileCommand::AirDrop(vec![
                3
            ]))))
        );
        assert!(a.contains(&&Action::Send(Command::Files(FileCommand::Remove(3)))));
    }

    #[test]
    fn accessory_battery_parts_and_reasons() {
        let settings = Settings::default();
        let st = IslandState {
            accessories: Some(AccessoriesState {
                bluetooth: nox_core::permissions::PermissionState::Granted,
                devices: vec![
                    Accessory {
                        id: "1".into(),
                        name: "AirPods".into(),
                        kind: AccessoryKind::Headphones,
                        connected: true,
                        battery: Reading::Measured(vec![
                            BatteryPart {
                                part: "Left".into(),
                                percent: 80,
                                charging: None,
                            },
                            BatteryPart {
                                part: "Case".into(),
                                percent: 15,
                                charging: Some(true),
                            },
                        ]),
                    },
                    Accessory {
                        id: "2".into(),
                        name: "Souris".into(),
                        kind: AccessoryKind::Mouse,
                        connected: true,
                        battery: Reading::Unavailable(Unavailable::NotSupported),
                    },
                ],
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        accessories(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("G 80\u{202F}%"));
        assert!(s.has_text("Boîtier 15\u{202F}% ⚡︎"));
        assert!(s.has_text("Batterie : Indisponible"));
    }
}
