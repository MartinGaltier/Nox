//! Interface byte counters (`net.link.generic.ifdata`, 64-bit) and the
//! primary interface from the SystemConfiguration dynamic store.
//!
//! `NET_RT_IFLIST2` is not used: on macOS 27 its `if_data64` byte counters
//! are truncated to 32 bits for unprivileged callers (en0 rx read 659 MB
//! while `netstat -ib` showed 4.95 GB = 659 MB + 2^32). The per-interface
//! ifmib entry returns the same 64-bit values as `netstat -ib`.

use super::cf::{CFTypeRef, Cf};
use crate::network::IfCounters;
use std::ffi::CString;

/// `struct ifmibdata` (<net/if_mib.h>).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct IfMibData {
    name: [u8; 16],
    pcount: u32,
    flags: u32,
    snd_len: u32,
    snd_maxlen: u32,
    snd_drops: u32,
    filler: [u32; 4],
    data: libc::if_data64,
}

const _: () = assert!(std::mem::size_of::<IfMibData>() == 180);

/// 64-bit byte counters of one interface.
pub fn interface_counters(name: &str) -> Result<IfCounters, String> {
    let c = CString::new(name).map_err(|e| e.to_string())?;
    // SAFETY: c is NUL-terminated.
    let index = unsafe { libc::if_nametoindex(c.as_ptr()) };
    if index == 0 {
        return Err(format!("interface {name} not found"));
    }
    let mut mib = [
        libc::CTL_NET,
        libc::PF_LINK,
        libc::NETLINK_GENERIC,
        libc::IFMIB_IFDATA,
        index as libc::c_int,
        libc::IFDATA_GENERAL,
    ];
    // SAFETY: an all-zero ifmibdata is valid.
    let mut d: IfMibData = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<IfMibData>();
    // SAFETY: `d` is a writable buffer of `len` bytes.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            6,
            (&mut d as *mut IfMibData).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 || len < std::mem::size_of::<IfMibData>() {
        return Err(format!("sysctl ifmib {name}"));
    }
    let data = d.data;
    let flags = d.flags as i32;
    Ok(IfCounters {
        name: name.to_owned(),
        up: flags & libc::IFF_UP != 0,
        loopback: flags & libc::IFF_LOOPBACK != 0,
        rx: data.ifi_ibytes,
        tx: data.ifi_obytes,
    })
}

#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    fn SCDynamicStoreCreate(
        alloc: CFTypeRef,
        name: CFTypeRef,
        callout: *const std::ffi::c_void,
        context: *const std::ffi::c_void,
    ) -> CFTypeRef;
    fn SCDynamicStoreCopyValue(store: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
}

/// A dynamic-store session without callbacks or run-loop source (pure
/// request/response to configd).
pub struct DynamicStore {
    store: Cf,
    key: Cf,
}

impl DynamicStore {
    pub fn new() -> Option<Self> {
        let name = Cf::string("Nox metrics");
        // SAFETY: no callout/context; Create rule.
        let store = unsafe {
            Cf::from_owned(SCDynamicStoreCreate(
                std::ptr::null(),
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            ))
        }?;
        Some(Self {
            store,
            key: Cf::string("State:/Network/Global/IPv4"),
        })
    }

    /// BSD name of the interface carrying the default IPv4 route.
    pub fn primary_interface(&self) -> Option<String> {
        // SAFETY: live store and key; Copy rule.
        let v = unsafe {
            Cf::from_owned(SCDynamicStoreCopyValue(
                self.store.as_ptr(),
                self.key.as_ptr(),
            ))
        }?;
        v.to_value().str("PrimaryInterface").map(str::to_owned)
    }
}
