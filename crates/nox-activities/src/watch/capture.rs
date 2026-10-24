//! Screen capture of a region via ScreenCaptureKit
//! (`SCScreenshotManager captureImageInRect:`, macOS 15.2+;
//! `CGWindowListCreateImage` is obsolete), reduced at once to a grey
//! thumbnail. Only the thumbnail leaves this module; full images are never
//! kept or written anywhere.

use super::tracker::SIDE;
use block2::RcBlock;
use nox_core::model::watch::Rect;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};
use objc2_foundation::NSError;
use objc2_screen_capture_kit::SCScreenshotManager;
use std::sync::mpsc;
use std::time::Duration;

/// Screen Recording granted (never prompts).
pub fn has_access() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Asks for Screen Recording (system prompt the first time only).
pub fn request_access() -> bool {
    CGRequestScreenCaptureAccess()
}

/// Draws `image` into a `SIDE`×`SIDE` 8-bit grey buffer.
fn thumbnail(image: &CGImage) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; SIDE * SIDE];
    let space = CGColorSpace::new_device_gray()?;
    // SAFETY: `buf` outlives the context (dropped at the end of this
    // function) and holds exactly width × bytes_per_row bytes for 8-bit grey
    // without alpha.
    let ctx = unsafe {
        CGBitmapContextCreate(
            buf.as_mut_ptr().cast(),
            SIDE,
            SIDE,
            8,
            SIDE,
            Some(&space),
            CGImageAlphaInfo::None.0,
        )
    }?;
    let r = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(SIDE as f64, SIDE as f64),
    );
    CGContext::draw_image(Some(&ctx), r, Some(image));
    drop(ctx);
    Some(buf)
}

/// Captures `region` (global points, top-left origin) as a grey thumbnail.
pub fn capture(region: Rect, timeout: Duration) -> Result<Vec<u8>, String> {
    let (tx, rx) = mpsc::sync_channel::<Result<Vec<u8>, String>>(1);
    let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let res = if image.is_null() {
            Err(if error.is_null() {
                "capture vide".to_string()
            } else {
                // SAFETY: a non-null error is a valid NSError for the
                // duration of the callback.
                unsafe { &*error }.localizedDescription().to_string()
            })
        } else {
            // SAFETY: the image is valid for the duration of the callback;
            // we only read it to draw the thumbnail.
            thumbnail(unsafe { &*image }).ok_or_else(|| "réduction impossible".into())
        };
        let _ = tx.try_send(res);
    });
    let rect = CGRect::new(
        CGPoint::new(region.x, region.y),
        CGSize::new(region.w, region.h),
    );
    // SAFETY: the block is retained by the framework until it is called;
    // `rect` is a plain value.
    unsafe { SCScreenshotManager::captureImageInRect_completionHandler(rect, Some(&block)) };
    rx.recv_timeout(timeout)
        .map_err(|_| "la capture n’a pas répondu".to_string())?
}
