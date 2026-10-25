//! User LaunchAgent that runs `nox --lid-restore` at login. Without a root
//! daemon, this is what puts `SleepDisabled` back after a crash followed by
//! a reboot: the value persists across reboots, the journal persists in the
//! user's Application Support folder, and the agent reads it. Created and
//! removed together with the sudoers rule.

use std::path::{Path, PathBuf};

pub const LABEL: &str = "io.github.martingaltier.nox.lid-restore";

pub fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        Path::new(&home)
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")),
    )
}

/// Escapes the five XML special characters.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn plist(program: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
        <string>--lid-restore</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
"#,
        program = xml(&program.to_string_lossy()),
    )
}

pub fn install(program: &Path) -> std::io::Result<()> {
    let Some(path) = path() else {
        return Err(std::io::Error::other("HOME inconnu"));
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, plist(program))
}

pub fn remove() -> std::io::Result<()> {
    match path() {
        Some(p) => match std::fs::remove_file(p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_runs_restore_at_login_as_user() {
        let p = plist(Path::new("/Applications/Nox & Co.app/Contents/MacOS/nox"));
        assert!(p.contains("<string>--lid-restore</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(p.contains("Nox &amp; Co.app"));
        assert!(!p.contains("UserName") && !p.contains("LaunchDaemons"));
    }
}
