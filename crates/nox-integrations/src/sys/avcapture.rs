//! AVCaptureSession + preview layer. The session runs only while the camera
//! page is visible; layer operations happen on the main thread.

use block2::RcBlock;
use crossbeam_channel::Sender;
use nox_core::permissions::PermissionState;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDeviceInput,
    AVCaptureDevicePosition, AVCaptureDeviceTypeBuiltInWideAngleCamera,
    AVCaptureDeviceTypeContinuityCamera, AVCaptureDeviceTypeExternal, AVCaptureSession,
    AVCaptureSessionPresetHigh, AVCaptureVideoPreviewLayer, AVLayerVideoGravityResizeAspectFill,
    AVMediaTypeVideo,
};
use objc2_foundation::{NSArray, NSString};
use objc2_quartz_core::{CAAutoresizingMask, CALayer};

fn video() -> &'static NSString {
    // SAFETY: immutable AVFoundation constant, always present on macOS 14+.
    unsafe { AVMediaTypeVideo }.expect("AVMediaTypeVideo")
}

pub fn status() -> PermissionState {
    // SAFETY: class method, never prompts.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(video()) } {
        AVAuthorizationStatus::Authorized => PermissionState::Granted,
        AVAuthorizationStatus::Denied => PermissionState::Denied,
        AVAuthorizationStatus::Restricted => PermissionState::Restricted,
        _ => PermissionState::NotDetermined,
    }
}

/// Shows the system prompt (first time only); the answer arrives on `tx`.
pub fn request_access(tx: Sender<bool>) {
    let block = RcBlock::new(move |granted: Bool| {
        let _ = tx.send(granted.as_bool());
    });
    // SAFETY: the block is copied by AVFoundation and only captures a Sender.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(video(), &block) };
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// Video capture devices (no permission needed to list them).
pub fn devices() -> Vec<Device> {
    // SAFETY: immutable constants; discovery session is a plain query.
    unsafe {
        let types = NSArray::from_slice(&[
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternal,
            AVCaptureDeviceTypeContinuityCamera,
        ]);
        let ds =
            AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
                &types,
                Some(video()),
                AVCaptureDevicePosition::Unspecified,
            );
        ds.devices()
            .iter()
            .map(|d| Device {
                id: d.uniqueID().to_string(),
                name: d.localizedName().to_string(),
            })
            .collect()
    }
}

pub struct Capture {
    session: Retained<AVCaptureSession>,
    layer: Retained<AVCaptureVideoPreviewLayer>,
}

// SAFETY: AVCaptureSession is documented as thread-safe (Apple recommends
// calling start/stopRunning off the main thread). The preview layer is only
// touched in `attach`/`detach`, which callers run on the main thread.
unsafe impl Send for Capture {}
// SAFETY: see `Send`; no interior state is mutated outside those rules.
unsafe impl Sync for Capture {}

impl Capture {
    /// Builds a session for `device_id` (or the default camera). Does not
    /// start it, so the camera light stays off.
    pub fn open(device_id: Option<&str>) -> Result<Self, String> {
        // SAFETY: standard AVFoundation setup calls on fresh objects.
        unsafe {
            let device = match device_id {
                Some(id) => AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(id)),
                None => None,
            }
            .or_else(|| AVCaptureDevice::defaultDeviceWithMediaType(video()))
            .ok_or("aucune caméra")?;
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                .map_err(|e| e.localizedDescription().to_string())?;
            let session = AVCaptureSession::new();
            session.beginConfiguration();
            session.setSessionPreset(AVCaptureSessionPresetHigh);
            if !session.canAddInput(&input) {
                session.commitConfiguration();
                return Err("caméra indisponible (utilisée ailleurs ?)".into());
            }
            session.addInput(&input);
            session.commitConfiguration();
            let layer = AVCaptureVideoPreviewLayer::layerWithSession(&session);
            if let Some(g) = AVLayerVideoGravityResizeAspectFill {
                layer.setVideoGravity(g);
            }
            Ok(Self { session, layer })
        }
    }

    /// Blocking (hundreds of ms): call from a worker thread.
    pub fn start(&self) {
        // SAFETY: thread-safe session API.
        unsafe { self.session.startRunning() };
    }

    pub fn stop(&self) {
        // SAFETY: thread-safe session API; removing inputs releases the device.
        unsafe {
            self.session.stopRunning();
            self.session.beginConfiguration();
            for input in self.session.inputs().iter() {
                self.session.removeInput(&input);
            }
            self.session.commitConfiguration();
        }
    }

    pub fn is_running(&self) -> bool {
        // SAFETY: thread-safe getter.
        unsafe { self.session.isRunning() }
    }

    /// Main thread only. `host` is a live `CALayer *` retained by the UI.
    pub fn attach(&self, host: usize, mirror: bool) {
        debug_assert!(objc2::MainThreadMarker::new().is_some());
        // SAFETY: the UI guarantees `host` is a valid CALayer until it calls
        // `set_preview_host(None)`, and we only run on the main thread.
        let host: &CALayer = unsafe { &*(host as *const CALayer) };
        let layer: &CALayer = &self.layer;
        if layer.superlayer().is_some_and(|s| std::ptr::eq(&*s, host)) {
            self.set_mirror(mirror);
            return;
        }
        layer.removeFromSuperlayer();
        layer.setFrame(host.bounds());
        layer.setAutoresizingMask(
            CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
        );
        host.addSublayer(layer);
        self.set_mirror(mirror);
    }

    fn set_mirror(&self, mirror: bool) {
        // SAFETY: main thread; connection exists once the layer has a session.
        unsafe {
            if let Some(c) = self.layer.connection()
                && c.isVideoMirroringSupported()
            {
                c.setAutomaticallyAdjustsVideoMirroring(false);
                c.setVideoMirrored(mirror);
            }
        }
    }

    /// Main thread only.
    pub fn detach(&self) {
        self.layer.removeFromSuperlayer();
    }
}
