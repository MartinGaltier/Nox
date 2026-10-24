//! Quick actions (E6): lock, screenshot (Screenshot.app), dark mode, screen
//! saver, empty Trash (Finder, Automation), open app/URL, run a Shortcut.
//! Every external process is one-shot, on demand, bounded and killed on stop.

use crate::sys::{applescript, system, workspace};
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::shortcuts::{QuickAction, ShortcutCommand, ShortcutsState};
use nox_core::{CancelToken, Hub, Module, ModuleContext, ModuleError, ModuleId, Worker};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SHORTCUTS: &str = "/usr/bin/shortcuts";
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct ShortcutsModule {
    worker: Option<Worker>,
    tx: Option<Sender<ShortcutCommand>>,
}

impl Module for ShortcutsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Shortcuts
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let (tx, rx) = unbounded();
        let hub = ctx.hub.clone();
        hub.update(|s| s.shortcuts = Some(ShortcutsState::default()));
        self.worker = Some(Worker::spawn("shortcuts", move |token| run(hub, rx, token)));
        self.tx = Some(tx);
        Ok(())
    }

    fn stop(&mut self) {
        self.tx.take();
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }

    fn command(&mut self, cmd: nox_core::Command) {
        if let (nox_core::Command::Shortcuts(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(c);
        }
    }
}

/// Runs a one-shot process; kills it on cancel or timeout. Returns stdout.
pub fn run_bounded(
    mut cmd: Command,
    timeout: Duration,
    token: &CancelToken,
) -> Result<String, String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().expect("piped");
    // Drain stdout concurrently so a chatty child never blocks on a full pipe.
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.by_ref().take(1 << 20).read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Ok(st),
            Ok(None) if token.is_cancelled() || start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(if token.is_cancelled() {
                    "interrompu".to_owned()
                } else {
                    "délai dépassé".to_owned()
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => break Err(e.to_string()),
        }
    };
    let out = reader.join().unwrap_or_default();
    match status {
        Ok(st) if st.success() => Ok(out),
        Ok(st) => Err(format!("échec ({st})")),
        Err(e) => Err(e),
    }
}

pub fn parse_list(out: &str) -> Vec<String> {
    let mut v: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .take(500)
        .collect();
    v.sort_by_key(|s| s.to_lowercase());
    v.dedup();
    v
}

/// Accepts only URLs with a scheme (no file paths smuggled as URLs).
pub fn valid_url(u: &str) -> bool {
    let Some((scheme, rest)) = u.split_once(':') else {
        return false;
    };
    !rest.is_empty()
        && !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !scheme.eq_ignore_ascii_case("file")
}

fn perform(action: &QuickAction, token: &CancelToken) -> Result<String, String> {
    match action {
        QuickAction::LockScreen => system::lock_screen()
            .then(|| "Session verrouillée".to_owned())
            .ok_or_else(|| "verrouillage indisponible".to_owned()),
        QuickAction::Screenshot => {
            // Screenshot.app keeps the user's save location and holds the
            // Screen Recording permission itself (Nox needs none).
            if workspace::open_path("/System/Applications/Utilities/Screenshot.app") {
                Ok("Capture d’écran".into())
            } else {
                Err("Capture d’écran indisponible".into())
            }
        }
        QuickAction::ToggleDarkMode => {
            let now = system::dark_mode().ok_or("mode sombre indisponible")?;
            if system::set_dark_mode(!now) {
                Ok(if now { "Mode clair" } else { "Mode sombre" }.into())
            } else {
                Err("bascule refusée".into())
            }
        }
        QuickAction::ToggleDoNotDisturb => Err(
            "Concentration : aucune API publique ; utilisez un raccourci « Activer la concentration »"
                .into(),
        ),
        QuickAction::OpenApp(name) => workspace::open_app(name)
            .then(|| format!("{name} ouvert"))
            .ok_or_else(|| format!("{name} introuvable")),
        QuickAction::RunShortcut(name) => {
            let mut c = Command::new(SHORTCUTS);
            c.arg("run").arg(name);
            run_bounded(c, RUN_TIMEOUT, token).map(|_| format!("« {name} » exécuté"))
        }
        QuickAction::EmptyTrash => {
            // The UI confirms first (permanent). Finder Automation prompt on first use.
            applescript::run(
                "with timeout of 30 seconds\ntell application id \"com.apple.finder\" to empty trash\nend timeout",
            )
            .map(|_| "Corbeille vidée".to_owned())
            .map_err(|e| match e {
                applescript::ScriptError::NotAuthorized => {
                    "Autorisation requise : Automatisation (Finder)".to_owned()
                }
                e => e.to_string(),
            })
        }
        QuickAction::StartScreenSaver => {
            workspace::open_path("/System/Library/CoreServices/ScreenSaverEngine.app")
                .then(|| "Économiseur".to_owned())
                .ok_or_else(|| "économiseur indisponible".to_owned())
        }
        QuickAction::OpenUrl(u) if valid_url(u) => workspace::open_url(u)
            .then(|| "Lien ouvert".to_owned())
            .ok_or_else(|| "lien non pris en charge".to_owned()),
        QuickAction::OpenUrl(_) => Err("URL invalide".into()),
    }
}

fn publish(hub: &Hub, state: &ShortcutsState) {
    let s = state.clone();
    hub.update(move |st| {
        if st.shortcuts.is_some() {
            st.shortcuts = Some(s);
        }
    });
}

fn run(hub: Hub, rx: Receiver<ShortcutCommand>, token: CancelToken) {
    let mut state = ShortcutsState::default();
    publish(&hub, &state);
    while !token.is_cancelled() {
        select! {
            recv(rx) -> c => match c {
                Ok(ShortcutCommand::Run(a)) => {
                    state.last_result = Some(perform(&a, &token));
                    publish(&hub, &state);
                }
                Ok(ShortcutCommand::RefreshUserShortcuts) => {
                    let mut c = Command::new(SHORTCUTS);
                    c.arg("list");
                    match run_bounded(c, LIST_TIMEOUT, &token) {
                        Ok(out) => state.user_shortcuts = parse_list(&out),
                        Err(e) => state.last_result = Some(Err(format!("Raccourcis : {e}"))),
                    }
                    publish(&hub, &state);
                }
                Err(_) => break,
            },
            default(Duration::from_millis(500)) => {},
        }
    }
    hub.update(|s| s.shortcuts = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shortcut_list() {
        assert_eq!(
            parse_list("Zeta\n\n alpha \nZeta\nBeta\n"),
            vec!["alpha", "Beta", "Zeta"]
        );
    }

    #[test]
    fn url_validation() {
        assert!(valid_url("https://example.org"));
        assert!(valid_url(
            "x-apple.systempreferences:com.apple.preference.security"
        ));
        assert!(!valid_url("file:///etc/passwd"));
        assert!(!valid_url("/Applications/Calculator.app"));
        assert!(!valid_url("nocolon"));
    }

    #[test]
    fn bounded_process_times_out_and_is_killed() {
        let t = CancelToken::new();
        let mut c = Command::new("/bin/sleep");
        c.arg("5");
        let start = Instant::now();
        assert_eq!(
            run_bounded(c, Duration::from_millis(200), &t),
            Err("délai dépassé".into())
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        let mut c = Command::new("/bin/echo");
        c.arg("ok");
        assert_eq!(
            run_bounded(c, Duration::from_secs(5), &t).unwrap().trim(),
            "ok"
        );
    }

    #[test]
    #[ignore = "real machine: runs `shortcuts list` once (no shortcut is executed)"]
    fn live_shortcuts_list() {
        let mut c = Command::new(SHORTCUTS);
        c.arg("list");
        let r = run_bounded(c, LIST_TIMEOUT, &CancelToken::new());
        // Count only: shortcut names are personal.
        eprintln!(
            "shortcuts list: {:?}",
            r.as_ref().map(|o| parse_list(o).len())
        );
        assert!(r.is_ok());
    }
}
