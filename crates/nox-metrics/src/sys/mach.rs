//! Mach host statistics (CPU ticks, VM page counts) and sysctl helpers.

use std::ffi::{CString, c_void};

/// Per-CPU cumulative ticks: user, system, idle, nice.
pub type CpuTicks = [u32; 4];

pub fn cpu_ticks() -> Result<Vec<CpuTicks>, String> {
    let mut count: libc::natural_t = 0;
    let mut info: libc::processor_info_array_t = std::ptr::null_mut();
    let mut info_count: libc::mach_msg_type_number_t = 0;
    // SAFETY: out pointers are valid; on success the kernel hands us a
    // vm-allocated array of `info_count` integers that we copy then free with
    // vm_deallocate. The host port send right is released.
    unsafe {
        let host = mach_host_self();
        let kr = libc::host_processor_info(
            host,
            libc::PROCESSOR_CPU_LOAD_INFO,
            &mut count,
            &mut info,
            &mut info_count,
        );
        mach_port_deallocate(mach_task_self(), host);
        if kr != libc::KERN_SUCCESS || info.is_null() {
            return Err(format!("host_processor_info = {kr}"));
        }
        let words = std::slice::from_raw_parts(info as *const i32, info_count as usize);
        let cpus = (count as usize).min(words.len() / 4);
        let out = (0..cpus)
            .map(|i| {
                let w = &words[i * 4..i * 4 + 4];
                [w[0] as u32, w[1] as u32, w[2] as u32, w[3] as u32]
            })
            .collect();
        libc::vm_deallocate(
            mach_task_self(),
            info as libc::vm_address_t,
            info_count as libc::vm_size_t * std::mem::size_of::<i32>(),
        );
        Ok(out)
    }
}

/// `vm_statistics64` up to revision 3 (macOS 26+ appends tagged-storage
/// fields; the kernel fills the newest revision that fits and returns its
/// count, older kernels return a shorter count).
#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct VmStatistics64Rev3 {
    free_count: u32,
    active_count: u32,
    inactive_count: u32,
    wire_count: u32,
    zero_fill_count: u64,
    reactivations: u64,
    pageins: u64,
    pageouts: u64,
    faults: u64,
    cow_faults: u64,
    lookups: u64,
    hits: u64,
    purges: u64,
    purgeable_count: u32,
    speculative_count: u32,
    decompressions: u64,
    compressions: u64,
    swapins: u64,
    swapouts: u64,
    compressor_page_count: u32,
    throttled_count: u32,
    external_page_count: u32,
    internal_page_count: u32,
    total_uncompressed_pages_in_compressor: u64,
    swapped_count: u64,
    total_tag_storage_pages: u64,
    nontag_pageable_tag_storage_pages: u64,
    nontag_wired_tag_storage_pages: u64,
    free_tag_storage_pages: u64,
    tag_storing_tag_storage_pages: u64,
    total_tagged_pages: u64,
    resident_tagged_pages: u64,
    compressed_tagged_pages: u64,
    tagged_compressions: u64,
    tagged_decompressions: u64,
    compressed_tag_storage_bytes: u64,
}

const _: () = assert!(std::mem::size_of::<VmStatistics64Rev3>() == 248);
const REV1_COUNT: u32 = (std::mem::offset_of!(VmStatistics64Rev3, swapped_count) / 4) as u32;
const REV3_COUNT: u32 = (std::mem::size_of::<VmStatistics64Rev3>() / 4) as u32;
const _: () = assert!(REV1_COUNT == 38);

pub fn vm_stats() -> Result<crate::memory::VmPages, String> {
    // SAFETY: an all-zero vm_statistics64 is valid.
    let mut raw: VmStatistics64Rev3 = unsafe { std::mem::zeroed() };
    let mut count: libc::mach_msg_type_number_t = REV3_COUNT;
    // SAFETY: `raw` is large enough for `count` integer_t words; the kernel
    // writes at most that many and updates `count`.
    let kr = unsafe {
        let host = mach_host_self();
        let kr = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            (&mut raw as *mut VmStatistics64Rev3).cast(),
            &mut count,
        );
        mach_port_deallocate(mach_task_self(), host);
        kr
    };
    if kr != libc::KERN_SUCCESS {
        return Err(format!("host_statistics64 = {kr}"));
    }
    if count < REV1_COUNT {
        return Err(format!("host_statistics64 returned {count} words"));
    }
    Ok(crate::memory::VmPages {
        free: raw.free_count.into(),
        active: raw.active_count.into(),
        inactive: raw.inactive_count.into(),
        speculative: raw.speculative_count.into(),
        wired: raw.wire_count.into(),
        purgeable: raw.purgeable_count.into(),
        compressor: raw.compressor_page_count.into(),
        external: raw.external_page_count.into(),
        internal: raw.internal_page_count.into(),
        tag_storage: if count >= REV3_COUNT {
            raw.total_tag_storage_pages
        } else {
            0
        },
    })
}

/// The task port name of this process.
pub fn mach_task_self() -> libc::mach_port_t {
    // SAFETY: plain read of the process-wide task port name that libsystem
    // initialises before main and never changes.
    unsafe { mach_task_self_ }
}

unsafe extern "C" {
    static mach_task_self_: libc::mach_port_t;
    fn mach_host_self() -> libc::mach_port_t;
    fn mach_port_deallocate(task: libc::mach_port_t, name: libc::mach_port_t) -> i32;
}

/// Reads a fixed-size sysctl value by name.
fn sysctl_raw<T: Copy>(name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    // SAFETY: an all-zero T (integer / POD struct) is valid.
    let mut v: T = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<T>();
    // SAFETY: buffer and size describe `v`; no new value is written.
    let r = unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            (&mut v as *mut T).cast::<c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0 && size == std::mem::size_of::<T>()).then_some(v)
}

pub fn sysctl_u64(name: &str) -> Option<u64> {
    sysctl_raw::<u64>(name)
}

pub fn sysctl_i32(name: &str) -> Option<i32> {
    sysctl_raw::<i32>(name)
}

pub fn sysctl_string(name: &str) -> Option<String> {
    let c = CString::new(name).ok()?;
    let mut size = 0usize;
    // SAFETY: size query (null buffer).
    let r = unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 || size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size];
    // SAFETY: buf has `size` bytes.
    let r = unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return None;
    }
    buf.truncate(size);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    String::from_utf8(buf).ok()
}

/// `vm.swapusage`: (used, total) bytes.
pub fn swap_usage() -> Option<(u64, u64)> {
    sysctl_raw::<libc::xsw_usage>("vm.swapusage").map(|x| (x.xsu_used, x.xsu_total))
}

/// CPU time consumed by the calling thread so far.
pub fn thread_cpu_time() -> std::time::Duration {
    // SAFETY: an all-zero timespec is valid; clock_gettime fills it.
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    // SAFETY: valid out pointer; CLOCK_THREAD_CPUTIME_ID is supported on macOS.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    std::time::Duration::new(
        ts.tv_sec.max(0) as u64,
        ts.tv_nsec.clamp(0, 999_999_999) as u32,
    )
}
