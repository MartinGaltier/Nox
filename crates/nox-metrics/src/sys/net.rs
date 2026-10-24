//! Interface byte counters (64-bit, `NET_RT_IFLIST2`) and the primary
//! interface from the SystemConfiguration dynamic store.

use super::cf::{CFTypeRef, Cf};
use crate::network::IfCounters;

pub fn interface_counters() -> Result<Vec<IfCounters>, String> {
    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
    let mut len = 0usize;
    // SAFETY: size query with a null buffer.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            6,
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 || len == 0 {
        return Err("sysctl NET_RT_IFLIST2 size".into());
    }
    // Interfaces can appear between the two calls.
    len += len / 8;
    let mut buf = vec![0u8; len];
    // SAFETY: buf has `len` bytes; the kernel writes at most `len`.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            6,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return Err("sysctl NET_RT_IFLIST2".into());
    }
    buf.truncate(len);
    let mut out = vec![];
    let mut off = 0usize;
    let hdr = std::mem::size_of::<libc::if_msghdr2>();
    while off + 4 <= buf.len() {
        let msglen = u16::from_ne_bytes([buf[off], buf[off + 1]]) as usize;
        if msglen == 0 || off + msglen > buf.len() {
            break;
        }
        let kind = buf[off + 3];
        if i32::from(kind) == libc::RTM_IFINFO2 && msglen >= hdr {
            // SAFETY: at least size_of::<if_msghdr2>() bytes are in bounds;
            // read_unaligned copes with the packed layout.
            let m: libc::if_msghdr2 =
                unsafe { std::ptr::read_unaligned(buf[off..].as_ptr().cast()) };
            let index = m.ifm_index;
            let flags = m.ifm_flags;
            let data = m.ifm_data;
            if let Some(name) = if_name(u32::from(index)) {
                out.push(IfCounters {
                    name,
                    up: flags & libc::IFF_UP != 0,
                    loopback: flags & libc::IFF_LOOPBACK != 0,
                    rx: data.ifi_ibytes,
                    tx: data.ifi_obytes,
                });
            }
        }
        off += msglen;
    }
    Ok(out)
}

fn if_name(index: u32) -> Option<String> {
    let mut buf = [0 as libc::c_char; libc::IF_NAMESIZE];
    // SAFETY: buf holds IF_NAMESIZE bytes as required.
    let p = unsafe { libc::if_indextoname(index, buf.as_mut_ptr()) };
    if p.is_null() {
        return None;
    }
    // SAFETY: if_indextoname wrote a NUL-terminated name into buf.
    let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(s.to_string_lossy().into_owned())
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
