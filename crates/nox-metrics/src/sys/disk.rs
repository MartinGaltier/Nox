//! Mounted file systems (`getfsstat`) and volume names (`getattrlist`).

use crate::disk::Mount;
use std::ffi::{CStr, CString};

fn cstr(buf: &[libc::c_char]) -> String {
    // SAFETY: statfs strings are NUL-terminated within their fixed arrays;
    // the slice bounds the read.
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

pub fn mounts() -> Result<Vec<Mount>, String> {
    // SAFETY: null buffer = count query.
    let n = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if n < 0 {
        return Err("getfsstat".into());
    }
    let cap = n as usize + 4;
    // SAFETY: an all-zero statfs is valid.
    let mut buf: Vec<libc::statfs> = vec![unsafe { std::mem::zeroed() }; cap];
    let bytes = (cap * std::mem::size_of::<libc::statfs>()) as libc::c_int;
    // SAFETY: buf holds `cap` entries = `bytes` bytes; MNT_NOWAIT never blocks
    // on unresponsive network mounts.
    let n = unsafe { libc::getfsstat(buf.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    if n < 0 {
        return Err("getfsstat".into());
    }
    buf.truncate(n as usize);
    Ok(buf
        .iter()
        .map(|s| Mount {
            mount: cstr(&s.f_mntonname),
            from: cstr(&s.f_mntfromname),
            fs_type: cstr(&s.f_fstypename),
            flags: s.f_flags,
            block_size: u64::from(s.f_bsize),
            blocks: s.f_blocks,
            blocks_available: s.f_bavail,
        })
        .collect())
}

#[repr(C, packed(4))]
struct NameBuf {
    length: u32,
    name_ref: libc::attrreference_t,
    data: [u8; 512],
}

/// The volume's user-visible name (e.g. "Macintosh HD").
pub fn volume_name(mount: &str) -> Option<String> {
    let path = CString::new(mount).ok()?;
    // SAFETY: an all-zero attrlist is valid; fields set below.
    let mut al: libc::attrlist = unsafe { std::mem::zeroed() };
    al.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    al.volattr = libc::ATTR_VOL_INFO | libc::ATTR_VOL_NAME;
    let mut out = NameBuf {
        length: 0,
        name_ref: libc::attrreference_t {
            attr_dataoffset: 0,
            attr_length: 0,
        },
        data: [0; 512],
    };
    // SAFETY: path is NUL-terminated; `out` is a writable buffer of the given
    // size.
    let r = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&mut al as *mut libc::attrlist).cast(),
            (&mut out as *mut NameBuf).cast(),
            std::mem::size_of::<NameBuf>(),
            0,
        )
    };
    if r != 0 {
        return None;
    }
    // The offset is relative to the attrreference itself (offset 4).
    let off = out.name_ref.attr_dataoffset as usize + 4;
    let len = out.name_ref.attr_length as usize;
    let total = std::mem::size_of::<NameBuf>();
    if off < 12 || off + len > total || len == 0 {
        return None;
    }
    let data_start = off - 12;
    let slice = out.data.get(data_start..data_start + len)?;
    let s = CStr::from_bytes_until_nul(slice).ok()?;
    Some(s.to_string_lossy().into_owned())
}
