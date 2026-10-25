//! Unsafe read-only probes, isolated here: CoreFoundation values, the
//! `IOPMrootDomain` registry entry (`SleepDisabled`), IOPowerSources (battery)
//! and libnotify (thermal pressure). Every public function is safe and returns
//! an honest `None`/`Err` when a value cannot be read, never a made-up default.
//! Nothing here needs privileges.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFIndex = isize;
pub type CFTypeID = usize;
pub type Boolean = u8;
type MachPort = u32;
type IoObject = u32;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT64_TYPE: CFIndex = 4;
const K_IO_MAIN_PORT_DEFAULT: MachPort = 0;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    fn CFBooleanGetTypeID() -> CFTypeID;
    fn CFBooleanGetValue(b: CFTypeRef) -> Boolean;
    fn CFNumberGetTypeID() -> CFTypeID;
    fn CFNumberGetValue(n: CFTypeRef, the_type: CFIndex, value_ptr: *mut c_void) -> Boolean;
    fn CFStringGetTypeID() -> CFTypeID;
    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFStringGetCString(
        s: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> Boolean;
    fn CFDictionaryGetTypeID() -> CFTypeID;
    fn CFDictionaryGetValue(d: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFArrayGetTypeID() -> CFTypeID;
    fn CFArrayGetCount(a: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(a: CFArrayRef, idx: CFIndex) -> *const c_void;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: MachPort, matching: *mut c_void) -> IoObject;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> CFTypeRef;
    fn IOObjectRelease(object: IoObject) -> c_int;
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFArrayRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFDictionaryRef;
    fn IOPSGetProvidingPowerSourceType(snapshot: CFTypeRef) -> CFStringRef;
}

// libSystem (libnotify).
unsafe extern "C" {
    fn notify_register_check(name: *const c_char, out_token: *mut c_int) -> u32;
    fn notify_get_state(token: c_int, state64: *mut u64) -> u32;
    fn notify_cancel(token: c_int) -> u32;
}

/// An owned CF object (released on drop). Never null.
pub struct CfOwned(CFTypeRef);

// SAFETY: the CF types handled here (strings, numbers, dictionaries, arrays,
// run loop sources) are immutable or internally synchronised; ownership of one
// +1 reference moves with the wrapper.
unsafe impl Send for CfOwned {}

impl CfOwned {
    /// Takes ownership of a +1 reference (Create/Copy rule). `None` if null.
    ///
    /// # Safety
    /// `ptr` must be null or a valid CF object the caller owns.
    pub unsafe fn from_create(ptr: CFTypeRef) -> Option<Self> {
        // Not `then_some(Self(ptr))`: that builds the wrapper eagerly and
        // drops it when `ptr` is null, i.e. CFRelease(NULL), which traps.
        (!ptr.is_null()).then(|| Self(ptr))
    }
    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        // SAFETY: we own exactly one reference to a non-null CF object.
        unsafe { CFRelease(self.0) }
    }
}

/// A CFString from Rust text (`None` for interior NULs).
pub fn cfstr(s: &str) -> Option<CfOwned> {
    let c = CString::new(s).ok()?;
    // SAFETY: `c` is a valid NUL-terminated UTF-8 C string for the call;
    // the result follows the Create rule.
    unsafe {
        CfOwned::from_create(CFStringCreateWithCString(
            std::ptr::null(),
            c.as_ptr(),
            K_CF_STRING_ENCODING_UTF8,
        ))
    }
}

/// Interprets a borrowed CF value as a boolean.
///
/// # Safety
/// `v` must be null or a valid CF object for the duration of the call.
unsafe fn cf_bool(v: CFTypeRef) -> Option<bool> {
    if v.is_null() {
        return None;
    }
    // SAFETY: caller guarantees `v` is a live CF object.
    unsafe {
        if CFGetTypeID(v) == CFBooleanGetTypeID() {
            Some(CFBooleanGetValue(v) != 0)
        } else if CFGetTypeID(v) == CFNumberGetTypeID() {
            cf_i64(v).map(|n| n != 0)
        } else {
            None
        }
    }
}

/// # Safety
/// `v` must be null or a valid CF object for the duration of the call.
unsafe fn cf_i64(v: CFTypeRef) -> Option<i64> {
    if v.is_null() {
        return None;
    }
    // SAFETY: caller guarantees `v` is live; we check its type first and pass
    // a correctly sized out pointer.
    unsafe {
        if CFGetTypeID(v) != CFNumberGetTypeID() {
            return None;
        }
        let mut out: i64 = 0;
        (CFNumberGetValue(v, K_CF_NUMBER_SINT64_TYPE, (&mut out as *mut i64).cast()) != 0)
            .then_some(out)
    }
}

/// # Safety
/// `v` must be null or a valid CF object for the duration of the call.
unsafe fn cf_string(v: CFTypeRef) -> Option<String> {
    if v.is_null() {
        return None;
    }
    // SAFETY: caller guarantees `v` is live; type checked; buffer is writable
    // and its size is passed.
    unsafe {
        if CFGetTypeID(v) != CFStringGetTypeID() {
            return None;
        }
        let mut buf = [0 as c_char; 256];
        if CFStringGetCString(
            v,
            buf.as_mut_ptr(),
            buf.len() as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
        ) == 0
        {
            return None;
        }
        Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

/// # Safety
/// `d` must be a valid CFDictionary for the duration of the call.
unsafe fn dict_get(d: CFDictionaryRef, key: &str) -> CFTypeRef {
    let Some(k) = cfstr(key) else {
        return std::ptr::null();
    };
    // SAFETY: `d` is a live dictionary (caller), `k` a live CFString; the
    // result is borrowed (Get rule).
    unsafe { CFDictionaryGetValue(d, k.as_ptr()) }
}

/// Reads a boolean property of the `IOPMrootDomain` registry entry (no
/// privileges needed): `SleepDisabled`, `AppleClamshellState`,
/// `AppleClamshellCausesSleep`.
pub fn root_domain_bool(key: &str) -> Result<bool, String> {
    let k = cfstr(key).ok_or("bad key")?;
    // SAFETY: IOServiceMatching takes a C string and returns a +1 dictionary
    // consumed by IOServiceGetMatchingService. The service handle is released
    // below; the property follows the Create rule and is wrapped.
    unsafe {
        let matching = IOServiceMatching(c"IOPMrootDomain".as_ptr());
        if matching.is_null() {
            return Err("IOServiceMatching failed".into());
        }
        let service = IOServiceGetMatchingService(K_IO_MAIN_PORT_DEFAULT, matching);
        if service == 0 {
            return Err("IOPMrootDomain not found".into());
        }
        let prop = CfOwned::from_create(IORegistryEntryCreateCFProperty(
            service,
            k.as_ptr(),
            std::ptr::null(),
            0,
        ));
        IOObjectRelease(service);
        let prop = prop.ok_or_else(|| format!("{key} not published"))?;
        cf_bool(prop.as_ptr()).ok_or_else(|| format!("{key} is not a boolean"))
    }
}

/// `SleepDisabled` as set by `pmset disablesleep` (powerd publishes it on the
/// root domain; see xnu IOPMrootDomain::setProperties).
pub fn sleep_disabled() -> Result<bool, String> {
    root_domain_bool("SleepDisabled")
}

/// Power source snapshot from IOPowerSources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSourceInfo {
    /// Providing source is the battery (not AC / UPS).
    pub on_battery: bool,
    /// Internal battery charge, `None` without a battery or when unreadable.
    pub battery_percent: Option<u8>,
}

impl PowerSourceInfo {
    /// On battery at or below `threshold` (0 = guard off). An unknown
    /// percentage never trips the guard.
    pub fn below(&self, threshold: u8) -> bool {
        threshold > 0 && self.on_battery && self.battery_percent.is_some_and(|p| p <= threshold)
    }
}

pub fn power_source() -> Option<PowerSourceInfo> {
    // SAFETY: the blob and list follow the Copy rule and are wrapped; the
    // descriptions and the providing type are borrowed from the blob, which
    // outlives every use below.
    unsafe {
        let blob = CfOwned::from_create(IOPSCopyPowerSourcesInfo())?;
        let providing = cf_string(IOPSGetProvidingPowerSourceType(blob.as_ptr()))?;
        let on_battery = providing == "Battery Power";
        let mut battery_percent = None;
        if let Some(list) = CfOwned::from_create(IOPSCopyPowerSourcesList(blob.as_ptr()))
            && CFGetTypeID(list.as_ptr()) == CFArrayGetTypeID()
        {
            for i in 0..CFArrayGetCount(list.as_ptr()) {
                let ps = CFArrayGetValueAtIndex(list.as_ptr(), i);
                let d = IOPSGetPowerSourceDescription(blob.as_ptr(), ps);
                if d.is_null() || CFGetTypeID(d) != CFDictionaryGetTypeID() {
                    continue;
                }
                if cf_string(dict_get(d, "Type")).as_deref() != Some("InternalBattery") {
                    continue;
                }
                let cur = cf_i64(dict_get(d, "Current Capacity"));
                let max = cf_i64(dict_get(d, "Max Capacity"));
                if let (Some(cur), Some(max)) = (cur, max)
                    && max > 0
                    && cur >= 0
                {
                    let pct = cur.saturating_mul(100).saturating_add(max / 2) / max;
                    battery_percent = Some(pct.clamp(0, 100) as u8);
                }
            }
        }
        Some(PowerSourceInfo {
            on_battery,
            battery_percent,
        })
    }
}

/// System thermal pressure level (`kOSThermalNotificationPressureLevelName`):
/// 0 nominal, 1 moderate, 2 heavy, 3 trapping, 4 sleeping.
pub fn thermal_pressure() -> Option<u64> {
    let mut token: c_int = 0;
    // SAFETY: valid C string and out pointers; the token is cancelled after use.
    unsafe {
        if notify_register_check(
            c"com.apple.system.thermalpressurelevel".as_ptr(),
            &mut token,
        ) != 0
        {
            return None;
        }
        let mut state: u64 = 0;
        let ok = notify_get_state(token, &mut state) == 0;
        notify_cancel(token);
        ok.then_some(state)
    }
}

/// Executable path of a process.
pub fn pid_path(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable and its size is passed.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(buf)))
}
