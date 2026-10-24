//! In-process preference and metadata reads (no `defaults`/`mdls` process).

use objc2::rc::Retained;
use objc2_foundation::NSString;
use std::ffi::{CString, c_void};

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFPreferencesCopyAppValue(key: *const c_void, app: *const c_void) -> *mut c_void;
}

/// A string preference of another app (`CFPreferencesCopyAppValue`).
pub fn app_string(app: &str, key: &str) -> Option<String> {
    let (k, a) = (NSString::from_str(key), NSString::from_str(app));
    // SAFETY: CFStrings toll-free bridged from NSString; Copy rule result.
    let v = unsafe {
        CFPreferencesCopyAppValue(Retained::as_ptr(&k).cast(), Retained::as_ptr(&a).cast())
    };
    // SAFETY: we own the returned object (Copy rule), possibly null.
    let v = unsafe { Retained::from_raw(v as *mut objc2::runtime::AnyObject) }?;
    v.downcast_ref::<NSString>().map(|s| s.to_string())
}

/// Whether `path` carries the extended attribute `name`.
pub fn has_xattr(path: &std::path::Path, name: &str) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let (Ok(p), Ok(n)) = (
        CString::new(path.as_os_str().as_bytes()),
        CString::new(name),
    ) else {
        return false;
    };
    // SAFETY: NUL-terminated strings; a null buffer asks for the size only.
    unsafe {
        libc::getxattr(
            p.as_ptr(),
            n.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            libc::XATTR_NOFOLLOW,
        ) >= 0
    }
}
