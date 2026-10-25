//! Recent screen captures, scratchpad and clipboard history pages.

use super::{
    Ctx, ListLayout, empty, icon_button, label, module_missing, pill, secondary, tertiary,
};
use crate::format;
use crate::geom::{Color, Rect};
use crate::scene::{Action, Align, Op, Scene, Weight};
use nox_core::model::captures::CaptureCommand;
use nox_core::model::clipboard::{ClipKind, ClipboardCommand};
use nox_core::model::scratchpad::ScratchpadCommand;
use nox_core::permissions::PermissionState;
use nox_core::{Command, Section};

/// `~/Desktop` rather than the full home path.
fn short_path(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && p.starts_with(&h) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

pub fn captures(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(c) = cx.state.captures.as_ref() else {
        return module_missing(s, r, Section::Captures, enabled);
    };
    if matches!(
        c.access,
        PermissionState::Denied | PermissionState::Restricted
    ) {
        empty(
            s,
            r.split_top(r.h - 34.0, 0.0).0,
            "lock.slash",
            "Dossier des captures inaccessible",
            &format!(
                "Autorisez Nox à lire « {} » dans Réglages Système › Confidentialité › Fichiers et dossiers.",
                short_path(&c.folder)
            ),
        );
        pill(
            s,
            cx,
            Rect::new(r.mid_x() - 90.0, r.max_y() - 30.0, 180.0, 24.0),
            "Ouvrir les Réglages Système",
            false,
            Action::OpenUrl(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders"
                    .into(),
            ),
        );
        return;
    }
    if c.items.is_empty() {
        return empty(
            s,
            r,
            "photo.on.rectangle",
            "Aucune capture récente",
            &format!(
                "Les captures d’écran de « {} » apparaissent ici.",
                short_path(&c.folder)
            ),
        );
    }
    let send = |cmd: CaptureCommand| Action::Send(Command::Captures(cmd));
    let tw = 132.0;
    let th = 96.0;
    let cols = ((r.w + 8.0) / (tw + 8.0)).floor().max(1.0) as usize;
    let x0 = r.x + (r.w - (cols as f64 * (tw + 8.0) - 8.0)) / 2.0;
    let mut list = ListLayout::begin(s, r, cx.scroll);
    for chunk in c.items.chunks(cols) {
        let Some(row) = list.row(th, 8.0) else {
            continue;
        };
        for (i, it) in chunk.iter().enumerate() {
            let t = Rect::new(x0 + i as f64 * (tw + 8.0), row.y, tw, th);
            let thumb = Rect::new(t.x, t.y, t.w, t.h - 20.0);
            if it.is_video {
                s.fill(thumb, 9.0, Color::CARD_HOVER);
                s.symbol(thumb, "video.fill", 18.0, Color::SECONDARY);
            } else {
                s.ops.push(Op::FileImage {
                    rect: thumb,
                    path: it.path.clone(),
                    radius: 9.0,
                });
            }
            s.text(
                Rect::new(t.x + 2.0, t.max_y() - 17.0, t.w - 4.0, 14.0),
                format::relative(it.created_at, cx.now),
                tertiary(9.5).align(Align::Center),
            );
            s.hit(thumb, send(CaptureCommand::Open(it.id)), &it.name);
            if cx.hovered(t) {
                let b = 20.0;
                let mut x = thumb.max_x() - 4.0 - b;
                icon_button(
                    s,
                    cx,
                    Rect::new(x, thumb.y + 4.0, b, b),
                    "trash",
                    9.0,
                    send(CaptureCommand::MoveToTrash(it.id)),
                    "Mettre à la corbeille",
                );
                x -= b + 4.0;
                icon_button(
                    s,
                    cx,
                    Rect::new(x, thumb.y + 4.0, b, b),
                    "magnifyingglass",
                    9.0,
                    send(CaptureCommand::Reveal(it.id)),
                    "Afficher dans le Finder",
                );
                if !it.is_video {
                    x -= b + 4.0;
                    icon_button(
                        s,
                        cx,
                        Rect::new(x, thumb.y + 4.0, b, b),
                        "doc.on.doc",
                        9.0,
                        send(CaptureCommand::CopyImage(it.id)),
                        "Copier l’image",
                    );
                }
            }
        }
        s.refresh_in(60.0);
    }
    list.end(s);
}

/// The text itself is edited by a native text view placed over
/// `Op::TextEditor`; the scene only draws its frame and status.
pub fn scratchpad(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(p) = cx.state.scratchpad.as_ref() else {
        return module_missing(s, r, Section::Scratchpad, enabled);
    };
    let (body, foot) = r.split_top(r.h - 22.0, 4.0);
    s.fill(body, 10.0, Color::CARD);
    s.ops.push(Op::TextEditor {
        rect: body.inset(8.0, 6.0),
    });
    let status = match (&p.error, p.saved_at) {
        (Some(e), _) => (e.clone(), Color::ORANGE),
        (None, Some(t)) => (
            format!("Enregistré {}", format::relative(t, cx.now)),
            Color::TERTIARY,
        ),
        (None, None) => ("Pas encore enregistré".to_string(), Color::TERTIARY),
    };
    s.text(
        Rect::new(foot.x, foot.y + 4.0, foot.w - 90.0, 13.0),
        status.0,
        tertiary(10.0).color(status.1),
    );
    pill(
        s,
        cx,
        Rect::new(foot.max_x() - 80.0, foot.y, 80.0, 20.0),
        "Effacer",
        false,
        Action::Confirm {
            title: "Effacer le bloc-notes ?".into(),
            detail: "Le texte sera supprimé et enregistré vide.".into(),
            command: Command::Scratchpad(ScratchpadCommand::Clear),
        },
    );
}

fn clip_symbol(k: ClipKind) -> &'static str {
    match k {
        ClipKind::Text => "text.alignleft",
        ClipKind::Url => "link",
        ClipKind::Files(_) => "doc.on.doc",
        ClipKind::Image => "photo",
    }
}

pub fn clipboard(s: &mut Scene, cx: &Ctx, r: Rect, enabled: bool) {
    let Some(c) = cx.state.clipboard.as_ref() else {
        return module_missing(s, r, Section::Clipboard, enabled);
    };
    // Nothing is read unless macOS lets Nox paste without asking each time.
    match c.access {
        PermissionState::Granted | PermissionState::NotRequired => {}
        PermissionState::NotDetermined => {
            empty(
                s,
                r.split_top(r.h - 34.0, 0.0).0,
                "doc.on.clipboard",
                "Autorisation de collage requise",
                "macOS demandera une fois si Nox peut lire le presse-papiers ; choisissez « Toujours autoriser ».",
            );
            pill(
                s,
                cx,
                Rect::new(r.mid_x() - 70.0, r.max_y() - 30.0, 140.0, 24.0),
                "Autoriser…",
                true,
                Action::Send(Command::Clipboard(ClipboardCommand::RequestAccess)),
            );
            return;
        }
        _ => {
            return empty(
                s,
                r,
                "lock.slash",
                "Collage non autorisé",
                "Choisissez « Toujours autoriser » pour Nox dans Réglages Système › Confidentialité et sécurité › Coller depuis d’autres apps.",
            );
        }
    }
    if c.items.is_empty() {
        return empty(
            s,
            r,
            "doc.on.clipboard",
            "Presse-papiers vide",
            "Ce que vous copiez apparaît ici. L’historique reste en mémoire et disparaît à la fermeture de Nox.",
        );
    }
    let (head, body) = r.split_top(20.0, 6.0);
    s.text(
        head.split_right(110.0, 0.0).0,
        format!("{} élément(s) · en mémoire seulement", c.items.len()),
        tertiary(10.5),
    );
    pill(
        s,
        cx,
        head.split_right(100.0, 0.0).1,
        "Tout effacer",
        false,
        Action::Send(Command::Clipboard(ClipboardCommand::Clear)),
    );
    let mut list = ListLayout::begin(s, body, cx.scroll);
    for e in &c.items {
        let Some(row) = list.row(40.0, 6.0) else {
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
            Action::Send(Command::Clipboard(ClipboardCommand::Restore(e.id))),
            "Remettre dans le presse-papiers",
        );
        s.symbol(
            Rect::new(row.x + 8.0, row.y + 9.0, 22.0, 22.0),
            clip_symbol(e.kind),
            13.0,
            Color::SECONDARY,
        );
        let preview = match e.kind {
            ClipKind::Files(n) if e.preview.is_empty() => format!("{n} fichier(s)"),
            ClipKind::Image if e.preview.is_empty() => "Image".into(),
            _ => e.preview.replace('\n', " ⏎ "),
        };
        s.text(
            Rect::new(row.x + 38.0, row.y + 5.0, row.w - 74.0, 15.0),
            preview,
            label(11.5).weight(Weight::Medium),
        );
        let mut meta = format::relative(e.copied_at, cx.now);
        if let Some(app) = &e.source_app {
            meta.push_str(" · ");
            meta.push_str(app);
        }
        meta.push_str(" · ");
        meta.push_str(&format::bytes(e.byte_len as f64));
        s.text(
            Rect::new(row.x + 38.0, row.y + 21.0, row.w - 74.0, 13.0),
            meta,
            secondary(9.5),
        );
        icon_button(
            s,
            cx,
            Rect::new(row.max_x() - 30.0, row.y + 9.0, 22.0, 22.0),
            "xmark",
            9.0,
            Action::Send(Command::Clipboard(ClipboardCommand::Remove(e.id))),
            "Retirer",
        );
        s.refresh_in(60.0);
    }
    list.end(s);
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use nox_core::model::captures::{CaptureItem, CapturesState};
    use nox_core::model::clipboard::{ClipEntry, ClipboardState};
    use nox_core::model::scratchpad::ScratchpadState;
    use nox_core::{IslandState, Settings};

    #[test]
    fn captures_thumbnails_and_actions() {
        let settings = Settings::default();
        let st = IslandState {
            captures: Some(CapturesState {
                access: PermissionState::Granted,
                folder: "Bureau".into(),
                items: vec![CaptureItem {
                    id: 4,
                    path: "/tmp/c.png".into(),
                    name: "Capture.png".into(),
                    created_at: 1_800_000_000.0 - 120.0,
                    size: Some(1),
                    is_video: false,
                }],
            }),
            ..Default::default()
        };
        let mut cx = ctx(&st, &settings);
        cx.hover = Some(crate::geom::Point::new(100.0, 80.0));
        let mut s = Scene::default();
        captures(&mut s, &cx, page_rect(), true);
        assert!(s.ops.iter().any(|o| matches!(o, Op::FileImage { .. })));
        assert!(
            s.actions()
                .contains(&&Action::Send(Command::Captures(CaptureCommand::Open(4))))
        );
        assert!(s.has_text("il y a 2"));
        let denied = IslandState {
            captures: Some(CapturesState {
                access: PermissionState::Denied,
                folder: "Bureau".into(),
                items: vec![],
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        captures(&mut s, &ctx(&denied, &settings), page_rect(), true);
        assert!(s.has_text("inaccessible"));
    }

    #[test]
    fn scratchpad_reserves_editor_and_shows_status() {
        let settings = Settings::default();
        let st = IslandState {
            scratchpad: Some(ScratchpadState {
                text: "secret".into(),
                saved_at: None,
                error: Some("texte tronqué (256 Kio maximum)".into()),
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        scratchpad(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.ops.iter().any(|o| matches!(o, Op::TextEditor { .. })));
        assert!(s.has_text("tronqué"));
        assert!(
            !s.has_text("secret"),
            "the text is drawn by the native view only"
        );
        assert!(
            s.actions()
                .iter()
                .any(|a| matches!(a, Action::Confirm { .. }))
        );
    }

    #[test]
    fn clipboard_list_and_commands() {
        let settings = Settings::default();
        let st = IslandState {
            clipboard: Some(ClipboardState {
                access: Default::default(),
                items: vec![ClipEntry {
                    id: 9,
                    kind: ClipKind::Url,
                    preview: "https://example.org".into(),
                    source_app: Some("Safari".into()),
                    copied_at: 1_800_000_000.0 - 30.0,
                    byte_len: 19,
                }],
            }),
            ..Default::default()
        };
        let mut s = Scene::default();
        clipboard(&mut s, &ctx(&st, &settings), page_rect(), true);
        assert!(s.has_text("https://example.org"));
        assert!(s.has_text("maintenant · Safari"));
        let a = s.actions();
        assert!(a.contains(&&Action::Send(Command::Clipboard(
            ClipboardCommand::Restore(9)
        ))));
        assert!(a.contains(&&Action::Send(Command::Clipboard(
            ClipboardCommand::Remove(9)
        ))));
    }
}
