//! Modal explanations and errors (NSAlert).

use super::app::App;
use nox_core::Command;
use nox_core::model::power::PowerCommand;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication};
use objc2_foundation::NSString;

fn activate(mtm: MainThreadMarker) {
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

pub fn error(mtm: MainThreadMarker, title: &str, detail: &str) {
    let a = NSAlert::new(mtm);
    a.setAlertStyle(NSAlertStyle::Warning);
    a.setMessageText(&NSString::from_str(title));
    a.setInformativeText(&NSString::from_str(detail));
    activate(mtm);
    a.runModal();
}

/// Destructive confirmation; true if the user confirmed.
pub fn confirm(mtm: MainThreadMarker, title: &str, detail: &str) -> bool {
    let a = NSAlert::new(mtm);
    a.setAlertStyle(NSAlertStyle::Warning);
    a.setMessageText(&NSString::from_str(title));
    a.setInformativeText(&NSString::from_str(detail));
    a.addButtonWithTitle(&NSString::from_str("Confirmer"));
    a.addButtonWithTitle(&NSString::from_str("Annuler"));
    activate(mtm);
    a.runModal() == NSAlertFirstButtonReturn
}

/// Explains what the closed-lid permission does before asking macOS for the
/// administrator password (once). Nothing happens without explicit consent.
pub fn install_helper(app: &App) {
    let mtm = app.mtm;
    let a = NSAlert::new(mtm);
    a.setMessageText(&NSString::from_str(
        "Autoriser le mode « Mac actif capot fermé » ?",
    ));
    a.setInformativeText(&NSString::from_str(
        "Pour garder le Mac éveillé capot fermé sans écran externe, macOS exige le réglage \
         « veille capot désactivée », que seul un administrateur peut changer.\n\n\
         Nox installe pour cela une règle sudo limitée à deux commandes : \
         /usr/bin/pmset -a disablesleep 1 et … 0, pour votre compte uniquement. Nox ne peut \
         rien faire d’autre avec sudo. Pendant une session, Nox l’active ; à la fin, à l’arrêt \
         ou après un plantage, la veille est rétablie, y compris après un redémarrage (un agent \
         de session léger s’en charge).\n\n\
         macOS va vous demander votre mot de passe une seule fois. Nox ne le voit ni ne le \
         stocke. Si un ancien assistant root de Nox est présent, il sera retiré dans le même \
         passage. Vous pourrez retirer l’autorisation depuis les Réglages.",
    ));
    a.addButtonWithTitle(&NSString::from_str("Autoriser…"));
    a.addButtonWithTitle(&NSString::from_str("Annuler"));
    activate(mtm);
    if a.runModal() == NSAlertFirstButtonReturn {
        app.hub.send(Command::Power(PowerCommand::InstallHelper));
    }
}
