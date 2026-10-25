//! Data models published by modules and rendered by the UI.
//!
//! Ownership (edit only your files; changes elsewhere go through the coordinator):
//! - Intégrations: music, calendar, notifications, accessories, downloads, files, shortcuts, camera, captures, scratchpad, clipboard
//! - Activités: agents, watch, timers, companion
//! - Mesures: metrics, mixer
//! - Énergie: power

pub mod accessories;
pub mod agents;
pub mod calendar;
pub mod camera;
pub mod captures;
pub mod clipboard;
pub mod companion;
pub mod downloads;
pub mod files;
pub mod metrics;
pub mod mixer;
pub mod music;
pub mod notifications;
pub mod power;
pub mod scratchpad;
pub mod shortcuts;
pub mod timers;
pub mod watch;

/// Seconds since the Unix epoch (serialisable wall-clock timestamp).
pub type Timestamp = f64;

pub fn now() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
