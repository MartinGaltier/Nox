//! Global shortcut ⌃⌥N toggling the island (A19). Carbon's
//! `RegisterEventHotKey` needs no Accessibility permission (unlike an event
//! tap) and costs nothing until the keys are pressed.

use std::cell::RefCell;
use std::ffi::c_void;

type OSStatus = i32;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> OSStatus;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: Handler,
        n: u32,
        types: *const EventTypeSpec,
        user: *mut c_void,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn RemoveEventHandler(h: *mut c_void) -> OSStatus;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: *mut c_void,
        options: u32,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn UnregisterEventHotKey(h: *mut c_void) -> OSStatus;
}

const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const CONTROL_KEY: u32 = 1 << 12;
const OPTION_KEY: u32 = 1 << 11;
const KEY_N: u32 = 45;

thread_local! {
    static CALLBACK: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

extern "C" fn on_hot_key(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> OSStatus {
    CALLBACK.with(|c| {
        if let Some(f) = c.borrow().as_ref() {
            f();
        }
    });
    0
}

pub struct HotKey {
    key: *mut c_void,
    handler: *mut c_void,
}

impl HotKey {
    /// Must be called on the main thread (Carbon events arrive there).
    pub fn register(f: impl Fn() + 'static) -> Option<Self> {
        CALLBACK.with(|c| *c.borrow_mut() = Some(Box::new(f)));
        let spec = EventTypeSpec {
            event_class: K_EVENT_CLASS_KEYBOARD,
            event_kind: K_EVENT_HOT_KEY_PRESSED,
        };
        let mut handler = std::ptr::null_mut();
        let mut key = std::ptr::null_mut();
        // SAFETY: valid pointers for the duration of the calls; the handler is
        // a plain extern fn and the registration is undone in Drop.
        unsafe {
            let target = GetApplicationEventTarget();
            if InstallEventHandler(
                target,
                on_hot_key,
                1,
                &spec,
                std::ptr::null_mut(),
                &mut handler,
            ) != 0
            {
                log::warn!("hot key: InstallEventHandler failed");
                return None;
            }
            let id = EventHotKeyID {
                signature: u32::from_be_bytes(*b"NOX!"),
                id: 1,
            };
            let st = RegisterEventHotKey(KEY_N, CONTROL_KEY | OPTION_KEY, id, target, 0, &mut key);
            if st != 0 {
                log::warn!("hot key ⌃⌥N unavailable (status {st}), probably taken by another app");
                RemoveEventHandler(handler);
                return None;
            }
        }
        Some(Self { key, handler })
    }
}

impl Drop for HotKey {
    fn drop(&mut self) {
        // SAFETY: both refs were returned by the successful registration.
        unsafe {
            UnregisterEventHotKey(self.key);
            RemoveEventHandler(self.handler);
        }
        CALLBACK.with(|c| *c.borrow_mut() = None);
    }
}
