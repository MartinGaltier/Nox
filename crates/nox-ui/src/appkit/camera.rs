//! Camera preview host layer. The capture session and its
//! `AVCaptureVideoPreviewLayer` belong to nox-integrations; the UI only owns
//! an empty host `CALayer` inside the island view and hands it over while the
//! camera page is visible.
//!
//! Ownership contract (agreed with integrations,
//! `nox_core::model::camera::set_preview_host`): the host layer is retained
//! by the UI for as long as it is handed out, it is touched on the main
//! thread only, and `None` is sent (detaching synchronously) before the host
//! is removed and released. Order: host then `Start`; `Stop` then `None`.

use super::island_window::IslandView;
use super::util::ns_rect;
use crate::geom::Rect;
use objc2::rc::Retained;
use objc2_quartz_core::CALayer;
use std::cell::RefCell;

thread_local! {
    static HOST: RefCell<Option<Retained<CALayer>>> = const { RefCell::new(None) };
}

/// Hands the host to the camera module. Returns true when a host is set.
fn hand_over(host: Option<&CALayer>) -> bool {
    // SAFETY (consumer side of the contract): the pointer stays valid because
    // HOST keeps the layer retained until after `set_preview_host(None)`
    // returned, and the camera module only dereferences it on the main thread.
    let ptr = host.map(|l| l as *const CALayer as usize);
    nox_core::model::camera::set_preview_host(ptr);
    ptr.is_some()
}

/// Shows/hides the host layer. Returns whether a preview is attached.
pub fn set_visible(view: &IslandView, on: bool) -> bool {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        if on {
            let Some(root) = view.layer() else {
                return false;
            };
            let host = h.get_or_insert_with(CALayer::new).clone();
            host.setMasksToBounds(true);
            host.setHidden(true);
            root.addSublayer(&host);
            hand_over(Some(&host))
        } else {
            if let Some(host) = h.take() {
                hand_over(None);
                host.removeFromSuperlayer();
            }
            false
        }
    })
}

/// Positions the host layer where the page reserved room (view coordinates).
pub fn place(rect: Option<(Rect, f64)>) {
    HOST.with(|h| {
        if let Some(host) = h.borrow().as_ref() {
            match rect {
                Some((r, radius)) => {
                    host.setFrame(ns_rect(r));
                    host.setCornerRadius(radius);
                    host.setHidden(false);
                }
                None => host.setHidden(true),
            }
        }
    })
}
