//! Camera mirror (E5). The capture session exists only between
//! `CameraCommand::Start` and `Stop` (page visible): the camera light is off
//! otherwise. The preview layer goes into the UI's host layer (see
//! `nox_core::model::camera::set_preview_host`). No frame is recorded.

use crate::sys::avcapture::{self, Capture};
use crate::sys::workspace;
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use nox_core::model::camera::{
    CameraCommand, CameraState, clear_preview_host_handler, preview_host, set_preview_host_handler,
};
use nox_core::permissions::{Permission, PermissionState};
use nox_core::{CancelToken, Hub, Module, ModuleContext, ModuleError, ModuleId, Settings, Worker};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

enum Msg {
    Command(CameraCommand),
    Settings(Arc<Settings>),
}

/// State shared with the main-thread host handler. Locks are held briefly
/// and never across a main-thread dispatch.
#[derive(Default)]
struct Shared {
    capture: Mutex<Option<Arc<Capture>>>,
    mirror: AtomicBool,
}

impl Shared {
    fn current(&self) -> Option<Arc<Capture>> {
        self.capture
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    /// Main thread: (re)attach to the current host, or detach.
    fn sync_layer(&self) {
        if let Some(c) = self.current() {
            match preview_host() {
                Some(host) => c.attach(host, self.mirror.load(Ordering::SeqCst)),
                None => c.detach(),
            }
        }
    }
}

#[derive(Default)]
pub struct CameraModule {
    worker: Option<Worker>,
    tx: Option<Sender<Msg>>,
    shared: Arc<Shared>,
}

impl Module for CameraModule {
    fn id(&self) -> ModuleId {
        ModuleId::Camera
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        let shared = Arc::new(Shared::default());
        shared
            .mirror
            .store(ctx.settings.camera.mirror, Ordering::SeqCst);
        let s2 = shared.clone();
        // Called by the UI on the main thread.
        set_preview_host_handler(move |_host| s2.sync_layer());
        let (tx, rx) = unbounded();
        let hub = ctx.hub.clone();
        let s3 = shared.clone();
        hub.update(|s| s.camera = Some(CameraState::default()));
        self.worker = Some(Worker::spawn("camera", move |token| {
            run(hub, s3, rx, token)
        }));
        self.shared = shared;
        self.tx = Some(tx);
        Ok(())
    }

    fn stop(&mut self) {
        clear_preview_host_handler();
        self.tx.take();
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }

    fn command(&mut self, cmd: nox_core::Command) {
        if let (nox_core::Command::Camera(c), Some(tx)) = (cmd, &self.tx) {
            let _ = tx.send(Msg::Command(c));
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Settings(settings.clone()));
        }
    }
}

struct Engine {
    hub: Hub,
    shared: Arc<Shared>,
    state: CameraState,
    device_ids: Vec<(String, String)>,
    /// The page wants the preview (Start received, no Stop since).
    wanted: bool,
    access_tx: Sender<bool>,
}

impl Engine {
    fn publish(&self) {
        let s = self.state.clone();
        self.hub.update(move |st| {
            if st.camera.is_some() {
                st.camera = Some(s);
            }
        });
    }

    fn refresh_devices(&mut self) {
        self.device_ids = avcapture::devices()
            .into_iter()
            .map(|d| (d.id, d.name))
            .collect();
        self.state.devices = self.device_ids.iter().map(|(_, n)| n.clone()).collect();
        if self
            .state
            .selected
            .as_ref()
            .is_some_and(|s| !self.state.devices.contains(s))
        {
            self.state.selected = None;
        }
    }

    fn selected_id(&self) -> Option<String> {
        let name = self.state.selected.as_ref()?;
        self.device_ids
            .iter()
            .find(|(_, n)| n == name)
            .map(|(id, _)| id.clone())
    }

    fn start_capture(&mut self) {
        self.state.access = avcapture::status();
        match self.state.access {
            PermissionState::Granted => {}
            PermissionState::NotDetermined => {
                avcapture::request_access(self.access_tx.clone());
                self.publish();
                return;
            }
            _ => {
                self.state.running = false;
                self.publish();
                return;
            }
        }
        if self.shared.current().is_some() {
            return;
        }
        self.refresh_devices();
        match Capture::open(self.selected_id().as_deref()) {
            Ok(c) => {
                let c = Arc::new(c);
                c.start();
                self.state.running = c.is_running();
                self.state.error = (!self.state.running).then(|| "caméra non démarrée".into());
                *self
                    .shared
                    .capture
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(c);
                let shared = self.shared.clone();
                nox_core::main_thread::run_async(move || shared.sync_layer());
            }
            Err(e) => {
                self.state.running = false;
                self.state.error = Some(e);
            }
        }
        self.publish();
    }

    fn stop_capture(&mut self) {
        let taken = self
            .shared
            .capture
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(c) = taken {
            c.stop();
            // Detach and release the layer on the main thread.
            nox_core::main_thread::run_async(move || {
                c.detach();
                drop(c);
            });
        }
        self.state.running = false;
        self.publish();
    }

    fn on_command(&mut self, c: CameraCommand) {
        match c {
            CameraCommand::Start => {
                self.wanted = true;
                self.start_capture();
            }
            CameraCommand::Stop => {
                self.wanted = false;
                self.stop_capture();
            }
            CameraCommand::Select(name) => {
                self.state.selected = Some(name);
                if self.shared.current().is_some() {
                    self.stop_capture();
                    if self.wanted {
                        self.start_capture();
                    }
                } else {
                    self.publish();
                }
            }
            CameraCommand::RequestAccess => match avcapture::status() {
                PermissionState::NotDetermined => avcapture::request_access(self.access_tx.clone()),
                PermissionState::Denied | PermissionState::Restricted => {
                    workspace::open_url(Permission::Camera.settings_url());
                }
                _ => {}
            },
        }
    }
}

fn run(hub: Hub, shared: Arc<Shared>, rx: Receiver<Msg>, token: CancelToken) {
    let (access_tx, access_rx) = unbounded();
    let mut e = Engine {
        hub,
        shared,
        state: CameraState {
            access: avcapture::status(),
            ..Default::default()
        },
        device_ids: vec![],
        wanted: false,
        access_tx,
    };
    e.refresh_devices();
    e.publish();
    while !token.is_cancelled() {
        select! {
            recv(rx) -> m => match m {
                Ok(Msg::Command(c)) => e.on_command(c),
                Ok(Msg::Settings(s)) => {
                    e.shared.mirror.store(s.camera.mirror, Ordering::SeqCst);
                    let shared = e.shared.clone();
                    nox_core::main_thread::run_async(move || shared.sync_layer());
                }
                Err(_) => break,
            },
            recv(access_rx) -> granted => {
                e.state.access = avcapture::status();
                if granted == Ok(true) && e.wanted {
                    e.start_capture();
                } else {
                    e.publish();
                }
            },
            default(Duration::from_millis(500)) => {},
        }
    }
    e.stop_capture();
    e.hub.update(|s| s.camera = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "real machine: lists cameras and reads the Camera permission (never prompts)"]
    fn live_devices_and_status() {
        eprintln!("camera access: {:?}", avcapture::status());
        let d = avcapture::devices();
        eprintln!(
            "{} camera(s): {:?}",
            d.len(),
            d.iter().map(|d| &d.name).collect::<Vec<_>>()
        );
    }
}
