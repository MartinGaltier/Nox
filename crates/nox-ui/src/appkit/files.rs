//! File URLs from drags and open panels; opening URLs.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{ClassType, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSModalResponseOK, NSOpenPanel, NSPasteboard, NSWorkspace};
use objc2_foundation::{NSArray, NSString, NSURL};

pub fn file_paths(pb: &NSPasteboard) -> Vec<String> {
    let classes = NSArray::from_slice(&[NSURL::class()]);
    // SAFETY: NSURL is a valid readable class; the result contains NSURLs.
    let objs: Option<Retained<NSArray<AnyObject>>> =
        unsafe { pb.readObjectsForClasses_options(&classes, None) };
    objs.map(|a| {
        a.iter()
            .filter_map(|o| o.downcast::<NSURL>().ok())
            .filter(|u| u.isFileURL())
            .filter_map(|u| u.path().map(|p| p.to_string()))
            .collect()
    })
    .unwrap_or_default()
}

/// Modal open panel (explicit user action, so activating Nox is expected).
pub fn pick_files(mtm: MainThreadMarker) -> Vec<String> {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(true);
    panel.setAllowsMultipleSelection(true);
    panel.setPrompt(Some(&NSString::from_str("Ajouter à l’étagère")));
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    if panel.runModal() != NSModalResponseOK {
        return vec![];
    }
    panel
        .URLs()
        .iter()
        .filter_map(|u| u.path().map(|p| p.to_string()))
        .collect()
}

pub fn open_url(s: &str) {
    if let Some(u) = NSURL::URLWithString(&NSString::from_str(s)) {
        NSWorkspace::sharedWorkspace().openURL(&u);
    }
}
