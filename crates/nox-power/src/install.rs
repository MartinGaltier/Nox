//! Helper installation / removal, only on explicit user action.
//!
//! The scripts in `scripts/{install,uninstall}-helper.sh` are embedded in the
//! app binary (covered by its code signature) and run once through the macOS
//! administrator prompt: `osascript -e 'do shell script … with administrator
//! privileges'`. The password goes to the system dialog only; Nox never sees
//! or stores it and never edits sudoers.
//!
//! SMAppService.daemon was evaluated and not used: it needs the daemon to
//! stay inside the app bundle (moving/deleting the app would break boot
//! recovery), and with an ad-hoc signature (no Team ID) each rebuild changes
//! the code identity that Background Task Management approved. See
//! docs/SLEEP_RECOVERY.md.

use nox_core::CancelToken;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const INSTALL_SCRIPT: &str = include_str!("../../../scripts/install-helper.sh");
const UNINSTALL_SCRIPT: &str = include_str!("../../../scripts/uninstall-helper.sh");

/// Helper binary and LaunchDaemon plist are both in place (no privileges
/// needed to check).
pub fn helper_files_present() -> bool {
    Path::new(nox_helper::INSTALLED_BINARY).is_file()
        && Path::new(nox_helper::LAUNCHD_PLIST).is_file()
}

/// The `.app` bundle this process runs from, if it contains the helper.
pub fn current_app_bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.canonicalize().unwrap_or(exe);
    // …/Nox.app/Contents/MacOS/nox
    let bundle = exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|b| b.extension().is_some_and(|e| e == "app"))
        .ok_or("Lancez Nox depuis Nox.app (scripts/build-app.sh) pour installer l’assistant.")?;
    if !bundle.join(nox_helper::BUNDLED_HELPER_RELATIVE).is_file() {
        return Err("Assistant absent du paquet Nox.app : reconstruisez l’app.".into());
    }
    Ok(bundle.to_owned())
}

/// POSIX shell single-quoting.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// AppleScript string literal.
fn as_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

/// The AppleScript run by osascript: `/bin/sh -c '<script>' <name> <args…>`
/// as root after the system password prompt.
fn admin_script(script: &str, name: &str, args: &[&str], prompt: &str) -> String {
    let mut cmd = format!("/bin/sh -c {} {}", sh_quote(script), sh_quote(name));
    for a in args {
        cmd.push(' ');
        cmd.push_str(&sh_quote(a));
    }
    format!(
        "do shell script {} with administrator privileges with prompt {}",
        as_quote(&cmd),
        as_quote(prompt)
    )
}

/// Runs osascript once and waits for the user to answer the system prompt.
/// Cancelling the token (module stop / app quit) kills the prompt.
fn run_admin(applescript: &str, token: &CancelToken) -> Result<String, String> {
    let mut child = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(applescript)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("osascript : {e}"))?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if token.sleep(Duration::from_millis(100)) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Opération interrompue (Nox s’arrête).".into());
                }
            }
            Err(e) => return Err(format!("osascript : {e}")),
        }
    };
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut err);
    }
    if status.success() {
        Ok(out.trim().to_owned())
    } else if err.contains("-128") {
        Err("Opération annulée.".into())
    } else {
        Err(err.trim().chars().take(300).collect())
    }
}

/// Blocking: shows the administrator prompt and installs from `bundle`.
pub fn install(bundle: &Path, token: &CancelToken) -> Result<(), String> {
    let bundle = bundle.to_str().ok_or("chemin de l’app non UTF-8")?;
    run_admin(
        &admin_script(
            INSTALL_SCRIPT,
            "install-helper.sh",
            &[bundle],
            "Nox souhaite installer son assistant pour garder le Mac actif capot fermé.",
        ),
        token,
    )
    .map(|_| ())
}

/// Blocking: shows the administrator prompt and removes the helper (lid
/// sleep restored first when Nox had changed it).
pub fn uninstall(token: &CancelToken) -> Result<(), String> {
    run_admin(
        &admin_script(
            UNINSTALL_SCRIPT,
            "uninstall-helper.sh",
            &[],
            "Nox souhaite désinstaller son assistant capot fermé.",
        ),
        token,
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_hostile_paths() {
        let path = r#"/Users/x/It's "Nox" \ $(rm -rf ~).app"#;
        let s = admin_script("echo \"$1\"", "t.sh", &[path], "p\"q");
        assert!(s.starts_with("do shell script \"/bin/sh -c "));
        assert!(s.ends_with(r#"with administrator privileges with prompt "p\"q""#));
        // Undo the AppleScript layer, then run the shell layer (without
        // privileges) to check the argument arrives intact.
        let inner = s
            .strip_prefix("do shell script \"")
            .and_then(|r| r.split("\" with administrator").next())
            .unwrap()
            .replace("\\\"", "\"")
            .replace(r"\\", r"\");
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(&inner)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), path);
    }

    #[test]
    fn scripts_are_embedded_and_parse() {
        for script in [INSTALL_SCRIPT, UNINSTALL_SCRIPT] {
            assert!(script.contains(nox_helper::LABEL));
            let st = Command::new("/bin/sh")
                .arg("-n")
                .arg("-c")
                .arg(script)
                .status()
                .unwrap();
            assert!(st.success());
        }
        assert!(INSTALL_SCRIPT.contains(nox_helper::BUNDLED_HELPER_RELATIVE));
        assert!(!INSTALL_SCRIPT.contains("/etc/sudoers") && !INSTALL_SCRIPT.contains("visudo"));
    }

    /// The root-run install verifies the root-owned copy, never the
    /// user-writable source, and only ever executes the verified copy.
    #[test]
    fn install_verifies_the_copy_not_the_source() {
        let lines: Vec<&str> = INSTALL_SCRIPT.lines().map(str::trim).collect();
        let pos = |needle: &str| {
            lines
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("missing {needle:?}"))
        };
        let copy = pos(r#"install -o root -g wheel -m 0700 "$SRC" "$TMP""#);
        let verify = pos(r#"codesign --verify --strict -R="identifier \"$LABEL\"" "$TMP""#);
        let publish = pos(r#"mv -f "$TMP" "$DST""#);
        let run = pos(r#""$DST" --print-launchd-plist"#);
        assert!(copy < verify && verify < publish && publish < run);
        for l in &lines {
            if l.starts_with('#') {
                continue;
            }
            assert!(!l.contains(r#"codesign --verify --strict "$SRC""#), "{l}");
            assert!(!l.starts_with(r#""$SRC""#), "source executed: {l}");
        }
    }

    #[test]
    fn scripts_refuse_without_root() {
        if nox_helper::sys::euid() == 0 {
            return;
        }
        let st = Command::new("/bin/sh")
            .arg("-c")
            .arg(INSTALL_SCRIPT)
            .arg("install-helper.sh")
            .arg("/nonexistent/Nox.app")
            .status()
            .unwrap();
        assert_eq!(st.code(), Some(77));
        let st = Command::new("/bin/sh")
            .arg("-c")
            .arg(UNINSTALL_SCRIPT)
            .status()
            .unwrap();
        assert_eq!(st.code(), Some(77));
    }
}
