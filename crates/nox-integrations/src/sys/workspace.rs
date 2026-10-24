//! NSWorkspace / NSRunningApplication / NSPasteboard / NSSharingService.
//! Everything that can show UI runs on the main thread.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSPasteboard, NSPasteboardTypeString, NSRunningApplication,
    NSSharingService, NSSharingServiceNameSendViaAirDrop, NSWorkspace,
};
use objc2_foundation::{NSArray, NSString, NSURL};

fn file_url(path: &str) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(path))
}

/// Opens a URL (http(s), x-apple.systempreferences:, facetime:, zoommtg:…).
pub fn open_url(url: &str) -> bool {
    let url = url.to_owned();
    super::on_main(super::MAIN_TIMEOUT, move || {
        NSURL::URLWithString(&NSString::from_str(&url))
            .is_some_and(|u| NSWorkspace::sharedWorkspace().openURL(&u))
    })
    .unwrap_or(false)
}

/// Opens a file or folder with its default application.
pub fn open_path(path: &str) -> bool {
    let path = path.to_owned();
    super::on_main(super::MAIN_TIMEOUT, move || {
        NSWorkspace::sharedWorkspace().openURL(&file_url(&path))
    })
    .unwrap_or(false)
}

/// Selects the given items in Finder.
pub fn reveal(paths: &[String]) -> bool {
    if paths.is_empty() {
        return false;
    }
    let paths = paths.to_vec();
    super::on_main(super::MAIN_TIMEOUT, move || {
        let urls: Vec<Retained<NSURL>> = paths.iter().map(|p| file_url(p)).collect();
        let array = NSArray::from_retained_slice(&urls);
        NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&array);
        true
    })
    .unwrap_or(false)
}

/// Brings a running app to the front (by pid), else launches it by bundle id.
pub fn activate_app(pid: Option<i32>, bundle_id: Option<&str>) -> bool {
    let bundle_id = bundle_id.map(str::to_owned);
    super::on_main(super::MAIN_TIMEOUT, move || {
        if let Some(app) =
            pid.and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
            && app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows)
        {
            return true;
        }
        let Some(bundle) = bundle_id else {
            return false;
        };
        let ws = NSWorkspace::sharedWorkspace();
        match ws.URLForApplicationWithBundleIdentifier(&NSString::from_str(&bundle)) {
            Some(url) => ws.openURL(&url),
            None => false,
        }
    })
    .unwrap_or(false)
}

/// Launches/activates an app by name or bundle id ("Safari", "com.apple.Safari")
/// or by absolute `.app` path.
pub fn open_app(name_or_id: &str) -> bool {
    if name_or_id.starts_with('/') {
        return open_path(name_or_id);
    }
    if name_or_id.contains('.') && activate_app(None, Some(name_or_id)) {
        return true;
    }
    for dir in [
        "/Applications",
        "/System/Applications",
        "/System/Applications/Utilities",
    ] {
        let p = format!("{dir}/{name_or_id}.app");
        if std::path::Path::new(&p).exists() {
            return open_path(&p);
        }
    }
    false
}

pub fn is_running(bundle_id: &str) -> bool {
    let id = NSString::from_str(bundle_id);
    !NSRunningApplication::runningApplicationsWithBundleIdentifier(&id).is_empty()
}

/// Display name of a running app.
pub fn app_name(pid: i32) -> Option<String> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|a| a.localizedName())
        .map(|s| s.to_string())
}

pub fn copy_text(text: &str) -> bool {
    let text = text.to_owned();
    super::on_main(super::MAIN_TIMEOUT, move || {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        // SAFETY: `NSPasteboardTypeString` is an immutable AppKit constant.
        let ty = unsafe { NSPasteboardTypeString };
        pb.setString_forType(&NSString::from_str(&text), ty)
    })
    .unwrap_or(false)
}

/// Opens the AirDrop sheet for the given files. `false` when AirDrop is
/// unavailable (Wi-Fi/Bluetooth off, or the items are not shareable).
pub fn airdrop(paths: &[String]) -> bool {
    let paths = paths.to_vec();
    super::on_main(super::MAIN_TIMEOUT, move || {
        // SAFETY: immutable AppKit constant.
        let name = unsafe { NSSharingServiceNameSendViaAirDrop };
        let Some(service) = NSSharingService::sharingServiceNamed(name) else {
            return false;
        };
        let urls: Vec<Retained<AnyObject>> = paths
            .iter()
            .map(|p| Retained::into_super(Retained::into_super(file_url(p))))
            .collect();
        let items = NSArray::from_retained_slice(&urls);
        // SAFETY: main thread; items are NSURLs, which AirDrop accepts.
        unsafe {
            if !service.canPerformWithItems(Some(&items)) {
                return false;
            }
            service.performWithItems(&items);
        }
        true
    })
    .unwrap_or(false)
}
