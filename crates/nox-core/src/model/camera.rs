use crate::permissions::PermissionState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CameraState {
    pub access: PermissionState,
    pub running: bool,
    /// Camera names (`localizedName`).
    pub devices: Vec<String>,
    /// Selected camera name; `None` = system default.
    pub selected: Option<String>,
    /// Why the preview could not start (device busy, no camera…).
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CameraCommand {
    RequestAccess,
    Start,
    Stop,
    Select(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSettings {
    pub mirror: bool,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self { mirror: true }
    }
}

// ---------------------------------------------------------------------------
// Preview host (UI ⇄ camera module), agreed with Agent Interface.
//
// The UI owns a host `CALayer` while the camera page is visible and passes
// its address (`CALayer *` as `usize`, retained by the UI until it calls
// `set_preview_host(None)`) from the main thread. The camera module adds its
// `AVCaptureVideoPreviewLayer` as a sublayer of that host and removes it when
// the host goes away. The last host is remembered so a module started later
// still finds it.

type HostHandler = std::sync::Arc<dyn Fn(Option<usize>) + Send + Sync>;

static HOST_HANDLER: std::sync::Mutex<Option<HostHandler>> = std::sync::Mutex::new(None);
static HOST: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);

/// Camera module side: installed on start, `clear_preview_host_handler` on stop.
/// The handler is called on the main thread (from `set_preview_host`).
pub fn set_preview_host_handler(f: impl Fn(Option<usize>) + Send + Sync + 'static) {
    *HOST_HANDLER.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(f));
}

pub fn clear_preview_host_handler() {
    HOST_HANDLER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
}

/// UI side, main thread only. `Some(ptr)`: attach the preview to this
/// `CALayer *`; `None`: detach now (synchronously, before this returns).
pub fn set_preview_host(host: Option<usize>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = host;
    // Called outside the lock so the handler may read `preview_host()`.
    let handler = HOST_HANDLER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(h) = handler {
        h(host);
    }
}

/// The host last set by the UI.
pub fn preview_host() -> Option<usize> {
    *HOST.lock().unwrap_or_else(|e| e.into_inner())
}
