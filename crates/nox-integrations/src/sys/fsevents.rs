//! FSEvents stream delivering "something changed under these folders" on a
//! private dispatch queue. Dropping the stream stops and releases it.

use crossbeam_channel::Sender;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSString};
use std::ffi::c_void;

type FSEventStreamRef = *mut c_void;

#[repr(C)]
struct FSEventStreamContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type Callback = extern "C" fn(
    stream: FSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: Callback,
        context: *const FSEventStreamContext,
        paths: *const c_void,
        since_when: u64,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(stream: FSEventStreamRef, queue: *const DispatchQueue);
    fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
    fn FSEventStreamStop(stream: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    fn FSEventStreamRelease(stream: FSEventStreamRef);
}

const SINCE_NOW: u64 = u64::MAX;
const FLAG_NO_DEFER: u32 = 0x02;
const FLAG_FILE_EVENTS: u32 = 0x10;

extern "C" fn on_events(
    _stream: FSEventStreamRef,
    info: *mut c_void,
    _n: usize,
    _paths: *mut c_void,
    _flags: *const u32,
    _ids: *const u64,
) {
    // SAFETY: `info` is the `Box<Sender<()>>` leaked in `Stream::new`, alive
    // until after the stream is invalidated in `Drop`.
    let tx = unsafe { &*(info as *const Sender<()>) };
    let _ = tx.try_send(());
}

pub struct Stream {
    stream: FSEventStreamRef,
    info: *mut Sender<()>,
    _queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: the stream is only started/stopped from the owning thread; the
// callback runs on our private serial queue and only touches the Sender.
unsafe impl Send for Stream {}

impl Stream {
    /// Watches `folders` (recursively, as FSEvents does). Each batch of
    /// changes sends one `()` (coalesced with `latency` seconds).
    pub fn new(folders: &[String], latency: f64, tx: Sender<()>) -> Option<Self> {
        if folders.is_empty() {
            return None;
        }
        let paths: Vec<Retained<NSString>> =
            folders.iter().map(|f| NSString::from_str(f)).collect();
        let array = NSArray::from_retained_slice(&paths);
        let info = Box::into_raw(Box::new(tx));
        let ctx = FSEventStreamContext {
            version: 0,
            info: info.cast(),
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        let queue = DispatchQueue::new("io.github.martingaltier.nox.fsevents", None);
        // SAFETY: valid callback/context; NSArray<NSString> is toll-free
        // bridged to CFArrayRef of CFStringRef; FSEvents copies the paths.
        let stream = unsafe {
            FSEventStreamCreate(
                std::ptr::null(),
                on_events,
                &ctx,
                Retained::as_ptr(&array).cast(),
                SINCE_NOW,
                latency,
                FLAG_NO_DEFER | FLAG_FILE_EVENTS,
            )
        };
        if stream.is_null() {
            // SAFETY: reclaims the box leaked above (never handed out).
            drop(unsafe { Box::from_raw(info) });
            return None;
        }
        // SAFETY: `stream` is valid; the queue outlives it (owned by `Stream`).
        let started = unsafe {
            FSEventStreamSetDispatchQueue(stream, &*queue);
            FSEventStreamStart(stream) != 0
        };
        let s = Self {
            stream,
            info,
            _queue: queue,
        };
        started.then_some(s)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: stop → invalidate → release is the documented teardown.
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
        }
        // Drain callbacks already queued before freeing their context.
        self._queue.exec_sync(|| {});
        // SAFETY: no callback can run any more; reclaims the leaked box once.
        drop(unsafe { Box::from_raw(self.info) });
    }
}
