//! MediaRemote (private framework) resolved at run time with `dlsym`.
//!
//! Every symbol is optional except `MRMediaRemoteGetNowPlayingInfo`: a missing
//! one only removes the matching field or command, it never fabricates data.
//! All `unsafe` of the adapter lives here.

use block2::{Block, RcBlock};
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSData, NSDate, NSDictionary, NSNumber, NSString};
use std::ffi::{CStr, CString, c_void};
use std::sync::mpsc;
use std::time::Duration;

const FRAMEWORK: &CStr = c"/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote";

type ObjBlock = Block<dyn Fn(*mut AnyObject)>;
type GetObj = unsafe extern "C" fn(*const DispatchQueue, *const ObjBlock);
type GetI32 = unsafe extern "C" fn(*const DispatchQueue, *const Block<dyn Fn(i32)>);
type GetBool = unsafe extern "C" fn(*const DispatchQueue, *const Block<dyn Fn(u8)>);
type Register = unsafe extern "C" fn(*const DispatchQueue);
type SendCommand = unsafe extern "C" fn(u32, *const AnyObject) -> u8;
type SetElapsed = unsafe extern "C" fn(f64);
/// `CFStringRef MRNowPlayingClientGet…(MRNowPlayingClientRef)` — Get rule (not retained).
type ClientString = unsafe extern "C" fn(*mut AnyObject) -> *mut NSString;
type ClientPid = unsafe extern "C" fn(*mut AnyObject) -> i32;

/// MediaRemote command identifiers (`MRMediaRemoteCommand`).
pub mod command {
    pub const PLAY: u32 = 0;
    pub const PAUSE: u32 = 1;
    pub const TOGGLE_PLAY_PAUSE: u32 = 2;
    pub const NEXT_TRACK: u32 = 4;
    pub const PREVIOUS_TRACK: u32 = 5;
    pub const SEEK_TO_POSITION: u32 = 24;
}

/// What the system Now Playing session reports. Extracted inside the callback
/// so no Objective-C object crosses threads.
#[derive(Debug, Default, Clone)]
pub struct Info {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub elapsed: Option<f64>,
    /// Unix seconds at which `elapsed` was sampled.
    pub timestamp: Option<f64>,
    pub rate: Option<f64>,
    pub artwork: Option<Vec<u8>>,
    pub artwork_mime: Option<String>,
    pub item_id: Option<String>,
    pub shuffle_mode: Option<i64>,
    pub repeat_mode: Option<i64>,
    pub queue_index: Option<i64>,
    pub queue_count: Option<i64>,
}

#[derive(Debug, Default, Clone)]
pub struct Client {
    pub bundle_id: Option<String>,
    pub parent_bundle_id: Option<String>,
    pub display_name: Option<String>,
    pub pid: Option<i32>,
}

pub struct MediaRemote {
    handle: *mut c_void,
    get_info: GetObj,
    get_pid: Option<GetI32>,
    get_is_playing: Option<GetBool>,
    get_client: Option<GetObj>,
    client_bundle: Option<ClientString>,
    client_parent: Option<ClientString>,
    client_name: Option<ClientString>,
    client_pid: Option<ClientPid>,
    register: Option<Register>,
    send: Option<SendCommand>,
    set_elapsed: Option<SetElapsed>,
    queue: dispatch2::DispatchRetained<DispatchQueue>,
}

// SAFETY: the handle and the resolved function pointers are immutable after
// `load`; MediaRemote's C entry points are callable from any thread and
// deliver their callbacks on the queue passed in.
unsafe impl Send for MediaRemote {}
// SAFETY: see `Send` — no interior mutability.
unsafe impl Sync for MediaRemote {}

fn sym<T: Copy>(handle: *mut c_void, name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    // SAFETY: `handle` comes from a successful `dlopen`; `c` is NUL-terminated.
    let p = unsafe { libc::dlsym(handle, c.as_ptr()) };
    if p.is_null() {
        return None;
    }
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
    // SAFETY: `T` is a function pointer type matching the documented C
    // signature of `name` (sizes checked above).
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
}

fn dict_get(d: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<Retained<AnyObject>> {
    d.objectForKey(&NSString::from_str(key))
}

fn dict_string(d: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<String> {
    let v = dict_get(d, key)?;
    let s = v.downcast_ref::<NSString>()?.to_string();
    let s = s.trim().to_owned();
    (!s.is_empty()).then_some(s)
}

fn dict_f64(d: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<f64> {
    let v = dict_get(d, key)?;
    let n = v.downcast_ref::<NSNumber>()?.doubleValue();
    n.is_finite().then_some(n)
}

fn dict_i64(d: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<i64> {
    let v = dict_get(d, key)?;
    Some(v.downcast_ref::<NSNumber>()?.longLongValue())
}

/// Reads one `NSString *` exported constant (e.g. an option key).
fn string_constant(handle: *mut c_void, name: &str) -> Option<Retained<NSString>> {
    let p: *const *mut NSString = sym::<*const *mut NSString>(handle, name)?;
    // SAFETY: the symbol is an `NSString * const` global exported by
    // MediaRemote; reading it yields a (possibly null) immortal string.
    let s = unsafe { *p };
    // SAFETY: non-null pointers are valid NSString instances (see above).
    unsafe { Retained::retain(s) }
}

const KEY: &str = "kMRMediaRemoteNowPlayingInfo";

fn info_from(d: &NSDictionary<NSString, AnyObject>) -> Info {
    let k = |s: &str| format!("{KEY}{s}");
    let artwork = dict_get(d, &k("ArtworkData"))
        .and_then(|v| v.downcast_ref::<NSData>().map(|d| d.to_vec()))
        .filter(|v| !v.is_empty());
    let timestamp = dict_get(d, &k("Timestamp")).and_then(|v| {
        v.downcast_ref::<NSDate>()
            .map(|d| d.timeIntervalSince1970())
    });
    Info {
        title: dict_string(d, &k("Title")),
        artist: dict_string(d, &k("Artist")),
        album: dict_string(d, &k("Album")),
        duration: dict_f64(d, &k("Duration")),
        elapsed: dict_f64(d, &k("ElapsedTime")),
        timestamp,
        rate: dict_f64(d, &k("PlaybackRate")),
        artwork,
        artwork_mime: dict_string(d, &k("ArtworkMIMEType")),
        item_id: dict_string(d, &k("ContentItemIdentifier")),
        shuffle_mode: dict_i64(d, &k("ShuffleMode")),
        repeat_mode: dict_i64(d, &k("RepeatMode")),
        queue_index: dict_i64(d, &k("QueueIndex")),
        queue_count: dict_i64(d, &k("TotalQueueCount")),
    }
}

impl MediaRemote {
    pub fn load() -> Result<Self, String> {
        // SAFETY: plain dlopen of a system framework path.
        let handle = unsafe { libc::dlopen(FRAMEWORK.as_ptr(), libc::RTLD_LAZY) };
        if handle.is_null() {
            return Err("MediaRemote.framework unavailable".into());
        }
        let get_info = sym::<GetObj>(handle, "MRMediaRemoteGetNowPlayingInfo")
            .ok_or("MRMediaRemoteGetNowPlayingInfo unavailable")?;
        Ok(Self {
            handle,
            get_info,
            get_pid: sym(handle, "MRMediaRemoteGetNowPlayingApplicationPID"),
            get_is_playing: sym(handle, "MRMediaRemoteGetNowPlayingApplicationIsPlaying"),
            get_client: sym(handle, "MRMediaRemoteGetNowPlayingClient"),
            client_bundle: sym(handle, "MRNowPlayingClientGetBundleIdentifier"),
            client_parent: sym(handle, "MRNowPlayingClientGetParentAppBundleIdentifier"),
            client_name: sym(handle, "MRNowPlayingClientGetDisplayName"),
            client_pid: sym(handle, "MRNowPlayingClientGetProcessIdentifier"),
            register: sym(handle, "MRMediaRemoteRegisterForNowPlayingNotifications"),
            send: sym(handle, "MRMediaRemoteSendCommand"),
            set_elapsed: sym(handle, "MRMediaRemoteSetElapsedTime"),
            queue: DispatchQueue::new("io.github.martingaltier.nox.now-playing.callbacks", None),
        })
    }

    /// Starts MediaRemote's change notifications, delivered on the main queue.
    pub fn register_for_notifications(&self) -> bool {
        match self.register {
            Some(f) => {
                // SAFETY: documented signature `void (dispatch_queue_t)`; the
                // main queue is immortal.
                unsafe { f(DispatchQueue::main()) };
                true
            }
            None => false,
        }
    }

    fn wait<T: Send + 'static>(
        &self,
        timeout: Duration,
        start: impl FnOnce(mpsc::Sender<T>),
    ) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        start(tx);
        rx.recv_timeout(timeout).ok()
    }

    /// `None` = no answer in time; `Some(None)` = nothing is playing.
    pub fn info(&self, timeout: Duration) -> Option<Option<Info>> {
        let f = self.get_info;
        self.wait(timeout, |tx| {
            let block = RcBlock::new(move |dict: *mut AnyObject| {
                let info = if dict.is_null() {
                    None
                } else {
                    // SAFETY: MediaRemote passes a CFDictionaryRef (toll-free
                    // bridged NSDictionary with string keys) valid for the call.
                    let d = unsafe { &*(dict as *const NSDictionary<NSString, AnyObject>) };
                    Some(info_from(d))
                };
                let _ = tx.send(info);
            });
            // SAFETY: `void MRMediaRemoteGetNowPlayingInfo(dispatch_queue_t,
            // void (^)(CFDictionaryRef))`; MediaRemote copies the block.
            unsafe { f(&*self.queue, &*block as *const _ as *const ObjBlock) };
        })
    }

    pub fn pid(&self, timeout: Duration) -> Option<i32> {
        let f = self.get_pid?;
        self.wait(timeout, |tx| {
            let block = RcBlock::new(move |pid: i32| {
                let _ = tx.send(pid);
            });
            // SAFETY: `void (dispatch_queue_t, void (^)(int))`.
            unsafe { f(&*self.queue, &*block) };
        })
        .filter(|&p| p > 0)
    }

    pub fn is_playing(&self, timeout: Duration) -> Option<bool> {
        let f = self.get_is_playing?;
        self.wait(timeout, |tx| {
            let block = RcBlock::new(move |b: u8| {
                let _ = tx.send(b != 0);
            });
            // SAFETY: `void (dispatch_queue_t, void (^)(Boolean))`.
            unsafe { f(&*self.queue, &*block) };
        })
    }

    pub fn client(&self, timeout: Duration) -> Option<Client> {
        let f = self.get_client?;
        let (bundle, parent, name, pid) = (
            self.client_bundle,
            self.client_parent,
            self.client_name,
            self.client_pid,
        );
        self.wait(timeout, |tx| {
            let block = RcBlock::new(move |client: *mut AnyObject| {
                if client.is_null() {
                    let _ = tx.send(Client::default());
                    return;
                }
                let read = |g: Option<ClientString>| -> Option<String> {
                    let g = g?;
                    // SAFETY: `client` is a live MRNowPlayingClientRef for the
                    // duration of the callback; the getter follows the Get rule.
                    let s = unsafe { g(client) };
                    // SAFETY: a non-null result is a valid CFString/NSString.
                    unsafe { s.as_ref() }
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty())
                };
                let c = Client {
                    bundle_id: read(bundle),
                    parent_bundle_id: read(parent),
                    display_name: read(name),
                    // SAFETY: as above, `int (MRNowPlayingClientRef)`.
                    pid: pid.map(|g| unsafe { g(client) }).filter(|&p| p > 0),
                };
                let _ = tx.send(c);
            });
            // SAFETY: `void (dispatch_queue_t, void (^)(MRNowPlayingClientRef))`.
            unsafe { f(&*self.queue, &*block as *const _ as *const ObjBlock) };
        })
    }

    /// Sends a command to the system's current Now Playing player.
    pub fn send(&self, cmd: u32) -> bool {
        let Some(f) = self.send else { return false };
        // SAFETY: `Boolean MRMediaRemoteSendCommand(MRMediaRemoteCommand, CFDictionaryRef)`;
        // a NULL options dictionary is accepted.
        unsafe { f(cmd, std::ptr::null()) != 0 }
    }

    pub fn seek(&self, position: f64) -> bool {
        if !position.is_finite() || position < 0.0 {
            return false;
        }
        if let (Some(f), Some(key)) = (
            self.send,
            string_constant(self.handle, "kMRMediaRemoteOptionPlaybackPosition"),
        ) {
            let value = NSNumber::new_f64(position);
            let options = NSDictionary::<NSString, NSNumber>::from_slices(&[&*key], &[&*value]);
            // SAFETY: as in `send`, with a valid NSDictionary as options.
            let ok = unsafe {
                f(
                    command::SEEK_TO_POSITION,
                    &*options as *const _ as *const AnyObject,
                )
            };
            if ok != 0 {
                return true;
            }
        }
        match self.set_elapsed {
            Some(f) => {
                // SAFETY: `void MRMediaRemoteSetElapsedTime(double)`.
                unsafe { f(position) };
                true
            }
            None => false,
        }
    }
}

/// Keeps NSNotificationCenter observers alive for the process lifetime.
pub struct Observers(
    #[allow(dead_code)]
    Vec<Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>>,
);

/// Observes the local notification center for each name; `on_change` runs on
/// the posting thread (MediaRemote posts on the queue given to
/// `register_for_notifications`, i.e. main).
pub fn observe(names: &[&str], on_change: impl Fn() + Clone + 'static) -> Observers {
    use objc2_foundation::{NSNotification, NSNotificationCenter};
    let center = NSNotificationCenter::defaultCenter();
    let tokens = names
        .iter()
        .map(|name| {
            let f = on_change.clone();
            let block = RcBlock::new(move |_n: std::ptr::NonNull<NSNotification>| f());
            let name = NSString::from_str(name);
            // SAFETY: no sender filter, no operation queue (synchronous on the
            // posting thread); the block only captures `'static` Rust state.
            unsafe {
                center.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block)
            }
        })
        .collect();
    Observers(tokens)
}
