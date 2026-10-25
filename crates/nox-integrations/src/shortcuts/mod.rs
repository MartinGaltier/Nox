//! Quick actions (E6): lock, screenshot (Screenshot.app), dark mode, screen
//! saver, empty Trash (Finder, Automation), open app/URL, run a Shortcut.
//! Every external process is one-shot, on demand, bounded and killed on stop.
//! Also the system controls (E7) and their in-island HUD (E8).

use crate::sys::audio::{self, Dir};
use crate::sys::{display, keyboard, system, workspace};
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::shortcuts::{
    ControlHud, ControlsState, HudKind, InputSourceInfo, QuickAction, ShortcutCommand,
    ShortcutsState,
};
use nox_core::{
    CancelToken, Hub, Module, ModuleContext, ModuleError, ModuleId, Reading, Settings, Unavailable,
    Worker,
};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SHORTCUTS: &str = "/usr/bin/shortcuts";
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
const RUN_TIMEOUT: Duration = Duration::from_secs(120);
const TRASH_TIMEOUT: Duration = Duration::from_secs(130);

#[derive(Default)]
pub struct ShortcutsModule {
    worker: Option<Worker>,
    tx: Option<Sender<ShortcutCommand>>,
    hud: Arc<AtomicBool>,
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
        self.hud
            .store(ctx.settings.shortcuts.hud_in_island, Ordering::SeqCst);
        let hud = self.hud.clone();
        self.worker = Some(Worker::spawn("shortcuts", move |token| {
            run(hub, rx, hud, token)
        }));
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

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        self.hud
            .store(settings.shortcuts.hud_in_island, Ordering::SeqCst);
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

/// `shortcuts run -- <name>`: after `--` a name such as `-o /x` stays a name.
pub fn run_shortcut_command(name: &str) -> Command {
    let mut c = Command::new(SHORTCUTS);
    c.arg("run").arg("--").arg(name);
    c
}

/// Emptying a large Trash can take a while: a one-shot, killable `osascript`
/// on a worker thread, never NSAppleScript on the main thread (which would
/// freeze the UI). The script is constant (no user input interpolated).
pub fn trash_command() -> Command {
    let mut c = Command::new("/usr/bin/osascript");
    c.args([
        "-e",
        "with timeout of 120 seconds\ntell application id \"com.apple.finder\" to empty trash\nend timeout",
    ]);
    c
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
        QuickAction::RunShortcut(name) => run_bounded(run_shortcut_command(name), RUN_TIMEOUT, token)
            .map(|_| format!("« {name} » exécuté")),
        QuickAction::EmptyTrash => {
            // The UI confirms first (permanent). Finder Automation prompt on
            // first use (attributed to Nox, the responsible app).
            run_bounded(trash_command(), TRASH_TIMEOUT, token)
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

/// Applies a control change and publishes only if a control really moved:
/// notifications that leave every value as it was do not wake the UI.
fn publish_if_changed(
    hub: &Hub,
    state: &mut ShortcutsState,
    on_change: impl Fn(&mut ShortcutsState),
) {
    let before = state.controls.clone();
    on_change(state);
    if state.controls != before {
        publish(hub, state);
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
        keyboard_backlight: keyboard::backlight()
            .map(Reading::Measured)
            .unwrap_or_else(na),
        input_source: keyboard::current_input_source().map(info),
        input_sources: keyboard::input_sources().into_iter().map(info).collect(),
        hud: None,
    }
}

fn info(s: keyboard::Source) -> InputSourceInfo {
    InputSourceInfo {
        id: s.id,
        name: s.name,
    }
}

/// The HUD to show after a control changed (E8), comparing two readings.
pub fn hud_for(before: &ControlsState, after: &ControlsState, now: f64) -> Option<ControlHud> {
    let v = |r: &Reading<f32>| r.value().copied();
    let b = |r: &Reading<bool>| r.value().copied().unwrap_or(false);
    if v(&before.volume) != v(&after.volume) || b(&before.muted) != b(&after.muted) {
        return Some(ControlHud {
            kind: HudKind::Volume,
            value: v(&after.volume),
            muted: b(&after.muted),
            at: now,
        });
    }
    if b(&before.mic_muted) != b(&after.mic_muted) {
        return Some(ControlHud {
            kind: HudKind::Microphone,
            value: None,
            muted: b(&after.mic_muted),
            at: now,
        });
    }
    let moved = |x: Option<f32>, y: Option<f32>| match (x, y) {
        (Some(x), Some(y)) => (x - y).abs() > 0.004,
        (x, y) => x != y,
    };
    if moved(v(&before.brightness), v(&after.brightness)) {
        return Some(ControlHud {
            kind: HudKind::Brightness,
            value: v(&after.brightness),
            muted: false,
            at: now,
        });
    }
    None
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
            ShortcutCommand::SetKeyboardBacklight(v) if v.is_finite() => keyboard::set_backlight(v),
            ShortcutCommand::SelectInputSource(ref id) => {
                let id = id.clone();
                // TIS selection belongs to the main thread.
                crate::sys::on_main(crate::sys::MAIN_TIMEOUT, move || {
                    keyboard::select_input_source(&id)
                })
                .unwrap_or(false)
            }
            _ => false,
        };
    ok.then_some(())
        .ok_or_else(|| "réglage non pris en charge par cet appareil".to_owned())
}

fn run(hub: Hub, rx: Receiver<ShortcutCommand>, hud: Arc<AtomicBool>, token: CancelToken) {
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
    // Brightness has no public notification; DisplayServices' private one
    // feeds the HUD when present (registered on the main run loop).
    let (bright_tx, bright_rx) = crossbeam_channel::bounded(1);
    // Keeps `bright_rx` connected when the observer is never created, so the
    // select below cannot spin on a disconnected channel.
    let _bright_keepalive = bright_tx.clone();
    let brightness_observer = crate::sys::on_main(crate::sys::MAIN_TIMEOUT, move || {
        display::BrightnessObserver::new(bright_tx)
    })
    .flatten();
    let on_change = |state: &mut ShortcutsState| {
        let after = read_controls();
        let c = &mut state.controls;
        let h = hud_for(c, &after, nox_core::model::now());
        c.volume = after.volume;
        c.muted = after.muted;
        c.mic_muted = after.mic_muted;
        c.brightness = after.brightness;
        c.output_device = after.output_device;
        c.input_device = after.input_device;
        if hud.load(Ordering::SeqCst) && h.is_some() {
            c.hud = h;
        }
    };
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
                publish_if_changed(&hub, &mut state, on_change);
            },
            recv(bright_rx) -> _ => {
                publish_if_changed(&hub, &mut state, on_change);
            },
        }
    }
    drop(listeners);
    if let Some(o) = brightness_observer {
        nox_core::main_thread::run_async(move || drop(o));
    }
    hub.update(|s| s.shortcuts = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn unchanged_controls_do_not_wake_the_ui() {
        let hub = Hub::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        hub.set_waker(move || {
            w.fetch_add(1, Ordering::SeqCst);
        });
        hub.update(|s| s.shortcuts = Some(ShortcutsState::default()));
        hub.acknowledge_wake();
        let mut state = ShortcutsState::default();
        let wakes_now = || wakes.load(Ordering::SeqCst);
        let before = wakes_now();

        // A notification that leaves every value as it was: nothing published.
        publish_if_changed(&hub, &mut state, |_| {});
        assert_eq!(wakes_now(), before);

        // A control that moved: published once.
        publish_if_changed(&hub, &mut state, |s| {
            s.controls.volume = Reading::Measured(0.7);
        });
        assert_eq!(wakes_now(), before + 1);
        assert!(matches!(
            hub.read(|st| st.shortcuts.as_ref().map(|s| s.controls.volume.clone())),
            Some(Reading::Measured(v)) if v == 0.7
        ));
    }

    #[test]
    fn hud_follows_the_control_that_changed() {
        let base = ControlsState {
            volume: Reading::Measured(0.5),
            muted: Reading::Measured(false),
            mic_muted: Reading::Measured(false),
            brightness: Reading::Measured(0.7),
            ..Default::default()
        };
        assert_eq!(hud_for(&base, &base, 1.0), None);
        let mut v = base.clone();
        v.volume = Reading::Measured(0.6);
        assert_eq!(hud_for(&base, &v, 1.0).unwrap().kind, HudKind::Volume);
        let mut m = base.clone();
        m.muted = Reading::Measured(true);
        assert!(hud_for(&base, &m, 1.0).unwrap().muted);
        let mut b = base.clone();
        b.brightness = Reading::Measured(0.702);
        assert_eq!(hud_for(&base, &b, 1.0), None, "sensor jitter ignored");
        b.brightness = Reading::Measured(0.8);
        assert_eq!(hud_for(&base, &b, 1.0).unwrap().kind, HudKind::Brightness);
        let mut mic = base.clone();
        mic.mic_muted = Reading::Measured(true);
        assert_eq!(hud_for(&base, &mic, 1.0).unwrap().kind, HudKind::Microphone);
    }

    #[test]
    fn parses_shortcut_list() {
        assert_eq!(
            parse_list("Zeta\n\n alpha \nZeta\nBeta\n"),
            vec!["alpha", "Beta", "Zeta"]
        );
    }

    #[test]
    fn process_arguments_cannot_become_options() {
        let c = run_shortcut_command("-o /tmp/x");
        let args: Vec<_> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["run", "--", "-o /tmp/x"]);
        let t = trash_command();
        assert_eq!(
            t.get_program(),
            "/usr/bin/osascript",
            "off the main thread, killable"
        );
        let script: Vec<_> = t
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(script[0], "-e");
        assert!(script[1].starts_with("with timeout of 120 seconds"));
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
        eprintln!(
            "keyboard backlight {:?}; input sources: {} (current known: {})",
            c.keyboard_backlight,
            c.input_sources.len(),
            c.input_source.is_some()
        );
        if let Some(k) = c.keyboard_backlight.value().copied() {
            assert!(
                keyboard::set_backlight(k),
                "setting the same backlight succeeds"
            );
            assert!((keyboard::backlight().unwrap() - k).abs() < 0.01);
        }
    }
}
