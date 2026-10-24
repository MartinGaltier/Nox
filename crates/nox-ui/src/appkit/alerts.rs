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

/// Explains what the closed-lid helper does before asking macOS for the
/// administrator password (once). Nothing happens without explicit consent.
pub fn install_helper(app: &App) {
    let mtm = app.mtm;
    let a = NSAlert::new(mtm);
    a.setMessageText(&NSString::from_str(
        "Installer l’assistant « capot fermé » ?",
    ));
    a.setInformativeText(&NSString::from_str(
        "Pour garder le Mac éveillé capot fermé sans écran externe, macOS exige un réglage \
         système (veille capot désactivée) que seul un administrateur peut changer.\n\n\
         Nox installe pour cela un petit assistant (launchd) qui ne sait faire que deux choses : \
         désactiver cette veille pendant une session demandée par vous, puis la restaurer — \
         y compris après un plantage ou un redémarrage.\n\n\
         macOS va vous demander votre mot de passe une seule fois. Il n’est jamais vu ni \
         stocké par Nox, et aucun fichier sudoers n’est modifié. Vous pourrez désinstaller \
         l’assistant depuis les Réglages.",
    ));
    a.addButtonWithTitle(&NSString::from_str("Installer…"));
    a.addButtonWithTitle(&NSString::from_str("Annuler"));
    activate(mtm);
    if a.runModal() == NSAlertFirstButtonReturn {
        app.hub.send(Command::Power(PowerCommand::InstallHelper));
    }
}
