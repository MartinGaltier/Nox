//! Native settings window (A9, A13): standard AppKit controls in tabs. Every
//! change saves `settings.json` and is forwarded to the module manager
//! (disabling a module really stops it).

use super::app::{self, App};
use super::login::{self, LoginStatus};
use super::util::ActionTarget;
use nox_core::model::agents::{AgentPeriod, AgentProvider};
use nox_core::model::companion::CompanionSide;
use nox_core::model::power::{KeepAwakeDuration, KeepAwakeMode, PowerCommand};
use nox_core::settings::{DisplayChoice, IdleContent, IslandSilhouette, IslandSize};
use nox_core::{Command, ModuleId, Section, Settings};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSControlStateValueOff, NSControlStateValueOn,
    NSLayoutAttribute, NSPopUpButton, NSStackView, NSTabView, NSTabViewItem, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSPoint, NSRect, NSSize, NSString};
use std::rc::Rc;

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    tabs: Retained<NSTabView>,
    targets: Vec<Retained<ActionTarget>>,
}

type Set<T> = Rc<dyn Fn(&mut Settings, T)>;

struct Form {
    mtm: MainThreadMarker,
    rows: Vec<Retained<NSView>>,
    targets: Vec<Retained<ActionTarget>>,
    current: Settings,
}

fn apply(f: impl FnOnce(&mut Settings)) {
    if let Some(a) = app::get() {
        let mut s = (*a.settings()).clone();
        f(&mut s);
        a.save_settings(s);
    }
}

impl Form {
    fn new(mtm: MainThreadMarker, current: Settings) -> Self {
        Self {
            mtm,
            rows: vec![],
            targets: vec![],
            current,
        }
    }

    fn target(&mut self, f: impl Fn(&AnyObject) + 'static) -> Retained<ActionTarget> {
        let t = ActionTarget::new(self.mtm, f);
        self.targets.push(t.clone());
        t
    }

    fn header(&mut self, text: &str) {
        let l = NSTextField::labelWithString(&NSString::from_str(text), self.mtm);
        l.setFont(Some(&objc2_app_kit::NSFont::boldSystemFontOfSize(13.0)));
        self.rows
            .push(Retained::into_super(Retained::into_super(l)));
    }

    fn note(&mut self, text: &str) {
        let l = NSTextField::wrappingLabelWithString(&NSString::from_str(text), self.mtm);
        l.setPreferredMaxLayoutWidth(520.0);
        l.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(11.0)));
        l.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
        self.rows
            .push(Retained::into_super(Retained::into_super(l)));
    }

    fn checkbox(
        &mut self,
        title: &str,
        get: impl Fn(&Settings) -> bool,
        set: impl Fn(&mut Settings, bool) + 'static,
    ) {
        let set: Set<bool> = Rc::new(set);
        let t = self.target(move |sender| {
            let on = sender
                .downcast_ref::<NSButton>()
                .is_some_and(|b| b.state() == NSControlStateValueOn);
            let set = set.clone();
            apply(move |s| set(s, on));
        });
        // SAFETY: `fire:` exists on the target, retained by the window.
        let b = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str(title),
                Some(&t),
                Some(ActionTarget::selector()),
                self.mtm,
            )
        };
        b.setState(if get(&self.current) {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.rows
            .push(Retained::into_super(Retained::into_super(b)));
    }

    fn popup<T: Clone + PartialEq + 'static>(
        &mut self,
        label: &str,
        options: Vec<(String, T)>,
        get: impl Fn(&Settings) -> T,
        set: impl Fn(&mut Settings, T) + 'static,
    ) {
        let cur = get(&self.current);
        let opts = Rc::new(options);
        let set: Set<T> = Rc::new(set);
        let o2 = opts.clone();
        let t = self.target(move |sender| {
            let Some(p) = sender.downcast_ref::<NSPopUpButton>() else {
                return;
            };
            let i = p.indexOfSelectedItem();
            if let Some((_, v)) = usize::try_from(i).ok().and_then(|i| o2.get(i)) {
                let v = v.clone();
                let set = set.clone();
                apply(move |s| set(s, v));
            }
        });
        let p = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(self.mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(220.0, 26.0)),
            false,
        );
        for (title, _) in opts.iter() {
            p.addItemWithTitle(&NSString::from_str(title));
        }
        if let Some(i) = opts.iter().position(|(_, v)| *v == cur) {
            p.selectItemAtIndex(i as isize);
        } else {
            // Value set elsewhere (file edited by hand): show it, unselected.
            p.addItemWithTitle(&NSString::from_str("Valeur personnalisée"));
            p.selectItemAtIndex(opts.len() as isize);
        }
        // SAFETY: target retained by the window for the control's lifetime.
        unsafe {
            p.setTarget(Some(&t));
            p.setAction(Some(ActionTarget::selector()));
        }
        let l = NSTextField::labelWithString(&NSString::from_str(label), self.mtm);
        let row = hstack(
            self.mtm,
            &[
                Retained::into_super(Retained::into_super(l)),
                Retained::into_super(Retained::into_super(Retained::into_super(p))),
            ],
        );
        self.rows.push(row);
    }

    fn button(&mut self, title: &str, f: impl Fn() + 'static) -> Retained<NSButton> {
        let t = self.target(move |_| f());
        // SAFETY: target retained by the window.
        unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(&t),
                Some(ActionTarget::selector()),
                self.mtm,
            )
        }
    }

    fn push(&mut self, v: Retained<NSView>) {
        self.rows.push(v);
    }

    fn view(self, targets: &mut Vec<Retained<ActionTarget>>) -> Retained<NSView> {
        targets.extend(self.targets);
        let stack =
            NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&self.rows), self.mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(8.0);
        stack.setEdgeInsets(NSEdgeInsets {
            top: 16.0,
            left: 20.0,
            bottom: 16.0,
            right: 20.0,
        });
        Retained::into_super(stack)
    }
}

fn hstack(mtm: MainThreadMarker, views: &[Retained<NSView>]) -> Retained<NSView> {
    let s = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(views), mtm);
    s.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    s.setSpacing(8.0);
    Retained::into_super(s)
}

fn ms_options(values: &[u64]) -> Vec<(String, u64)> {
    values
        .iter()
        .map(|v| {
            let t = if *v == 0 {
                "Immédiat".to_string()
            } else if *v < 1000 {
                format!("{v} ms")
            } else {
                format!(
                    "{} s",
                    crate::format::decimal(*v as f64 / 1000.0, 1).replace(",0", "")
                )
            };
            (t, *v)
        })
        .collect()
}

fn num_options<T: Copy + std::fmt::Display>(values: &[T], unit: &str) -> Vec<(String, T)> {
    values.iter().map(|v| (format!("{v}{unit}"), *v)).collect()
}

fn module_label(id: ModuleId) -> &'static str {
    match id {
        ModuleId::Music => "Musique",
        ModuleId::Calendar => "Calendrier",
        ModuleId::Notifications => "Notifications (Accessibilité)",
        ModuleId::Accessories => "Accessoires",
        ModuleId::Downloads => "Téléchargements",
        ModuleId::Files => "Étagère de fichiers",
        ModuleId::Shortcuts => "Raccourcis",
        ModuleId::Camera => "Caméra",
        ModuleId::Agents => "Agents IA",
        ModuleId::Watch => "Surveillance de zone (Enregistrement d’écran)",
        ModuleId::Timers => "Minuteurs",
        ModuleId::Companion => "Compagnon",
        ModuleId::Metrics => "Mesures système",
        ModuleId::Power => "Maintien éveillé",
        ModuleId::Captures => "Captures récentes (dossier Bureau)",
        ModuleId::Scratchpad => "Bloc-notes",
        ModuleId::Clipboard => "Presse-papiers (mémoire seulement)",
        ModuleId::Mixer => "Mixeur par app (Enregistrement audio du système)",
    }
}

fn general(
    mtm: MainThreadMarker,
    s: &Settings,
    t: &mut Vec<Retained<ActionTarget>>,
) -> Retained<NSView> {
    let mut f = Form::new(mtm, s.clone());
    f.header("Démarrage");
    let status = login::status();
    let login_box = {
        let tgt = f.target(|sender| {
            let on = sender
                .downcast_ref::<NSButton>()
                .is_some_and(|b| b.state() == NSControlStateValueOn);
            match login::set(on) {
                Ok(st) => {
                    apply(|s| s.launch_at_login = st == LoginStatus::Enabled);
                    if st == LoginStatus::RequiresApproval
                        && let Some(a) = app::get()
                    {
                        super::alerts::error(
                            a.mtm,
                            "Autorisation requise",
                            "Activez Nox dans Réglages Système › Général › Ouverture.",
                        );
                    }
                }
                Err(e) => {
                    if let Some(a) = app::get() {
                        super::alerts::error(
                            a.mtm,
                            "Ouverture à la connexion refusée par macOS",
                            &e,
                        );
                    }
                    if let Some(b) = sender.downcast_ref::<NSButton>() {
                        b.setState(if on {
                            NSControlStateValueOff
                        } else {
                            NSControlStateValueOn
                        });
                    }
                }
            }
        });
        // SAFETY: target retained by the window.
        let b = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str("Ouvrir Nox à la connexion"),
                Some(&tgt),
                Some(ActionTarget::selector()),
                mtm,
            )
        };
        b.setState(if status == LoginStatus::Enabled {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        b.setEnabled(status != LoginStatus::Unsupported);
        b
    };
    f.push(Retained::into_super(Retained::into_super(login_box)));
    if status == LoginStatus::RequiresApproval {
        f.note("En attente d’approbation dans Réglages Système › Général › Ouverture.");
    }

    f.header("Îlot");
    f.checkbox(
        "Déplier au survol",
        |s| s.island.expand_on_hover,
        |s, v| s.island.expand_on_hover = v,
    );
    f.popup(
        "Délai de survol",
        ms_options(&[0, 100, 250, 400, 600, 1000]),
        |s| s.island.hover_delay_ms,
        |s, v| s.island.hover_delay_ms = v,
    );
    f.popup(
        "Délai de fermeture",
        ms_options(&[0, 200, 400, 800, 1500]),
        |s| s.island.close_delay_ms,
        |s, v| s.island.close_delay_ms = v,
    );
    f.popup(
        "Taille dépliée",
        vec![
            ("Compacte".into(), IslandSize::Compact),
            ("Spacieuse".into(), IslandSize::Spacious),
            ("Personnalisée".into(), IslandSize::Custom),
        ],
        |s| s.island.size,
        |s, v| s.island.size = v,
    );
    f.popup(
        "Largeur personnalisée",
        num_options(&[400.0, 440.0, 480.0, 520.0, 560.0, 600.0, 640.0], " pt"),
        |s| s.island.width,
        |s, v| s.island.width = v,
    );
    f.popup(
        "Écran",
        vec![
            (
                "Automatique (intégré si présent)".into(),
                DisplayChoice::Automatic,
            ),
            ("Écran intégré".into(), DisplayChoice::BuiltIn),
            ("Écran principal".into(), DisplayChoice::Main),
        ],
        |s| s.island.display,
        |s, v| s.island.display = v,
    );
    f.checkbox(
        "Simuler l’encoche sur les écrans qui n’en ont pas",
        |s| s.island.simulate_notch,
        |s, v| s.island.simulate_notch = v,
    );
    f.popup(
        "Forme sans encoche",
        vec![
            ("Capsule dans la barre".into(), IslandSilhouette::Capsule),
            ("Encoche suspendue".into(), IslandSilhouette::Notch),
        ],
        |s| s.island.silhouette,
        |s, v| s.island.silhouette = v,
    );
    f.popup(
        "Au repos",
        vec![
            ("Rien".into(), IdleContent::Nothing),
            ("Batterie".into(), IdleContent::Battery),
            ("Musique en pause".into(), IdleContent::Music),
            ("Agents IA".into(), IdleContent::Agents),
        ],
        |s| s.island.idle_content,
        |s, v| s.island.idle_content = v,
    );
    f.checkbox(
        "Masquer jusqu’au survol",
        |s| s.island.hide_until_hover,
        |s, v| s.island.hide_until_hover = v,
    );
    f.checkbox(
        "Masquer quand une app est en plein écran",
        |s| s.island.hide_in_fullscreen,
        |s, v| s.island.hide_in_fullscreen = v,
    );
    f.checkbox(
        "Exclure des captures et partages d’écran",
        |s| s.island.hide_in_captures,
        |s, v| s.island.hide_in_captures = v,
    );
    f.checkbox("Contour", |s| s.island.outline, |s, v| s.island.outline = v);
    f.checkbox(
        "Fond translucide (déplié)",
        |s| s.island.translucent,
        |s, v| s.island.translucent = v,
    );
    f.checkbox(
        "Gestes du trackpad (balayer = section)",
        |s| s.island.gestures,
        |s, v| s.island.gestures = v,
    );
    f.checkbox(
        "Retour haptique",
        |s| s.island.haptics,
        |s, v| s.island.haptics = v,
    );
    f.checkbox(
        "Alertes temporaires dans l’îlot",
        |s| s.island.show_alerts,
        |s, v| s.island.show_alerts = v,
    );
    f.checkbox(
        "Raccourci global ⌃⌥N pour ouvrir l’îlot",
        |s| s.island.shortcut_enabled,
        |s, v| s.island.shortcut_enabled = v,
    );
    f.view(t)
}

fn sections(
    mtm: MainThreadMarker,
    s: &Settings,
    t: &mut Vec<Retained<ActionTarget>>,
) -> Retained<NSView> {
    let mut f = Form::new(mtm, s.clone());
    f.header("Sections de l’îlot");
    f.note("Cochez pour afficher, flèches pour réordonner. Désactiver un module l’arrête réellement (aucune collecte).");
    let n = s.island.sections.len();
    for (i, cfg) in s.island.sections.iter().enumerate() {
        let sec = cfg.section;
        let shown = {
            let tg = f.target(move |sender| {
                let on = sender
                    .downcast_ref::<NSButton>()
                    .is_some_and(|b| b.state() == NSControlStateValueOn);
                apply(move |s| {
                    if let Some(c) = s.island.sections.iter_mut().find(|c| c.section == sec) {
                        c.enabled = on;
                    }
                });
            });
            // SAFETY: target retained by the window.
            let b = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::from_str(sec.label_fr()),
                    Some(&tg),
                    Some(ActionTarget::selector()),
                    mtm,
                )
            };
            b.setState(if cfg.enabled {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            b
        };
        let module = sec.module();
        let module_box = {
            let tg = f.target(move |sender| {
                let on = sender
                    .downcast_ref::<NSButton>()
                    .is_some_and(|b| b.state() == NSControlStateValueOn);
                apply(move |s| s.set_module_enabled(module, on));
            });
            // SAFETY: target retained by the window.
            let b = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::from_str("module actif"),
                    Some(&tg),
                    Some(ActionTarget::selector()),
                    mtm,
                )
            };
            b.setState(if s.module_enabled(module) {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            b
        };
        let up = f.button("▲", move || {
            apply(move |s| {
                if i > 0 {
                    s.island.sections.swap(i, i - 1);
                }
            });
            reload();
        });
        up.setEnabled(i > 0);
        let down = f.button("▼", move || {
            apply(move |s| {
                if i + 1 < s.island.sections.len() {
                    s.island.sections.swap(i, i + 1);
                }
            });
            reload();
        });
        down.setEnabled(i + 1 < n);
        shown.setFrameSize(NSSize::new(170.0, 20.0));
        let row = hstack(
            mtm,
            &[
                Retained::into_super(Retained::into_super(up)),
                Retained::into_super(Retained::into_super(down)),
                Retained::into_super(Retained::into_super(shown)),
                Retained::into_super(Retained::into_super(module_box)),
            ],
        );
        f.push(row);
    }
    f.note("Mixeur : l’activer demande à macOS l’autorisation d’enregistrement audio du système. Le son n’est ni enregistré ni envoyé, et une app laissée à 100 % n’est jamais interceptée.");
    f.header("Modules sans section");
    f.checkbox(
        module_label(ModuleId::Companion),
        |s| s.module_enabled(ModuleId::Companion),
        |s, v| s.set_module_enabled(ModuleId::Companion, v),
    );
    f.checkbox(
        "Compagnon dans l’îlot fermé",
        |s| s.companion.enabled_in_compact,
        |s, v| s.companion.enabled_in_compact = v,
    );
    f.popup(
        "Personnage",
        vec![
            ("Goutte".into(), "blob".to_string()),
            ("Chat".into(), "cat".to_string()),
            ("Fantôme".into(), "ghost".to_string()),
        ],
        |s| s.companion.character.clone(),
        |s, v| s.companion.character = v,
    );
    f.popup(
        "Couleur",
        [
            ("Nacre", "pearl"),
            ("Citron", "lemon"),
            ("Pêche", "peach"),
            ("Rose", "rose"),
            ("Lilas", "lilac"),
            ("Ciel", "sky"),
            ("Menthe", "mint"),
        ]
        .into_iter()
        .map(|(l, v)| (l.to_string(), v.to_string()))
        .collect(),
        |s| s.companion.palette.clone(),
        |s, v| s.companion.palette = v,
    );
    f.popup(
        "Côté",
        vec![
            ("Gauche de l’encoche".into(), CompanionSide::Left),
            ("Droite de l’encoche".into(), CompanionSide::Right),
        ],
        |s| s.companion.side,
        |s, v| s.companion.side = v,
    );
    f.checkbox(
        "Réactions aux événements",
        |s| s.companion.reactions,
        |s, v| s.companion.reactions = v,
    );
    f.view(t)
}

fn content(
    mtm: MainThreadMarker,
    s: &Settings,
    t: &mut Vec<Retained<ActionTarget>>,
) -> Retained<NSView> {
    let mut f = Form::new(mtm, s.clone());
    f.header("Musique");
    f.checkbox("Paroles", |s| s.music.lyrics, |s, v| s.music.lyrics = v);
    f.checkbox(
        "File d’attente",
        |s| s.music.queue,
        |s, v| s.music.queue = v,
    );
    f.checkbox(
        "Visualiseur",
        |s| s.music.visualizer,
        |s, v| s.music.visualizer = v,
    );
    f.checkbox(
        "Chercher les paroles en ligne (envoie titre et artiste à lrclib.net)",
        |s| s.music.lyrics_online,
        |s, v| s.music.lyrics_online = v,
    );
    f.checkbox(
        "Rechercher les pochettes manquantes en ligne",
        |s| s.music.browser_artwork,
        |s, v| s.music.browser_artwork = v,
    );
    f.note("Envoie titre et artiste à Apple (iTunes) ; lit la session Firefox locale pour retrouver la miniature YouTube.");
    f.checkbox(
        "Niveaux audio réels pour le visualiseur",
        |s| s.music.live_levels,
        |s, v| s.music.live_levels = v,
    );
    f.note("Demande l’autorisation d’enregistrement audio du système ; sinon l’égaliseur est animé et marqué « non mesuré ».");
    let purge = f.button("Purger les caches (paroles et pochettes)", || {
        if let Some(a) = app::get() {
            a.hub.send(Command::Music(
                nox_core::model::music::MusicCommand::ClearLyricsCache,
            ));
        }
    });
    f.push(Retained::into_super(Retained::into_super(purge)));
    f.checkbox(
        "Inclure les autres lecteurs (navigateurs…)",
        |s| s.music.include_other_players,
        |s, v| s.music.include_other_players = v,
    );
    f.checkbox(
        "Alerte au changement de morceau",
        |s| s.music.track_change_alert,
        |s, v| s.music.track_change_alert = v,
    );
    f.header("Calendrier");
    f.popup(
        "Jours affichés",
        num_options(&[1u32, 3, 7, 14, 30], " j"),
        |s| s.calendar.days_ahead,
        |s, v| s.calendar.days_ahead = v,
    );
    f.popup(
        "Alerte avant un événement",
        num_options(&[0u32, 1, 5, 10, 15, 30], " min"),
        |s| s.calendar.alert_minutes_before,
        |s, v| s.calendar.alert_minutes_before = v,
    );
    f.checkbox(
        "Afficher les rappels datés",
        |s| s.calendar.show_reminders,
        |s, v| s.calendar.show_reminders = v,
    );
    // One switch per calendar known to the module (hidden by identifier).
    let calendars = app::get()
        .and_then(|a| a.state().calendar.map(|c| c.calendars))
        .unwrap_or_default();
    for cal in calendars {
        let id = cal.id.clone();
        let id2 = cal.id.clone();
        f.checkbox(
            &format!("Afficher « {} » ({})", cal.title, cal.source),
            move |s| !s.calendar.hidden_calendars.contains(&id),
            move |s, on| {
                s.calendar.hidden_calendars.retain(|h| *h != id2);
                if !on {
                    s.calendar.hidden_calendars.push(id2.clone());
                }
            },
        );
    }
    f.header("Notifications");
    f.popup(
        "Nombre maximal",
        num_options(&[5usize, 10, 20, 50], ""),
        |s| s.notifications.max_items,
        |s, v| s.notifications.max_items = v,
    );
    f.checkbox(
        "Masquer le contenu des messages",
        |s| s.notifications.hide_body,
        |s, v| s.notifications.hide_body = v,
    );
    f.view(t)
}

fn activities(
    mtm: MainThreadMarker,
    s: &Settings,
    t: &mut Vec<Retained<ActionTarget>>,
) -> Retained<NSView> {
    let mut f = Form::new(mtm, s.clone());
    f.header("Minuteurs");
    f.popup(
        "Concentration",
        num_options(&[15u32, 20, 25, 30, 45, 50, 60], " min"),
        |s| s.timers.focus_work_min,
        |s, v| s.timers.focus_work_min = v,
    );
    f.popup(
        "Pause courte",
        num_options(&[3u32, 5, 10], " min"),
        |s| s.timers.focus_short_break_min,
        |s, v| s.timers.focus_short_break_min = v,
    );
    f.popup(
        "Pause longue",
        num_options(&[10u32, 15, 20, 30], " min"),
        |s| s.timers.focus_long_break_min,
        |s, v| s.timers.focus_long_break_min = v,
    );
    f.popup(
        "Sessions",
        num_options(&[2u32, 3, 4, 6, 8], ""),
        |s| s.timers.focus_rounds,
        |s, v| s.timers.focus_rounds = v,
    );
    f.popup(
        "Pause longue toutes les",
        num_options(&[2u32, 3, 4, 6], " sessions"),
        |s| s.timers.focus_long_break_every,
        |s, v| s.timers.focus_long_break_every = v,
    );
    f.checkbox(
        "Enchaîner les phases automatiquement",
        |s| s.timers.focus_auto_advance,
        |s, v| s.timers.focus_auto_advance = v,
    );
    f.checkbox(
        "Son à la fin",
        |s| s.timers.sound,
        |s, v| s.timers.sound = v,
    );
    f.header("Captures et presse-papiers");
    f.popup(
        "Captures affichées",
        num_options(&[6usize, 12, 24], ""),
        |s| s.captures.max_items,
        |s, v| s.captures.max_items = v,
    );
    f.popup(
        "Historique du presse-papiers",
        num_options(&[10usize, 20, 50], " éléments"),
        |s| s.clipboard.max_items,
        |s, v| s.clipboard.max_items = v,
    );
    f.checkbox(
        "Garder aussi les images copiées",
        |s| s.clipboard.capture_images,
        |s, v| s.clipboard.capture_images = v,
    );
    f.note("L’historique reste en mémoire, n’est jamais écrit sur disque, et ignore les gestionnaires de mots de passe.");
    f.header("Accessoires");
    f.checkbox(
        "Alerte à la connexion",
        |s| s.accessories.alert_on_connect,
        |s, v| s.accessories.alert_on_connect = v,
    );
    f.popup(
        "Alerte batterie faible",
        num_options(&[0u8, 10, 15, 20, 30], " %"),
        |s| s.accessories.low_battery_percent,
        |s, v| s.accessories.low_battery_percent = v,
    );
    f.view(t)
}

fn agents(
    mtm: MainThreadMarker,
    s: &Settings,
    t: &mut Vec<Retained<ActionTarget>>,
) -> Retained<NSView> {
    let mut f = Form::new(mtm, s.clone());
    f.header("Agents IA suivis");
    for p in AgentProvider::ALL {
        f.checkbox(
            p.label(),
            move |s| s.agents.enabled.contains(&p),
            move |s, v| {
                s.agents.enabled.retain(|x| *x != p);
                if v {
                    s.agents.enabled.push(p);
                }
            },
        );
    }
    f.popup(
        "Alerte de fin après",
        num_options(&[10u64, 30, 60, 120, 300], " s de travail"),
        |s| s.agents.finish_alert_min_secs,
        |s, v| s.agents.finish_alert_min_secs = v,
    );
    f.checkbox(
        "Alerte quand une tâche se termine",
        |s| s.agents.finish_alert,
        |s, v| s.agents.finish_alert = v,
    );
    f.checkbox(
        "Alerte quand un agent attend une réponse",
        |s| s.agents.needs_input_alert,
        |s, v| s.agents.needs_input_alert = v,
    );
    f.checkbox(
        "Alerte à l’approche d’une limite",
        |s| s.agents.limit_alert,
        |s, v| s.agents.limit_alert = v,
    );
    f.popup(
        "Seuil d’alerte de limite",
        num_options(&[50.0, 75.0, 90.0, 95.0], " %"),
        |s| s.agents.limit_threshold_percent,
        |s, v| s.agents.limit_threshold_percent = v,
    );
    f.popup(
        "Budget quotidien (estimé)",
        vec![
            ("Aucun".into(), 0.0),
            ("5 $".into(), 5.0),
            ("10 $".into(), 10.0),
            ("25 $".into(), 25.0),
            ("50 $".into(), 50.0),
            ("100 $".into(), 100.0),
        ],
        |s| s.agents.daily_budget_usd,
        |s, v| s.agents.daily_budget_usd = v,
    );
    f.popup(
        "Période des totaux",
        vec![
            (AgentPeriod::Today.label_fr().into(), AgentPeriod::Today),
            (AgentPeriod::Week.label_fr().into(), AgentPeriod::Week),
            (AgentPeriod::Month.label_fr().into(), AgentPeriod::Month),
        ],
        |s| s.agents.period,
        |s, v| s.agents.period = v,
    );
    f.header("Limites des forfaits");
    f.checkbox(
        "Lire les limites (5 h, semaine) auprès des fournisseurs",
        |s| s.agents.allow_limit_queries,
        |s, v| s.agents.allow_limit_queries = v,
    );
    f.note("Désactivé par défaut. Si vous l’activez, Nox utilise les jetons déjà enregistrés par les CLI (Claude Code, Codex…) pour interroger leurs limites. Ils ne sont ni copiés ni stockés par Nox. Les coûts affichés sont toujours des estimations au tarif API.");
    f.header("Surveillance de zone");
    f.popup(
        "Fréquence de comparaison",
        ms_options(&[500, 1000, 2000, 5000]),
        |s| s.watch.interval_ms,
        |s, v| s.watch.interval_ms = v,
    );
    f.popup(
        "« S’arrête de changer » après",
        num_options(&[3u64, 5, 8, 15, 30], " s"),
        |s| s.watch.settle_secs,
        |s, v| s.watch.settle_secs = v,
    );
    f.popup(
        "Sensibilité",
        vec![
            ("Élevée (0,5 %)".into(), 0.005),
            ("Normale (1 %)".into(), 0.01),
            ("Faible (5 %)".into(), 0.05),
        ],
        |s| s.watch.sensitivity,
        |s, v| s.watch.sensitivity = v,
    );
    f.checkbox(
        "Son lors d’une alerte",
        |s| s.watch.sound,
        |s, v| s.watch.sound = v,
    );
    f.view(t)
}

fn system(
    mtm: MainThreadMarker,
    s: &Settings,
    t: &mut Vec<Retained<ActionTarget>>,
) -> Retained<NSView> {
    let mut f = Form::new(mtm, s.clone());
    f.header("Mesures");
    f.popup(
        "Fréquence (affichées)",
        ms_options(&[1000, 2000, 5000]),
        |s| s.metrics.interval_ms,
        |s, v| s.metrics.interval_ms = v,
    );
    f.popup(
        "Fréquence (arrière-plan)",
        ms_options(&[5000, 10_000, 30_000, 60_000]),
        |s| s.metrics.background_interval_ms,
        |s, v| s.metrics.background_interval_ms = v,
    );
    f.popup(
        "Historique",
        num_options(&[60usize, 120, 300, 600], " points"),
        |s| s.metrics.history_len,
        |s, v| s.metrics.history_len = v,
    );
    f.popup(
        "Alerte batterie faible",
        num_options(&[0u8, 10, 15, 20, 30], " %"),
        |s| s.metrics.low_battery_alert,
        |s, v| s.metrics.low_battery_alert = v,
    );
    f.header("Maintien éveillé");
    f.popup(
        "Mode par défaut",
        vec![
            (
                KeepAwakeMode::System.label_fr().into(),
                KeepAwakeMode::System,
            ),
            (
                KeepAwakeMode::SystemAndDisplay.label_fr().into(),
                KeepAwakeMode::SystemAndDisplay,
            ),
            (
                KeepAwakeMode::ClosedLid.label_fr().into(),
                KeepAwakeMode::ClosedLid,
            ),
        ],
        |s| s.power.default_mode,
        |s, v| s.power.default_mode = v,
    );
    f.popup(
        "Durée par défaut",
        KeepAwakeDuration::PRESETS
            .iter()
            .map(|d| (d.label_fr(), *d))
            .collect(),
        |s| s.power.default_duration,
        |s, v| s.power.default_duration = v,
    );
    f.checkbox(
        "Seulement sur secteur",
        |s| s.power.only_on_ac,
        |s, v| s.power.only_on_ac = v,
    );
    f.popup(
        "Arrêter sous",
        num_options(&[0u8, 10, 15, 20, 30, 50], " % de batterie"),
        |s| s.power.stop_below_percent,
        |s, v| s.power.stop_below_percent = v,
    );
    f.note("Le mode capot fermé demande une autorisation sudo limitée à « pmset disablesleep », installée une fois avec votre mot de passe administrateur. Nox ne voit ni ne stocke ce mot de passe.");
    let install = f.button("Installer l’autorisation capot fermé…", || {
        if let Some(a) = app::get() {
            super::alerts::install_helper(&a);
        }
    });
    let uninstall = f.button("Retirer l’autorisation capot fermé", || {
        if let Some(a) = app::get() {
            a.hub.send(Command::Power(PowerCommand::UninstallHelper));
        }
    });
    let row = hstack(
        mtm,
        &[
            Retained::into_super(Retained::into_super(install)),
            Retained::into_super(Retained::into_super(uninstall)),
        ],
    );
    f.push(row);
    f.view(t)
}

type PageBuilder =
    fn(MainThreadMarker, &Settings, &mut Vec<Retained<ActionTarget>>) -> Retained<NSView>;

fn build(
    mtm: MainThreadMarker,
    s: &Settings,
) -> (Retained<NSTabView>, Vec<Retained<ActionTarget>>) {
    let tabs = NSTabView::new(mtm);
    let mut targets = vec![];
    let pages: [(&str, PageBuilder); 6] = [
        ("Général", general),
        ("Sections", sections),
        ("Contenus", content),
        ("Activités", activities),
        ("Agents & surveillance", agents),
        ("Système & énergie", system),
    ];
    for (title, f) in pages {
        let item = NSTabViewItem::new();
        item.setLabel(&NSString::from_str(title));
        item.setView(Some(&f(mtm, s, &mut targets)));
        tabs.addTabViewItem(&item);
    }
    (tabs, targets)
}

impl SettingsWindow {
    /// Debug: every tab's controls with their frames in window coordinates,
    /// and whether anything falls outside the visible content area.
    #[cfg(debug_assertions)]
    pub fn layout_report(&self) -> String {
        use objc2_app_kit::{NSButton, NSPopUpButton, NSTextField};
        let mut out = String::new();
        let content = self.window.contentView().map(|v| v.frame());
        out.push_str(&format!("content: {content:?}\n"));
        let n = self.tabs.numberOfTabViewItems();
        for i in 0..n {
            self.tabs.selectTabViewItemAtIndex(i);
            if let Some(v) = self.window.contentView() {
                v.layoutSubtreeIfNeeded();
            }
            let item = self.tabs.tabViewItemAtIndex(i);
            out.push_str(&format!("== tab {}\n", item.label()));
            let Some(root) = item.view(MainThreadMarker::from(&*self.window)) else {
                continue;
            };
            let visible = self.tabs.contentRect();
            let mut overflow = 0;
            fn walk(
                v: &NSView,
                root: &NSView,
                depth: usize,
                out: &mut String,
                visible: NSRect,
                overflow: &mut usize,
            ) {
                for sub in v.subviews().iter() {
                    let f = sub.convertRect_toView(sub.bounds(), Some(root));
                    let title = if let Some(b) = sub.downcast_ref::<NSPopUpButton>() {
                        format!(
                            "popup «{}»",
                            b.titleOfSelectedItem()
                                .map(|t| t.to_string())
                                .unwrap_or_default()
                        )
                    } else if let Some(b) = sub.downcast_ref::<NSButton>() {
                        format!("button «{}»", b.title())
                    } else if let Some(t) = sub.downcast_ref::<NSTextField>() {
                        format!("label «{}»", t.stringValue())
                    } else {
                        String::new()
                    };
                    if !title.is_empty() {
                        let off = f.origin.y < -0.5
                            || f.origin.y + f.size.height > visible.size.height + 0.5;
                        if off {
                            *overflow += 1;
                        }
                        out.push_str(&format!(
                            "{}{title} y={:.0} h={:.0} x={:.0} w={:.0}{}\n",
                            "  ".repeat(depth),
                            f.origin.y,
                            f.size.height,
                            f.origin.x,
                            f.size.width,
                            if off { "  <-- HORS CHAMP" } else { "" }
                        ));
                    } else {
                        walk(&sub, root, depth + 1, out, visible, overflow);
                    }
                }
            }
            walk(&root, &root, 0, &mut out, visible, &mut overflow);
            out.push_str(&format!(
                "visible height {:.0}, hors champ: {overflow}\n",
                visible.size.height
            ));
        }
        self.tabs.selectTabViewItemAtIndex(0);
        out
    }
}

/// Rebuilds the window content (after reordering), keeping the selected tab.
fn reload() {
    nox_core::main_thread::run_async(|| {
        let Some(a) = app::get() else { return };
        let settings = a.settings();
        let mut w = a.settings_window.borrow_mut();
        let Some(sw) = w.as_mut() else { return };
        let idx = sw
            .tabs
            .selectedTabViewItem()
            .map(|i| sw.tabs.indexOfTabViewItem(&i))
            .unwrap_or(0);
        let (tabs, targets) = build(a.mtm, &settings);
        tabs.selectTabViewItemAtIndex(idx);
        sw.window.setContentView(Some(&tabs));
        sw.tabs = tabs;
        sw.targets = targets;
    });
}

pub fn show(a: &App) {
    let mtm = a.mtm;
    let settings = a.settings();
    let mut w = a.settings_window.borrow_mut();
    if w.is_none() {
        // SAFETY: designated initialiser.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 760.0)),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: owned by SettingsWindow.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Réglages de Nox"));
        let (tabs, targets) = build(mtm, &settings);
        window.setContentView(Some(&tabs));
        window.center();
        *w = Some(SettingsWindow {
            window,
            tabs,
            targets,
        });
    } else {
        drop(w);
        reload();
        w = a.settings_window.borrow_mut();
    }
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    if let Some(sw) = w.as_ref() {
        sw.window.makeKeyAndOrderFront(None);
    }
    let _ = Section::Music;
}
