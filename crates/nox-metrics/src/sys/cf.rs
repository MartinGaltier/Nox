//! Minimal CoreFoundation bindings: owned references and conversion to
//! [`Value`].

use crate::value::Value;
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void};

pub type CFTypeRef = *const c_void;
pub type CFIndex = isize;
type CFTypeID = usize;
type Boolean = u8;

const UTF8: u32 = 0x0800_0100;
const NUMBER_SINT64: CFIndex = 4;
const NUMBER_FLOAT64: CFIndex = 6;

#[cfg(test)]
#[repr(C)]
pub struct DictionaryCallBacks {
    _private: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    fn CFStringGetTypeID() -> CFTypeID;
    fn CFNumberGetTypeID() -> CFTypeID;
    fn CFBooleanGetTypeID() -> CFTypeID;
    fn CFDictionaryGetTypeID() -> CFTypeID;
    fn CFArrayGetTypeID() -> CFTypeID;
    fn CFDataGetTypeID() -> CFTypeID;

    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: CFIndex,
        encoding: u32,
        external: Boolean,
    ) -> CFTypeRef;
    fn CFStringGetLength(s: CFTypeRef) -> CFIndex;
    fn CFStringGetMaximumSizeForEncoding(len: CFIndex, encoding: u32) -> CFIndex;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: CFIndex, enc: u32) -> Boolean;

    fn CFNumberIsFloatType(n: CFTypeRef) -> Boolean;
    fn CFNumberGetValue(n: CFTypeRef, kind: CFIndex, out: *mut c_void) -> Boolean;
    fn CFBooleanGetValue(b: CFTypeRef) -> Boolean;

    fn CFDictionaryGetCount(d: CFTypeRef) -> CFIndex;
    fn CFDictionaryGetKeysAndValues(d: CFTypeRef, keys: *mut CFTypeRef, values: *mut CFTypeRef);
    fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;

    fn CFArrayGetCount(a: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(a: CFTypeRef, i: CFIndex) -> CFTypeRef;

    fn CFDataGetLength(d: CFTypeRef) -> CFIndex;
    fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
}

/// An owned (+1) CoreFoundation reference, released on drop.
pub struct Cf(CFTypeRef);

// SAFETY: CF objects used here (strings, numbers, dictionaries, IOReport
// subscriptions) are immutable or only touched from the single owning thread;
// CFRetain/CFRelease are thread-safe.
unsafe impl Send for Cf {}

impl Cf {
    /// Takes ownership of a +1 reference (Create/Copy rule). `None` if null.
    ///
    /// # Safety
    /// `ptr` must be null or a valid CF object the caller owns.
    pub unsafe fn from_owned(ptr: CFTypeRef) -> Option<Self> {
        // Never build a `Cf` around null: dropping it would CFRelease(NULL).
        if ptr.is_null() { None } else { Some(Self(ptr)) }
    }

    /// Retains a borrowed (+0) reference (Get rule). `None` if null.
    ///
    /// # Safety
    /// `ptr` must be null or a valid CF object.
    pub unsafe fn from_borrowed(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: caller guarantees `ptr` is a live CF object.
        Some(Self(unsafe { CFRetain(ptr) }))
    }

    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }

    pub fn string(s: &str) -> Self {
        // SAFETY: bytes/len describe a valid UTF-8 buffer for the call's duration.
        let p = unsafe {
            CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as CFIndex, UTF8, 0)
        };
        // SAFETY: Create rule; a valid UTF-8 &str never fails to convert.
        unsafe { Self::from_owned(p) }.expect("CFStringCreateWithBytes")
    }

    /// Borrowed lookup in a dictionary, converted to an owned reference.
    pub fn dict_get(&self, key: &str) -> Option<Cf> {
        if !is_type(self.0, Kind::Dict) {
            return None;
        }
        let k = Cf::string(key);
        // SAFETY: self is a dictionary (checked), k a live string.
        unsafe { Cf::from_borrowed(CFDictionaryGetValue(self.0, k.0)) }
    }

    pub fn to_value(&self) -> Value {
        // SAFETY: self.0 is a live CF object for our lifetime.
        unsafe { to_value(self.0, 0) }
    }

    pub fn to_string_lossy(&self) -> Option<String> {
        // SAFETY: self.0 is a live CF object.
        unsafe { cf_string(self.0) }
    }

    /// Array elements as retained references (empty if not an array).
    pub fn array_items(&self) -> Vec<Cf> {
        if !is_type(self.0, Kind::Array) {
            return vec![];
        }
        // SAFETY: self is an array (checked); indices are in bounds.
        unsafe {
            (0..CFArrayGetCount(self.0))
                .filter_map(|i| Cf::from_borrowed(CFArrayGetValueAtIndex(self.0, i)))
                .collect()
        }
    }
}

impl Clone for Cf {
    fn clone(&self) -> Self {
        // SAFETY: self.0 is live; CFRetain returns the same object at +1.
        Self(unsafe { CFRetain(self.0) })
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: we own exactly one reference.
        unsafe { CFRelease(self.0) }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Dict,
    Array,
}

fn is_type(p: CFTypeRef, kind: Kind) -> bool {
    if p.is_null() {
        return false;
    }
    // SAFETY: p is a non-null CF object (callers hold a reference).
    unsafe {
        let id = CFGetTypeID(p);
        match kind {
            Kind::Dict => id == CFDictionaryGetTypeID(),
            Kind::Array => id == CFArrayGetTypeID(),
        }
    }
}

/// # Safety
/// `p` must be a live CFString (or any CF object; non-strings yield `None`).
unsafe fn cf_string(p: CFTypeRef) -> Option<String> {
    // SAFETY: p is a live CF object per contract.
    unsafe {
        if p.is_null() || CFGetTypeID(p) != CFStringGetTypeID() {
            return None;
        }
        let len = CFStringGetLength(p);
        let cap = CFStringGetMaximumSizeForEncoding(len, UTF8) + 1;
        let mut buf = vec![0u8; cap.max(1) as usize];
        if CFStringGetCString(p, buf.as_mut_ptr().cast(), cap, UTF8) == 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        buf.truncate(end);
        Some(String::from_utf8_lossy(&buf).into_owned())
    }
}

const MAX_DEPTH: usize = 16;

/// # Safety
/// `p` must be null or a live CF object.
unsafe fn to_value(p: CFTypeRef, depth: usize) -> Value {
    if p.is_null() || depth > MAX_DEPTH {
        return Value::Other;
    }
    // SAFETY: p is a live CF object; every accessor below is called only on
    // the matching type, with buffers sized from CF's own counts.
    unsafe {
        let id = CFGetTypeID(p);
        if id == CFNumberGetTypeID() {
            if CFNumberIsFloatType(p) != 0 {
                let mut f = 0f64;
                CFNumberGetValue(p, NUMBER_FLOAT64, (&mut f as *mut f64).cast());
                Value::Float(f)
            } else {
                let mut i = 0i64;
                CFNumberGetValue(p, NUMBER_SINT64, (&mut i as *mut i64).cast());
                Value::Int(i)
            }
        } else if id == CFBooleanGetTypeID() {
            Value::Bool(CFBooleanGetValue(p) != 0)
        } else if id == CFStringGetTypeID() {
            cf_string(p).map(Value::Str).unwrap_or(Value::Other)
        } else if id == CFDataGetTypeID() {
            let len = CFDataGetLength(p).max(0) as usize;
            let ptr = CFDataGetBytePtr(p);
            if ptr.is_null() || len == 0 {
                Value::Data(vec![])
            } else {
                Value::Data(std::slice::from_raw_parts(ptr, len).to_vec())
            }
        } else if id == CFArrayGetTypeID() {
            Value::Array(
                (0..CFArrayGetCount(p))
                    .map(|i| to_value(CFArrayGetValueAtIndex(p, i), depth + 1))
                    .collect(),
            )
        } else if id == CFDictionaryGetTypeID() {
            let n = CFDictionaryGetCount(p).max(0) as usize;
            let mut keys = vec![std::ptr::null(); n];
            let mut vals = vec![std::ptr::null(); n];
            CFDictionaryGetKeysAndValues(p, keys.as_mut_ptr(), vals.as_mut_ptr());
            let mut map = BTreeMap::new();
            for (k, v) in keys.into_iter().zip(vals) {
                if let Some(k) = cf_string(k) {
                    map.insert(k, to_value(v, depth + 1));
                }
            }
            Value::Dict(map)
        } else {
            Value::Other
        }
    }
}

#[cfg(test)]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: DictionaryCallBacks;
    static kCFTypeDictionaryValueCallBacks: DictionaryCallBacks;
    fn CFNumberCreate(alloc: CFTypeRef, kind: CFIndex, value: *const c_void) -> CFTypeRef;
    fn CFDictionaryCreateMutable(
        alloc: CFTypeRef,
        capacity: CFIndex,
        keys: *const DictionaryCallBacks,
        values: *const DictionaryCallBacks,
    ) -> CFTypeRef;
    fn CFDictionarySetValue(d: CFTypeRef, key: CFTypeRef, value: CFTypeRef);
}

/// Fixture builders for tests.
#[cfg(test)]
impl Cf {
    pub fn number(v: i64) -> Self {
        // SAFETY: `v` outlives the call; kind matches the pointee type.
        let p =
            unsafe { CFNumberCreate(std::ptr::null(), NUMBER_SINT64, (&v as *const i64).cast()) };
        // SAFETY: Create rule.
        unsafe { Self::from_owned(p) }.expect("CFNumberCreate")
    }

    pub fn mutable_dict() -> Self {
        // SAFETY: standard CF type callbacks; Create rule.
        let p = unsafe {
            CFDictionaryCreateMutable(
                std::ptr::null(),
                0,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
        };
        // SAFETY: Create rule.
        unsafe { Self::from_owned(p) }.expect("CFDictionaryCreateMutable")
    }

    /// Sets `key` in a mutable dictionary created by [`Cf::mutable_dict`].
    pub fn dict_set(&self, key: &Cf, value: &Cf) {
        // SAFETY: self is a CFMutableDictionary (constructor contract); the
        // dictionary retains key and value.
        unsafe { CFDictionarySetValue(self.0, key.0, value.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_dict() {
        let d = Cf::mutable_dict();
        d.dict_set(&Cf::string("Amperage"), &Cf::number(-199));
        d.dict_set(&Cf::string("Name"), &Cf::string("Batterie é"));
        let v = d.to_value();
        assert_eq!(v.int("Amperage"), Some(-199));
        assert_eq!(v.str("Name"), Some("Batterie é"));
        assert_eq!(
            d.dict_get("Amperage").map(|n| n.to_value()),
            Some(Value::Int(-199))
        );
        assert!(d.dict_get("nope").is_none());
    }
}
