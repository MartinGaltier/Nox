//! Menu bar item (A12): open the island, keep-awake modes × durations with
//! remaining time, system monitor, settings, quit. The menu is rebuilt each
//! time it opens, so the remaining time is always current.

use super::app::{self, App};
use super::util::ActionTarget;
use crate::format;
use nox_core::model::power::{KeepAwakeDuration, KeepAwakeMode, PowerCommand};
use nox_core::{Command, IslandState};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::NSString;
use std::cell::RefCell;

pub struct DelegateIvars {
    targets: RefCell<Vec<Retained<ActionTarget>>>,
}

define_class!(
    // SAFETY: NSObject subclass without Drop; main-thread only ivars.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NoxMenuDelegate"]
    #[ivars = DelegateIvars]
    pub struct MenuDelegate;

    unsafe impl NSObjectProtocol for MenuDelegate {}

    unsafe impl NSMenuDelegate for MenuDelegate {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            if let Some(a) = app::get() {
                let mut t = self.ivars().targets.borrow_mut();
                t.clear();
                build(&a, menu, &mut t);
            }
        }
    }
);

pub struct StatusBar {
    mtm: MainThreadMarker,
    item: Retained<NSStatusItem>,
    _menu: Retained<NSMenu>,
    _delegate: Retained<MenuDelegate>,
}

fn symbol(name: &str) -> Option<Retained<NSImage>> {
    let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str("Nox")),
    )?;
    img.setTemplate(true);
    Some(img)
}

impl StatusBar {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        let delegate = MenuDelegate::alloc(mtm).set_ivars(DelegateIvars {
            targets: RefCell::new(vec![]),
        });
        // SAFETY: plain NSObject init.
        let delegate: Retained<MenuDelegate> = unsafe { msg_send![super(delegate), init] };
        menu.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        menu.setAutoenablesItems(false);
        item.setMenu(Some(&menu));
        let s = Self {
            mtm,
            item,
            _menu: menu,
            _delegate: delegate,
        };
        s.refresh_icon(&IslandState::default());
        s
    }

    /// Icon reflects keep-awake; the title shows remaining minutes.
    pub fn refresh_icon(&self, st: &IslandState) {
        let Some(button) = self.item.button(self.mtm) else {
            return;
        };
        let active = st.power.as_ref().and_then(|p| p.active.as_ref());
        let name = if active.is_some() {
            "cup.and.saucer.fill"
        } else {
            "moon.stars"
        };
        button.setImage(symbol(name).as_deref());
        let title = match active.and_then(|a| a.ends_at) {
            Some(e) => format!(
                " {}",
                format::duration((e - nox_core::model::now()).max(0.0))
            ),
            None if active.is_some() => " ∞".into(),
            None => String::new(),
        };
        button.setTitle(&NSString::from_str(&title));
    }
}

fn add(
    menu: &NSMenu,
    targets: &mut Vec<Retained<ActionTarget>>,
    title: &str,
    key: &str,
    enabled: bool,
    f: impl Fn(&App) + 'static,
) -> Retained<NSMenuItem> {
    let mtm = MainThreadMarker::from(menu);
    let t = ActionTarget::new(mtm, move |_s: &AnyObject| {
        if let Some(a) = app::get() {
            f(&a);
        }
    });
    // SAFETY: `fire:` exists on the target, which is retained in `targets`
    // for as long as the menu items reference it (until the next rebuild).
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(ActionTarget::selector()),
            &NSString::from_str(key),
        )
    };
    // SAFETY: the target outlives the item (see above).
    unsafe { item.setTarget(Some(&t)) };
    item.setEnabled(enabled);
    targets.push(t);
    menu.addItem(&item);
    item
}

fn info(menu: &NSMenu, title: &str) {
    let mtm = MainThreadMarker::from(menu);
    // SAFETY: no action; a disabled informative row.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            None,
            &NSString::from_str(""),
        )
    };
    item.setEnabled(false);
    menu.addItem(&item);
}

fn build(a: &App, menu: &NSMenu, targets: &mut Vec<Retained<ActionTarget>>) {
    let mtm = a.mtm;
    menu.removeAllItems();
    add(menu, targets, "Ouvrir l’îlot", "", true, |a| {
        a.toggle_island()
    });
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let st = a.state();
    match st.power.as_ref() {
        None => info(menu, "Maintien éveillé : module désactivé"),
        Some(p) => {
            match &p.active {
                Some(s) => {
                    let left = match s.ends_at {
                        Some(e) => format!(
                            "encore {}",
                            format::duration((e - nox_core::model::now()).max(0.0))
                        ),
                        None => "durée illimitée".into(),
                    };
                    info(menu, &format!("Éveillé : {} — {left}", s.mode.label_fr()));
                    add(
                        menu,
                        targets,
                        "Arrêter le maintien éveillé",
                        "",
                        true,
                        |a| a.hub.send(Command::Power(PowerCommand::Stop)),
                    );
                }
                None => {
                    info(menu, "Maintien éveillé : inactif");
                    let st = a.settings();
                    let title = format!(
                        "Activer ({}, {})",
                        st.power.default_mode.label_fr(),
                        st.power.default_duration.label_fr()
                    );
                    add(menu, targets, &title, "", true, |a| {
                        a.hub.send(Command::Power(PowerCommand::Toggle))
                    });
                }
            }
            for mode in [
                KeepAwakeMode::System,
                KeepAwakeMode::SystemAndDisplay,
                KeepAwakeMode::ClosedLid,
            ] {
                let sub = NSMenu::new(mtm);
                if mode.needs_helper() && !p.helper.usable() {
                    info(&sub, p.helper.label_fr());
                    let label = if p.helper_busy {
                        "Installation en cours…"
                    } else {
                        crate::views::system::helper_button(p.helper)
                    };
                    add(&sub, targets, label, "", !p.helper_busy, |a| {
                        super::alerts::install_helper(a)
                    });
                } else {
                    for d in KeepAwakeDuration::PRESETS {
                        add(&sub, targets, &d.label_fr(), "", true, move |a| {
                            a.hub
                                .send(Command::Power(PowerCommand::Start { mode, duration: d }))
                        });
                    }
                }
                // SAFETY: a submenu-holding item without action.
                let parent = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(mode.label_fr()),
                        None,
                        &NSString::from_str(""),
                    )
                };
                parent.setSubmenu(Some(&sub));
                menu.addItem(&parent);
            }
        }
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add(menu, targets, "Moniteur système…", "", true, |a| {
        super::monitor_window::show(a)
    });
    add(menu, targets, "Réglages…", ",", true, |a| {
        super::settings_window::show(a)
    });
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add(menu, targets, "Quitter Nox", "q", true, |a| a.quit());
}
