//! NSPasteboard reads/writes for the clipboard history (E10). The module
//! calls these on the main thread (`on_main`); tests use a uniquely named
//! pasteboard and never the user's general one.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::sel;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardAccessBehavior, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardTypeURL, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    Url(String),
    Files(Vec<String>),
    Image { data: Vec<u8>, png: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Never asked (the first programmatic read shows the alert).
    Default,
    Ask,
    AlwaysAllow,
    AlwaysDeny,
}

pub struct Board(Retained<NSPasteboard>);

// SAFETY: the module only touches a Board inside main-thread closures; tests
// use a private named pasteboard on one thread.
unsafe impl Send for Board {}

impl Board {
    pub fn general() -> Self {
        Self(NSPasteboard::generalPasteboard())
    }

    /// A fresh private pasteboard (tests); call `release` when done.
    pub fn unique() -> Self {
        Self(NSPasteboard::pasteboardWithUniqueName())
    }

    pub fn release(self) {
        // SAFETY: `-[NSPasteboard releaseGlobally]` (void, no arguments)
        // frees a named pasteboard; not in the generated bindings.
        let _: () = unsafe { objc2::msg_send![&*self.0, releaseGlobally] };
    }

    pub fn change_count(&self) -> i64 {
        self.0.changeCount() as i64
    }

    /// Paste privacy setting (macOS 15.4+); older systems allow access.
    pub fn access(&self) -> Access {
        let responds: bool =
            // SAFETY: respondsToSelector: exists on every NSObject.
            unsafe { objc2::msg_send![&*self.0, respondsToSelector: sel!(accessBehavior)] };
        if !responds {
            return Access::AlwaysAllow;
        }
        match self.0.accessBehavior() {
            NSPasteboardAccessBehavior::AlwaysAllow => Access::AlwaysAllow,
            NSPasteboardAccessBehavior::AlwaysDeny => Access::AlwaysDeny,
            NSPasteboardAccessBehavior::Ask => Access::Ask,
            _ => Access::Default,
        }
    }

    /// Type identifiers of the current contents (does not read the data).
    pub fn types(&self) -> Vec<String> {
        self.0
            .types()
            .map(|t| t.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }

    /// Reads the contents (this is the access macOS may alert about).
    /// Files first, then a URL, then text, then an image (if `images`).
    /// `None` when empty, unsupported or larger than `max_bytes`.
    pub fn read(&self, max_bytes: usize, images: bool) -> Option<Content> {
        // SAFETY: immutable AppKit constants.
        let (file_t, url_t, str_t, png_t, tiff_t) = unsafe {
            (
                NSPasteboardTypeFileURL,
                NSPasteboardTypeURL,
                NSPasteboardTypeString,
                NSPasteboardTypePNG,
                NSPasteboardTypeTIFF,
            )
        };
        let items = self.0.pasteboardItems()?;
        let files: Vec<String> = items
            .iter()
            .filter_map(|it| it.stringForType(file_t))
            .filter_map(|u| NSURL::URLWithString(&u).and_then(|u| u.path()))
            .map(|p| p.to_string())
            .take(1000)
            .collect();
        if !files.is_empty() {
            return Some(Content::Files(files));
        }
        let text = self.0.stringForType(str_t).map(|s| s.to_string());
        if let Some(u) = self.0.stringForType(url_t).map(|s| s.to_string()) {
            if u.len() <= max_bytes {
                return Some(Content::Url(u));
            }
            return None;
        }
        if let Some(t) = text {
            return (!t.is_empty() && t.len() <= max_bytes).then_some(Content::Text(t));
        }
        if images {
            for (t, png) in [(png_t, true), (tiff_t, false)] {
                if let Some(d) = self.0.dataForType(t) {
                    let bytes = d.to_vec();
                    return (bytes.len() <= max_bytes)
                        .then_some(Content::Image { data: bytes, png });
                }
            }
        }
        None
    }

    /// Replaces the contents (Restore). Returns the new change count.
    pub fn write(&self, c: &Content) -> Option<i64> {
        // SAFETY: immutable AppKit constants.
        let (url_t, str_t, png_t, tiff_t) = unsafe {
            (
                NSPasteboardTypeURL,
                NSPasteboardTypeString,
                NSPasteboardTypePNG,
                NSPasteboardTypeTIFF,
            )
        };
        self.0.clearContents();
        let ok = match c {
            Content::Text(t) => self.0.setString_forType(&NSString::from_str(t), str_t),
            Content::Url(u) => {
                self.0.setString_forType(&NSString::from_str(u), url_t)
                    && self.0.setString_forType(&NSString::from_str(u), str_t)
            }
            Content::Files(paths) => {
                let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
                    .iter()
                    .map(|p| {
                        ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(
                            p,
                        )))
                    })
                    .collect();
                self.0.writeObjects(&NSArray::from_retained_slice(&urls))
            }
            Content::Image { data, png } => self.0.setData_forType(
                Some(&NSData::with_bytes(data)),
                if *png { png_t } else { tiff_t },
            ),
        };
        ok.then(|| self.change_count())
    }

    /// Tests only: puts a marker type (e.g. a password manager's concealed
    /// marker) next to the current contents.
    pub fn add_marker(&self, ty: &str) -> bool {
        self.0
            .setData_forType(Some(&NSData::with_bytes(b"")), &NSString::from_str(ty))
    }
}
