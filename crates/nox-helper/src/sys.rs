//! Unsafe FFI, isolated here: CoreFoundation, IOKit (registry, power
//! sources, sleep request), libnotify (thermal pressure) and libproc (peer
//! executable path). Every public function is safe and returns an honest
//! `None`/`Err` when a value cannot be read — never a made-up default.
//!
//! Shared with `nox-power` (read-only probes and CF helpers).

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::fd::RawFd;
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
type IoReturn = c_int;

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
    fn IOPMFindPowerManagement(main_port: MachPort) -> IoObject;
    fn IOPMSleepSystem(fb: IoObject) -> IoReturn;
    fn IOServiceClose(connect: IoObject) -> c_int;
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
        (!ptr.is_null()).then_some(Self(ptr))
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

pub fn clamshell_closed() -> Option<bool> {
    root_domain_bool("AppleClamshellState").ok()
}

pub fn clamshell_causes_sleep() -> Option<bool> {
    root_domain_bool("AppleClamshellCausesSleep").ok()
}

/// Asks the kernel to sleep now (what closing the lid would have done).
pub fn request_system_sleep() -> Result<(), String> {
    // SAFETY: IOPMFindPowerManagement returns a connection (0 on failure)
    // that we close after the call.
    unsafe {
        let fb = IOPMFindPowerManagement(K_IO_MAIN_PORT_DEFAULT);
        if fb == 0 {
            return Err("IOPMFindPowerManagement failed".into());
        }
        let r = IOPMSleepSystem(fb);
        IOServiceClose(fb);
        if r == 0 {
            Ok(())
        } else {
            Err(format!("IOPMSleepSystem 0x{:x}", r as u32))
        }
    }
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
        if let Some(list) = CfOwned::from_create(IOPSCopyPowerSourcesList(blob.as_ptr())) {
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
                    battery_percent = Some(((cur * 100 + max / 2) / max).clamp(0, 100) as u8);
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

/// Effective uid of the process on the other end of a Unix socket.
pub fn peer_euid(fd: RawFd) -> Option<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `fd` is a socket owned by the caller; out pointers are valid.
    (unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } == 0).then_some(uid)
}

/// Pid of the peer process (`LOCAL_PEERPID`).
pub fn peer_pid(fd: RawFd) -> Option<i32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `fd` is a socket owned by the caller; `pid`/`len` are valid and
    // sized for LOCAL_PEERPID's pid_t result.
    let r = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    (r == 0 && pid > 0).then_some(pid)
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

/// Effective uid of this process.
pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// `poll(2)` on readable events. Returns the `revents` per fd.
pub fn poll_readable(
    fds: &[RawFd],
    timeout: Option<std::time::Duration>,
) -> std::io::Result<Vec<bool>> {
    let mut pfds: Vec<libc::pollfd> = fds
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let ms = timeout.map_or(-1, |d| d.as_millis().min(i32::MAX as u128) as c_int);
    // SAFETY: `pfds` is a valid, correctly sized array for the call.
    let r = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, ms) };
    if r < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::Interrupted {
            return Ok(vec![false; fds.len()]);
        }
        return Err(e);
    }
    Ok(pfds
        .iter()
        .map(|p| p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0)
        .collect())
}

static STOP_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

extern "C" fn on_stop_signal(_sig: c_int) {
    let fd = STOP_FD.load(std::sync::atomic::Ordering::SeqCst);
    if fd >= 0 {
        let b = [1u8];
        // SAFETY: write(2) is async-signal-safe; `b` is a valid 1-byte buffer.
        unsafe {
            libc::write(fd, b.as_ptr().cast(), 1);
        }
    }
}

/// SIGTERM / SIGINT / SIGHUP write one byte to `fd` (self-pipe) so the poll
/// loop can restore before exiting. `fd` must stay open for the process life.
pub fn install_stop_signals(fd: RawFd) {
    STOP_FD.store(fd, std::sync::atomic::Ordering::SeqCst);
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only performs an async-signal-safe write.
        unsafe {
            libc::signal(sig, on_stop_signal as *const () as libc::sighandler_t);
        }
    }
    // A client vanishing mid-write must not kill the helper.
    // SAFETY: ignoring SIGPIPE is always valid.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Uid of the user logged in at the console (owner of `/dev/console`).
pub fn console_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/dev/console").ok().map(|m| m.uid())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Read-only probes: safe to run anywhere, but values depend on the machine.
    #[test]
    fn probes_do_not_crash() {
        eprintln!("SleepDisabled: {:?}", sleep_disabled());
        eprintln!("power source: {:?}", power_source());
        eprintln!("thermal pressure: {:?}", thermal_pressure());
        eprintln!(
            "clamshell closed: {:?}, causes sleep: {:?}",
            clamshell_closed(),
            clamshell_causes_sleep()
        );
    }

    #[test]
    fn threshold_guard() {
        let p = PowerSourceInfo {
            on_battery: true,
            battery_percent: Some(20),
        };
        assert!(p.below(20));
        assert!(!p.below(0));
        assert!(!p.below(19));
        assert!(
            !PowerSourceInfo {
                on_battery: false,
                ..p
            }
            .below(50)
        );
        assert!(
            !PowerSourceInfo {
                battery_percent: None,
                ..p
            }
            .below(50)
        );
    }

    #[test]
    fn own_peer_identity() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        use std::os::fd::AsRawFd;
        // SAFETY: getuid has no preconditions.
        let me = unsafe { libc::getuid() };
        assert_eq!(peer_euid(a.as_raw_fd()), Some(me));
        assert_eq!(peer_pid(a.as_raw_fd()), Some(std::process::id() as i32));
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert_eq!(
            pid_path(std::process::id() as i32).and_then(|p| p.canonicalize().ok()),
            Some(exe)
        );
    }
}
