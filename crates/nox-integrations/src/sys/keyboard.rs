//! Keyboard backlight through CoreBrightness' `KeyboardBrightnessClient`
//! (private, resolved at run time, no permission) and input sources through
//! Text Input Sources (Carbon TIS, public).

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, sel};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use std::ffi::c_void;

const CORE_BRIGHTNESS: &std::ffi::CStr =
    c"/System/Library/PrivateFrameworks/CoreBrightness.framework/CoreBrightness";

fn client() -> Option<Retained<AnyObject>> {
    // SAFETY: dlopen of a system framework; the class is checked to exist
    // and to answer every selector used (encodings verified on macOS 27:
    // copyKeyboardBacklightIDs @16@0:8, brightnessForKeyboard: f24@0:8Q16,
    // setBrightness:forKeyboard: B28@0:8f16Q20, isKeyboardBuiltIn: B24@0:8Q16).
    unsafe {
        if libc::dlopen(CORE_BRIGHTNESS.as_ptr(), libc::RTLD_LAZY).is_null() {
            return None;
        }
        let cls = AnyClass::get(c"KeyboardBrightnessClient")?;
        for s in [
            sel!(copyKeyboardBacklightIDs),
            sel!(brightnessForKeyboard:),
            sel!(setBrightness:forKeyboard:),
            sel!(isKeyboardBuiltIn:),
        ] {
            if !cls.responds_to(s) {
                return None;
            }
        }
        let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
        msg_send![alloc, init]
    }
}

/// (client, keyboard id): the built-in keyboard, else the first one.
fn keyboard() -> Option<(Retained<AnyObject>, u64)> {
    let c = client()?;
    // SAFETY: selectors checked in `client`; the method follows the Copy
    // rule (`msg_send!` with a `copy…` name returns +1, managed by Retained).
    let ids: Option<Retained<NSArray<NSNumber>>> =
        unsafe { msg_send![&*c, copyKeyboardBacklightIDs] };
    let ids: Vec<u64> = ids?.iter().map(|n| n.unsignedLongLongValue()).collect();
    // SAFETY: as above.
    let builtin = ids.iter().copied().find(|&id| unsafe {
        let b: bool = msg_send![&*c, isKeyboardBuiltIn: id];
        b
    });
    let id = builtin.or_else(|| ids.first().copied())?;
    Some((c, id))
}

pub fn backlight() -> Option<f32> {
    let (c, id) = keyboard()?;
    // SAFETY: selector checked; returns float 0..1.
    let v: f32 = unsafe { msg_send![&*c, brightnessForKeyboard: id] };
    v.is_finite().then_some(v.clamp(0.0, 1.0))
}

pub fn set_backlight(v: f32) -> bool {
    let Some((c, id)) = keyboard() else {
        return false;
    };
    // SAFETY: selector checked; (float, unsigned long long) -> BOOL.
    unsafe { msg_send![&*c, setBrightness: v.clamp(0.0, 1.0), forKeyboard: id] }
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *mut c_void;
    fn TISCreateInputSourceList(properties: *const c_void, include_all: u8) -> *mut c_void;
    fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *const c_void;
    fn TISSelectInputSource(source: *mut c_void) -> i32;
    static kTISPropertyInputSourceID: *const c_void;
    static kTISPropertyLocalizedName: *const c_void;
    static kTISPropertyInputSourceCategory: *const c_void;
    static kTISPropertyInputSourceIsSelectCapable: *const c_void;
    static kTISCategoryKeyboardInputSource: *const c_void;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub id: String,
    pub name: String,
}

fn prop(src: *mut c_void, key: *const c_void) -> Option<String> {
    // SAFETY: Get rule: a CFStringRef owned by the source, valid while it is.
    let v = unsafe { TISGetInputSourceProperty(src, key) };
    // SAFETY: the requested keys are string properties.
    unsafe { (v as *const NSString).as_ref() }.map(|s| s.to_string())
}

fn source(src: *mut c_void) -> Option<Source> {
    // SAFETY: immutable TIS constants.
    let (id_key, name_key) = unsafe { (kTISPropertyInputSourceID, kTISPropertyLocalizedName) };
    Some(Source {
        id: prop(src, id_key)?,
        name: prop(src, name_key)?,
    })
}

fn selectable() -> Option<Retained<NSArray<AnyObject>>> {
    // SAFETY: TIS constants are immutable CFStrings; the filter dictionary
    // is a valid CFDictionary; the list follows the Create rule.
    unsafe {
        let keys = [
            &*(kTISPropertyInputSourceCategory as *const NSString),
            &*(kTISPropertyInputSourceIsSelectCapable as *const NSString),
        ];
        let yes = NSNumber::new_bool(true);
        let vals: [&AnyObject; 2] = [
            &*(kTISCategoryKeyboardInputSource as *const AnyObject),
            &yes,
        ];
        let filter = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &vals);
        let list = TISCreateInputSourceList(Retained::as_ptr(&filter).cast(), 0);
        Retained::from_raw(list as *mut NSArray<AnyObject>)
    }
}

/// Enabled keyboard input sources the user can switch to.
pub fn input_sources() -> Vec<Source> {
    selectable()
        .map(|l| {
            l.iter()
                .filter_map(|s| source(Retained::as_ptr(&s) as *mut c_void))
                .collect()
        })
        .unwrap_or_default()
}

pub fn current_input_source() -> Option<Source> {
    // SAFETY: Copy rule: released by Retained.
    let cur = unsafe { Retained::from_raw(TISCopyCurrentKeyboardInputSource() as *mut AnyObject) }?;
    source(Retained::as_ptr(&cur) as *mut c_void)
}

/// Switches to the enabled source with this id. Call on the main thread.
pub fn select_input_source(id: &str) -> bool {
    let Some(list) = selectable() else {
        return false;
    };
    list.iter().any(|s| {
        let p = Retained::as_ptr(&s) as *mut c_void;
        // SAFETY: `p` is a live TISInputSourceRef from the list.
        source(p).is_some_and(|src| src.id == id) && unsafe { TISSelectInputSource(p) } == 0
    })
}
