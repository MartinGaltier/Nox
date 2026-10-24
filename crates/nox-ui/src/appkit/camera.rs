//! Camera preview host layer. The capture session and its
//! `AVCaptureVideoPreviewLayer` belong to nox-integrations; the UI only owns
//! an empty host `CALayer` inside the island view and hands it over while the
//! camera page is visible.
//!
//! Ownership contract (agreed with integrations): the host layer is retained
//! by the UI for as long as it is handed out, it is touched on the main
//! thread only, and `None` is sent before it is removed. Until the
//! integrations crate exposes its hook, no layer is attached and the page
//! says so honestly.

use super::island_window::IslandView;
use super::util::ns_rect;
use crate::geom::Rect;
use objc2::rc::Retained;
use objc2_quartz_core::CALayer;
use std::cell::RefCell;

thread_local! {
    static HOST: RefCell<Option<Retained<CALayer>>> = const { RefCell::new(None) };
}

/// Hook to the camera module, if available. Returns true when a preview
/// layer will be attached to the host.
fn hand_over(_host: Option<&CALayer>) -> bool {
    // The integrations crate has not published `set_preview_host` yet.
    false
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
