//! Local storage locations. Everything Nox writes lives under these folders.

use std::path::PathBuf;

pub const BUNDLE_ID: &str = "io.github.martingaltier.nox";
pub const APP_NAME: &str = "Nox";

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `~/Library/Application Support/Nox` (settings, small state files).
pub fn app_support_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NOX_DATA_DIR") {
        return PathBuf::from(dir);
    }
    home().join("Library/Application Support").join(APP_NAME)
}

/// `~/Library/Caches/<bundle id>` (artwork, lyrics, price tables).
pub fn cache_dir() -> PathBuf {
    home().join("Library/Caches").join(BUNDLE_ID)
}

/// `~/Library/Logs/Nox`.
pub fn logs_dir() -> PathBuf {
    home().join("Library/Logs").join(APP_NAME)
}

/// Per-module folder inside Application Support, created on demand.
pub fn module_dir(module: &str) -> std::io::Result<PathBuf> {
    let d = app_support_dir().join(module);
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// Atomic write: temp file + fsync + rename, mode 0600.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, path)
}
