//! Nox closed-lid helper: a minimal root launchd daemon that toggles
//! `pmset disablesleep` on behalf of the logged-in user's Nox app and restores
//! it on its own when the app goes away (crash, kill -9, force quit), when the
//! battery or thermal guards trip, and at boot (see docs/SLEEP_RECOVERY.md).
//!
//! Layout:
//! - [`protocol`] — line-based wire format, strict whitelist of requests.
//! - [`core`] — lease state machine + restore logic (pure, time injected).
//! - [`journal`] — root-owned record written *before* changing the setting.
//! - [`backend`] — the only side effects: read state, run `/usr/bin/pmset`.
//! - [`server`] / [`peer`] — Unix socket loop and peer verification.
//! - [`sys`] — unsafe FFI (CoreFoundation, IOKit, libnotify, libproc).
//! - [`launchd`] — LaunchDaemon plist generation for the install script.

pub mod backend;
pub mod core;
pub mod journal;
pub mod launchd;
pub mod peer;
pub mod protocol;
pub mod server;
pub mod sys;

/// launchd label, also the installed binary name.
pub const LABEL: &str = "io.github.martingaltier.nox.helper";
/// Socket the app connects to.
pub const SOCKET_PATH: &str = "/var/run/io.github.martingaltier.nox.helper.sock";
/// Installed helper binary.
pub const INSTALLED_BINARY: &str =
    "/Library/PrivilegedHelperTools/io.github.martingaltier.nox.helper";
/// Installed LaunchDaemon plist.
pub const LAUNCHD_PLIST: &str = "/Library/LaunchDaemons/io.github.martingaltier.nox.helper.plist";
/// Root-owned state directory and journal.
pub const STATE_DIR: &str = "/Library/Application Support/Nox";
pub const JOURNAL_PATH: &str = "/Library/Application Support/Nox/helper-state.json";
/// Helper log (stderr redirected by launchd). Contains no personal data.
pub const LOG_PATH: &str = "/Library/Logs/Nox/nox-helper.log";
/// Path of the helper inside Nox.app.
pub const BUNDLED_HELPER_RELATIVE: &str = "Contents/Library/LaunchServices/nox-helper";

/// Bumped on any incompatible wire change.
pub const PROTOCOL_VERSION: u32 = 1;
/// Crate version (helper and app are built from the same workspace).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The only program the helper ever executes, with fixed arguments.
pub const PMSET: &str = "/usr/bin/pmset";

/// Small timestamped stderr logger (the helper has no other dependencies).
pub fn log_line(msg: &str) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    eprintln!("[{secs}] nox-helper: {msg}");
}
