//! IOKit registry, user-client and IOPowerSources bindings.

use super::cf::{CFTypeRef, Cf};
use std::ffi::{CString, c_char, c_void};

pub type IoObjectT = u32;
type KernReturn = i32;
type MachPort = u32;

const MAIN_PORT_DEFAULT: MachPort = 0;
pub const KERN_SUCCESS: KernReturn = 0;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CFTypeRef;
    fn IOServiceGetMatchingService(main: MachPort, matching: CFTypeRef) -> IoObjectT;
    fn IOServiceGetMatchingServices(
        main: MachPort,
        matching: CFTypeRef,
        iter: *mut IoObjectT,
    ) -> KernReturn;
    fn IOIteratorNext(iter: IoObjectT) -> IoObjectT;
    fn IOObjectRelease(obj: IoObjectT) -> KernReturn;
    fn IOServiceOpen(
        service: IoObjectT,
        task: MachPort,
        kind: u32,
        conn: *mut IoObjectT,
    ) -> KernReturn;
    fn IOServiceClose(conn: IoObjectT) -> KernReturn;
    fn IOConnectCallStructMethod(
        conn: IoObjectT,
        selector: u32,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
        output_size: *mut usize,
    ) -> KernReturn;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObjectT,
        key: CFTypeRef,
        alloc: CFTypeRef,
        options: u32,
    ) -> CFTypeRef;

    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFTypeRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFTypeRef;
    fn IOPSGetProvidingPowerSourceType(blob: CFTypeRef) -> CFTypeRef;
}

/// An owned io_object_t (service, iterator or connection handle's service).
pub struct IoObject(IoObjectT);

impl IoObject {
    /// First registry service whose class matches `class`.
    pub fn matching(class: &str) -> Option<Self> {
        let c = CString::new(class).ok()?;
        // SAFETY: c is NUL-terminated; IOServiceGetMatchingService consumes the
        // matching dictionary reference returned by IOServiceMatching.
        let svc = unsafe {
            let m = IOServiceMatching(c.as_ptr());
            if m.is_null() {
                return None;
            }
            IOServiceGetMatchingService(MAIN_PORT_DEFAULT, m)
        };
        // `then`, not `then_some`: never build (and drop) a wrapper around 0.
        (svc != 0).then(|| Self(svc))
    }

    /// Every registry service whose class matches `class`.
    pub fn all_matching(class: &str) -> Vec<Self> {
        let Ok(c) = CString::new(class) else {
            return vec![];
        };
        let mut iter: IoObjectT = 0;
        // SAFETY: as above; the matching dictionary is consumed; `iter` is a
        // valid out pointer.
        let kr = unsafe {
            let m = IOServiceMatching(c.as_ptr());
            if m.is_null() {
                return vec![];
            }
            IOServiceGetMatchingServices(MAIN_PORT_DEFAULT, m, &mut iter)
        };
        if kr != KERN_SUCCESS || iter == 0 {
            return vec![];
        }
        let iter = Self(iter);
        let mut out = vec![];
        loop {
            // SAFETY: iter is a live iterator; each returned object is +1 and
            // wrapped immediately so it is released exactly once.
            let o = unsafe { IOIteratorNext(iter.0) };
            if o == 0 {
                break;
            }
            out.push(Self(o));
        }
        out
    }

    /// One registry property (not the whole, possibly large, property table).
    pub fn property(&self, key: &str) -> Option<Cf> {
        let k = Cf::string(key);
        // SAFETY: self.0 is a live registry entry; Create rule on the result.
        unsafe {
            Cf::from_owned(IORegistryEntryCreateCFProperty(
                self.0,
                k.as_ptr(),
                std::ptr::null(),
                0,
            ))
        }
    }

    /// Opens a user client on this service.
    pub fn open(&self, kind: u32) -> Result<Connection, i32> {
        let mut conn: IoObjectT = 0;
        // SAFETY: self.0 is a live service; conn is a valid out pointer;
        // mach_task_self() names our own task.
        let kr = unsafe { IOServiceOpen(self.0, super::mach::mach_task_self(), kind, &mut conn) };
        if kr == KERN_SUCCESS && conn != 0 {
            Ok(Connection(conn))
        } else {
            Err(kr)
        }
    }
}

impl Drop for IoObject {
    fn drop(&mut self) {
        // SAFETY: we own one reference to this object.
        unsafe {
            IOObjectRelease(self.0);
        }
    }
}

/// An open IOKit user-client connection, closed on drop.
pub struct Connection(IoObjectT);

impl Connection {
    /// `IOConnectCallStructMethod` with a `#[repr(C)]` POD input and output.
    ///
    /// # Safety
    /// `T` must be the exact plain-old-data wire struct the user client expects
    /// for `selector` (no pointers, any bit pattern valid).
    pub unsafe fn call_struct<T: Copy>(&self, selector: u32, input: &T) -> Result<T, i32> {
        let mut out = std::mem::MaybeUninit::<T>::zeroed();
        let mut out_size = std::mem::size_of::<T>();
        // SAFETY: buffers are sized from T; T is POD per the caller contract so
        // the zeroed/kernel-filled output is a valid T.
        unsafe {
            let kr = IOConnectCallStructMethod(
                self.0,
                selector,
                (input as *const T).cast(),
                std::mem::size_of::<T>(),
                out.as_mut_ptr().cast(),
                &mut out_size,
            );
            if kr == KERN_SUCCESS {
                Ok(out.assume_init())
            } else {
                Err(kr)
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: we own this connection; closing it once is required.
        unsafe {
            IOServiceClose(self.0);
        }
    }
}

/// One IOPowerSources snapshot: providing source + each source description.
pub struct PowerSources {
    pub providing: Option<String>,
    pub descriptions: Vec<crate::value::Value>,
}

pub fn power_sources() -> Option<PowerSources> {
    // SAFETY: Copy rule on the blob and list; descriptions are borrowed from
    // the blob (Get rule) and converted while `blob` is alive.
    unsafe {
        let blob = Cf::from_owned(IOPSCopyPowerSourcesInfo())?;
        let providing = Cf::from_borrowed(IOPSGetProvidingPowerSourceType(blob.as_ptr()))
            .and_then(|s| s.to_string_lossy());
        let list = Cf::from_owned(IOPSCopyPowerSourcesList(blob.as_ptr()));
        let descriptions = list
            .map(|l| {
                l.array_items()
                    .iter()
                    .filter_map(|ps| {
                        Cf::from_borrowed(IOPSGetPowerSourceDescription(blob.as_ptr(), ps.as_ptr()))
                    })
                    .map(|d| d.to_value())
                    .collect()
            })
            .unwrap_or_default();
        Some(PowerSources {
            providing,
            descriptions,
        })
    }
}
