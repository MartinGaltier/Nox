//! Minimal logger: stderr + ~/Library/Logs/Nox/nox.log (rotated at 2 MB).
//! Never logs private content (notification bodies, event titles, prompts).

use std::io::Write;
use std::sync::Mutex;

struct Logger {
    file: Option<Mutex<std::fs::File>>,
    level: log::LevelFilter,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= self.level
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!(
            "{:.3} {:<5} {}: {}\n",
            nox_core::model::now(),
            r.level(),
            r.target(),
            r.args()
        );
        let _ = std::io::stderr().write_all(line.as_bytes());
        if let Some(f) = &self.file {
            let _ = f
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .write_all(line.as_bytes());
        }
    }
    fn flush(&self) {}
}

pub fn init() {
    let dir = nox_core::paths::logs_dir();
    let path = dir.join("nox.log");
    let _ = std::fs::create_dir_all(&dir);
    if std::fs::metadata(&path)
        .map(|m| m.len() > 2_000_000)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join("nox.1.log"));
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
        .map(Mutex::new);
    let level = match std::env::var("NOX_LOG").as_deref() {
        Ok("debug") => log::LevelFilter::Debug,
        Ok("trace") => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    };
    if log::set_boxed_logger(Box::new(Logger { file, level })).is_ok() {
        log::set_max_level(level);
    }
}
