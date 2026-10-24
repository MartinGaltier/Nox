//! Native scratchpad editor (NSTextView) placed over the page's
//! `Op::TextEditor` rectangle.
//!
//! Focus rule: the island panel may become key ONLY while this editor is on
//! screen (so the user can type after clicking into it). When the editor
//! goes away the panel stops accepting key status and the keyboard is handed
//! back to the frontmost app. The text is never logged.

use super::app;
use super::util::ns_rect;
use crate::geom::Rect;
use nox_core::Command;
use nox_core::model::scratchpad::ScratchpadCommand;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplicationActivationOptions, NSColor, NSFont, NSScrollView, NSTextDelegate, NSTextView,
    NSTextViewDelegate, NSView, NSWorkspace,
};
use objc2_foundation::{NSNotification, NSString};
use std::cell::{Cell, RefCell};

thread_local! {
    static ACCEPT_KEY: Cell<bool> = const { Cell::new(false) };
    static EDITOR: RefCell<Option<Editor>> = const { RefCell::new(None) };
}

/// Read by the panel's `canBecomeKeyWindow`.
pub fn accepts_key() -> bool {
    ACCEPT_KEY.with(Cell::get)
}

struct Editor {
    scroll: Retained<NSScrollView>,
    text: Retained<NSTextView>,
    _delegate: Retained<EditorDelegate>,
    shown: bool,
}

define_class!(
    // SAFETY: NSObject subclass without ivars or Drop; main thread only.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NoxEditorDelegate"]
    struct EditorDelegate;

    unsafe impl NSObjectProtocol for EditorDelegate {}

    unsafe impl NSTextDelegate for EditorDelegate {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, n: &NSNotification) {
            let Some(obj) = n.object() else { return };
            let Some(tv) = obj.downcast_ref::<NSTextView>() else {
                return;
            };
            let text = tv.string().to_string();
            if let Some(a) = app::get() {
                // The module debounces the write (1 s after the last change).
                a.hub
                    .send(Command::Scratchpad(ScratchpadCommand::SetText(text)));
            }
        }
    }

    unsafe impl NSTextViewDelegate for EditorDelegate {}
);

fn create(mtm: MainThreadMarker) -> Editor {
    let scroll = NSTextView::scrollableTextView(mtm);
    scroll.setDrawsBackground(false);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    let doc = scroll
        .documentView()
        .expect("scrollable text view has a document");
    let text: Retained<NSTextView> = doc.downcast().expect("document view is an NSTextView");
    text.setRichText(false);
    text.setDrawsBackground(false);
    text.setTextColor(Some(&NSColor::whiteColor()));
    text.setInsertionPointColor(Some(&NSColor::whiteColor()));
    text.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    text.setAllowsUndo(true);
    // SAFETY: plain NSObject initialiser.
    let delegate: Retained<EditorDelegate> =
        unsafe { msg_send![super(EditorDelegate::alloc(mtm).set_ivars(())), init] };
    text.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    Editor {
        scroll,
        text,
        _delegate: delegate,
        shown: false,
    }
}

/// Shows the editor at `rect` (view coordinates) or hides it. `initial` is
/// loaded only when the editor appears, never while the user types.
pub fn sync(host: &NSView, rect: Option<Rect>, initial: impl FnOnce() -> String) {
    let mtm = MainThreadMarker::from(host);
    EDITOR.with(|e| {
        let mut e = e.borrow_mut();
        match rect {
            Some(r) => {
                let ed = e.get_or_insert_with(|| create(mtm));
                ed.scroll.setFrame(ns_rect(r));
                if !ed.shown {
                    ed.text.setString(&NSString::from_str(&initial()));
                    host.addSubview(&ed.scroll);
                    ed.shown = true;
                    ACCEPT_KEY.with(|k| k.set(true));
                }
            }
            None => {
                if let Some(ed) = e.as_mut()
                    && ed.shown
                {
                    ed.shown = false;
                    ed.scroll.removeFromSuperview();
                    ACCEPT_KEY.with(|k| k.set(false));
                    give_keyboard_back(host);
                }
            }
        }
    })
}

/// The panel took key status for typing: return it to the app in front
/// (Nox never activated itself, so the frontmost app is the user's).
fn give_keyboard_back(host: &NSView) {
    let Some(w) = host.window() else { return };
    if !w.isKeyWindow() {
        return;
    }
    w.resignKeyWindow();
    if let Some(front) = NSWorkspace::sharedWorkspace().frontmostApplication()
        && front.processIdentifier() != std::process::id() as i32
    {
        front.activateWithOptions(NSApplicationActivationOptions::empty());
    }
}
