//! Quick actions (E6): lock, screenshot (Screenshot.app), dark mode, screen
//! saver, empty Trash (Finder, Automation), open app/URL, run a Shortcut.
//! Every external process is one-shot, on demand, bounded and killed on stop.

use crate::sys::audio::{self, Dir};
use crate::sys::{display, system, workspace};
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::shortcuts::{ControlsState, QuickAction, ShortcutCommand, ShortcutsState};
use nox_core::{
    CancelToken, Hub, Module, ModuleContext, ModuleError, ModuleId, Reading, Unavailable, Worker,
};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SHORTCUTS: &str = "/usr/bin/shortcuts";
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
const RUN_TIMEOUT: Duration = Duration::from_secs(120);
const TRASH_TIMEOUT: Duration = Duration::from_secs(130);

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
            // `--`: a shortcut named like an option stays a name.
            c.arg("run").arg("--").arg(name);
            run_bounded(c, RUN_TIMEOUT, token).map(|_| format!("« {name} » exécuté"))
        }
        QuickAction::EmptyTrash => {
            // The UI confirms first (permanent). Emptying a large Trash can
            // take a while, so it runs in a one-shot, killable `osascript`
            // instead of NSAppleScript on the main thread. Finder Automation
            // prompt on first use (attributed to Nox, the responsible app).
            let mut c = Command::new("/usr/bin/osascript");
            c.args([
                "-e",
                "with timeout of 120 seconds\ntell application id \"com.apple.finder\" to empty trash\nend timeout",
            ]);
            run_bounded(c, TRASH_TIMEOUT, token)
                .map(|_| "Corbeille vidée".to_owned())
                .map_err(|e| format!("Corbeille : {e} (Automatisation pour Finder autorisée ?)"))
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

/// Reads every control; a control the device does not expose is reported
/// as not supported, never as 0.
pub fn read_controls() -> ControlsState {
    fn na<T>() -> Reading<T> {
        Reading::Unavailable(Unavailable::NotSupported)
    }
    let out = audio::default_device(Dir::Output);
    let inp = audio::default_device(Dir::Input);
    ControlsState {
        volume: out
            .and_then(audio::volume)
            .map(Reading::Measured)
            .unwrap_or_else(na),
        muted: out
            .and_then(|d| audio::muted(d, Dir::Output))
            .map(Reading::Measured)
            .unwrap_or_else(na),
        mic_muted: inp
            .and_then(|d| audio::muted(d, Dir::Input))
            .map(Reading::Measured)
            .unwrap_or_else(na),
        brightness: display::brightness()
            .map(Reading::Measured)
            .unwrap_or_else(na),
        output_device: out.and_then(audio::device_name),
        input_device: inp.and_then(audio::device_name),
    }
}

fn set_control(c: &ShortcutCommand) -> Result<(), String> {
    let ok =
        match *c {
            ShortcutCommand::SetVolume(v) if v.is_finite() => {
                audio::default_device(Dir::Output).is_some_and(|d| audio::set_volume(d, v))
            }
            ShortcutCommand::SetMuted(m) => audio::default_device(Dir::Output)
                .is_some_and(|d| audio::set_muted(d, Dir::Output, m)),
            ShortcutCommand::SetMicMuted(m) => audio::default_device(Dir::Input)
                .is_some_and(|d| audio::set_muted(d, Dir::Input, m)),
            ShortcutCommand::SetBrightness(v) if v.is_finite() => display::set_brightness(v),
            _ => false,
        };
    ok.then_some(())
        .ok_or_else(|| "réglage non pris en charge par cet appareil".to_owned())
}

fn run(hub: Hub, rx: Receiver<ShortcutCommand>, token: CancelToken) {
    let mut state = ShortcutsState {
        controls: read_controls(),
        ..Default::default()
    };
    publish(&hub, &state);
    let (audio_tx, audio_rx) = crossbeam_channel::bounded(1);
    let mut listeners = Some(audio::Listeners::new(audio_tx.clone()));
    let devices = || {
        (
            audio::default_device(Dir::Output),
            audio::default_device(Dir::Input),
        )
    };
    let mut current = devices();
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
                Ok(ShortcutCommand::RefreshControls) => {
                    state.controls = read_controls();
                    publish(&hub, &state);
                }
                Ok(c) => {
                    if let Err(e) = set_control(&c) {
                        state.last_result = Some(Err(e));
                    }
                    state.controls = read_controls();
                    publish(&hub, &state);
                }
                Err(_) => break,
            },
            recv(audio_rx) -> _ => {
                // Coalesce the burst a device switch produces.
                std::thread::sleep(Duration::from_millis(50));
                let now = devices();
                if now != current {
                    current = now;
                    // Unregister the old device before listening to the new one.
                    drop(listeners.take());
                    listeners = Some(audio::Listeners::new(audio_tx.clone()));
                }
                let c = read_controls();
                state.controls.volume = c.volume;
                state.controls.muted = c.muted;
                state.controls.mic_muted = c.mic_muted;
                state.controls.output_device = c.output_device;
                state.controls.input_device = c.input_device;
                publish(&hub, &state);
            },
            default(Duration::from_millis(500)) => {},
        }
    }
    drop(listeners);
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

    #[test]
    #[ignore = "real machine: reads controls and re-applies the current volume (no audible change)"]
    fn live_controls() {
        let c = read_controls();
        eprintln!(
            "volume {:?} muted {:?} mic_muted {:?} brightness {:?} out {:?}",
            c.volume,
            c.muted,
            c.mic_muted,
            c.brightness,
            c.output_device.is_some()
        );
        if let (Some(d), Some(v)) = (
            audio::default_device(Dir::Output),
            c.volume.value().copied(),
        ) {
            assert!(audio::set_volume(d, v), "setting the same volume succeeds");
            assert!((audio::volume(d).unwrap() - v).abs() < 0.01);
        }
    }
}
