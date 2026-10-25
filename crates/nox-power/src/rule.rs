// Adapted from vorssaint-utils (GPL-3.0-or-later), Services/SudoersSupport.swift, commit e916961.
//! The sudoers rule that lets Nox run `pmset -a disablesleep 1|0` without a
//! password. Installed once, on explicit user action, through the macOS
//! administrator prompt (`osascript … with administrator privileges`). Nox
//! never sees or stores the password and never edits sudoers itself: the
//! embedded script `scripts/install-lid-rule.sh` does, as root.
//!

use crate::launch_agent;
use crate::pmset::{PMSET, SUDO};
use nox_core::CancelToken;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// The previous privileged helper (a root LaunchDaemon). Removed together
/// with the rule; see `scripts/install-lid-rule.sh`.
const LEGACY_PLIST: &str = "/Library/LaunchDaemons/io.github.martingaltier.nox.helper.plist";
const INSTALL_SCRIPT: &str = include_str!("../../../scripts/install-lid-rule.sh");
const SAFE_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The rule file for `uid`. A numeric uid interpolates as bare digits; a user
/// name would need sanitising against sudoers and shell syntax.
pub fn rule_text(uid: u32) -> String {
    format!("#{uid} ALL=(root) NOPASSWD: {PMSET} -a disablesleep 1, {PMSET} -a disablesleep 0\n")
}

/// `sudo -n -l` on the exact command Nox runs: succeeds only when the rule
/// is in place and matches. Never prompts.
pub fn installed() -> bool {
    Command::new(SUDO)
        .args(["-n", "-l", PMSET, "-a", "disablesleep", "1"])
        .env_clear()
        .env("PATH", SAFE_PATH)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn legacy_helper_present() -> bool {
    Path::new(LEGACY_PLIST).exists()
}

/// Current uid as the rule needs it.
pub fn current_uid() -> Result<u32, String> {
    let out = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map_err(|e| format!("id : {e}"))?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| "uid illisible".to_string())
}

/// Installs the rule (and, when `remove_legacy`, removes the previous helper
/// daemon) with one administrator prompt, then the user-level LaunchAgent that
/// restores lid sleep after a reboot. Blocking.
pub fn install(remove_legacy: bool, exe: &Path, token: &CancelToken) -> Result<(), String> {
    let uid = current_uid()?.to_string();
    let legacy = if remove_legacy { "1" } else { "0" };
    run_admin(
        &admin_script(
            &["install", uid.as_str(), legacy],
            "Nox souhaite autoriser le mode « Mac actif capot fermé » : une règle sudo \
             limitée à pmset disablesleep pour votre compte.",
        ),
        token,
    )?;
    if !installed() {
        return Err("La règle est écrite mais sudo ne l’accepte pas encore.".into());
    }
    launch_agent::install(exe).map_err(|e| format!("agent de restauration : {e}"))
}

/// Removes the rule and the LaunchAgent, and the previous helper if still
/// there. Blocking. The caller stops any active session first.
pub fn uninstall(token: &CancelToken) -> Result<(), String> {
    run_admin(
        &admin_script(
            &["remove"],
            "Nox souhaite retirer l’autorisation du mode capot fermé (et l’ancien assistant, s’il est encore là).",
        ),
        token,
    )?;
    launch_agent::remove().map_err(|e| format!("agent de restauration : {e}"))
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
fn admin_script(args: &[&str], prompt: &str) -> String {
    let mut cmd = format!(
        "/bin/sh -c {} install-lid-rule.sh",
        sh_quote(INSTALL_SCRIPT)
    );
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
fn run_admin(applescript: &str, token: &CancelToken) -> Result<(), String> {
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
    if status.success() {
        return Ok(());
    }
    let mut err = String::new();
    if let Some(mut s) = child.stderr.take() {
        use std::io::Read;
        let _ = s.read_to_string(&mut err);
    }
    if err.contains("-128") {
        Err("Opération annulée.".into())
    } else {
        Err(err.trim().chars().take(300).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_text_is_uid_based_and_exact() {
        assert_eq!(
            rule_text(501),
            "#501 ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0\n"
        );
    }

    #[test]
    fn rule_text_parses_as_sudoers() {
        let dir = std::env::temp_dir().join(format!("nox-rule-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rule");
        std::fs::write(&file, rule_text(501)).unwrap();
        let ok = Command::new("/usr/sbin/visudo")
            .arg("-cf")
            .arg(&file)
            .output()
            .unwrap()
            .status
            .success();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(ok, "visudo rejected the rule");
    }

    /// The shell writes the same grant as `rule_text`: same command strings.
    #[test]
    fn install_script_grants_the_same_commands() {
        let grant = rule_text(0);
        let grant = grant.split_once(" ALL=").unwrap().1.trim_end();
        assert!(INSTALL_SCRIPT.contains(grant), "{grant}");
        let st = Command::new("/bin/sh")
            .arg("-n")
            .arg("-c")
            .arg(INSTALL_SCRIPT)
            .status()
            .unwrap();
        assert!(st.success());
    }

    #[test]
    fn script_refuses_without_root() {
        if current_uid() == Ok(0) {
            return;
        }
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(INSTALL_SCRIPT)
            .arg("install-lid-rule.sh")
            .arg("install")
            .arg("501")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(77));
        assert!(out.stdout.is_empty());
    }

    #[test]
    fn admin_script_quoting_survives_hostile_arguments() {
        let s = admin_script(&["install", "501", "1"], "p\"q");
        assert!(s.starts_with("do shell script \"/bin/sh -c "));
        assert!(s.ends_with(r#"with administrator privileges with prompt "p\"q""#));
        // Undo the AppleScript layer, then check the shell layer hands the
        // arguments over intact (run with a no-op script, no privileges).
        let inner = s
            .strip_prefix("do shell script \"")
            .and_then(|r| r.split("\" with administrator").next())
            .unwrap()
            .replace("\\\"", "\"")
            .replace(r"\\", r"\");
        assert!(
            inner.ends_with("install-lid-rule.sh 'install' '501' '1'"),
            "{inner}"
        );
    }
}
