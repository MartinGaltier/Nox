//! Core Audio HAL: property reads, audio process objects, process taps,
//! private aggregate devices and IOProcs. Every function is safe; the
//! `unsafe` blocks each carry their justification.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProc, AudioDeviceIOProcID,
    AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
    AudioObjectID, AudioObjectPropertyAddress, CATapDescription, CATapMuteBehavior,
};
use objc2_core_audio_types::AudioStreamBasicDescription;
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSMutableDictionary, NSNumber, NSString};
use std::ffi::c_void;
use std::ptr::NonNull;

pub type ObjectId = AudioObjectID;
pub const SYSTEM_OBJECT: ObjectId = 1;

const fn cc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

pub const SCOPE_GLOBAL: u32 = cc(b"glob");
pub const SCOPE_INPUT: u32 = cc(b"inpt");
pub const SCOPE_OUTPUT: u32 = cc(b"outp");

pub mod sel {
    use super::cc;
    pub const DEFAULT_OUTPUT_DEVICE: u32 = cc(b"dOut");
    pub const PROCESS_OBJECT_LIST: u32 = cc(b"prs#");
    pub const TRANSLATE_PID: u32 = cc(b"id2p");
    pub const DEVICE_UID: u32 = cc(b"uid ");
    pub const NAME: u32 = cc(b"lnam");
    pub const NOMINAL_SAMPLE_RATE: u32 = cc(b"nsrt");
    pub const STREAM_CONFIGURATION: u32 = cc(b"slay");
    pub const PROCESS_PID: u32 = cc(b"ppid");
    pub const PROCESS_BUNDLE_ID: u32 = cc(b"pbid");
    pub const PROCESS_RUNNING_OUTPUT: u32 = cc(b"piro");
    pub const TAP_FORMAT: u32 = cc(b"tfmt");
}

/// Core Audio status (`OSStatus`), e.g. `'!obj'` or a TCC refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub i32);

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let b = self.0.to_be_bytes();
        if b.iter().all(|c| c.is_ascii_graphic()) {
            write!(f, "OSStatus '{}'", String::from_utf8_lossy(&b))
        } else {
            write!(f, "OSStatus {}", self.0)
        }
    }
}

fn check(s: i32) -> Result<(), Status> {
    if s == 0 { Ok(()) } else { Err(Status(s)) }
}

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: 0,
    }
}

/// `AudioStreamBasicDescription` with a zero default.
#[derive(Clone, Copy)]
struct Asbd(AudioStreamBasicDescription);

impl Default for Asbd {
    fn default() -> Self {
        Self(AudioStreamBasicDescription {
            mSampleRate: 0.0,
            mFormatID: 0,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: 0,
            mBytesPerFrame: 0,
            mChannelsPerFrame: 0,
            mBitsPerChannel: 0,
            mReserved: 0,
        })
    }
}

/// Reads a fixed-size POD property.
fn get<T: Copy + Default>(obj: ObjectId, selector: u32, scope: u32) -> Result<T, Status> {
    let mut a = address(selector, scope);
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `v` is a valid, writable T of `size` bytes; T is POD (Copy) and
    // every bit pattern the HAL writes for these selectors is a valid T.
    check(unsafe {
        AudioObjectGetPropertyData(
            obj,
            NonNull::from(&mut a),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut v).cast(),
        )
    })?;
    Ok(v)
}

/// Reads a variable-length array property of POD elements.
fn get_vec<T: Copy + Default>(obj: ObjectId, selector: u32, scope: u32) -> Result<Vec<T>, Status> {
    let mut a = address(selector, scope);
    let mut size = 0u32;
    // SAFETY: size query on a valid address.
    check(unsafe {
        AudioObjectGetPropertyDataSize(
            obj,
            NonNull::from(&mut a),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    })?;
    let n = size as usize / std::mem::size_of::<T>().max(1);
    let mut v = vec![T::default(); n];
    if n == 0 {
        return Ok(v);
    }
    // SAFETY: `v` holds `size` writable bytes.
    check(unsafe {
        AudioObjectGetPropertyData(
            obj,
            NonNull::from(&mut a),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(v.as_mut_ptr().cast()).expect("non-empty vec"),
        )
    })?;
    v.truncate(size as usize / std::mem::size_of::<T>().max(1));
    Ok(v)
}

/// Reads a CFString property (returned at +1).
fn get_string(obj: ObjectId, selector: u32) -> Option<String> {
    let mut a = address(selector, SCOPE_GLOBAL);
    let mut p: *mut c_void = std::ptr::null_mut();
    let mut size = std::mem::size_of::<*mut c_void>() as u32;
    // SAFETY: out buffer is one pointer; the HAL writes a retained CFStringRef.
    let s = unsafe {
        AudioObjectGetPropertyData(
            obj,
            NonNull::from(&mut a),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut p).cast(),
        )
    };
    if s != 0 || p.is_null() {
        return None;
    }
    // SAFETY: CFString is toll-free bridged to NSString; we own the +1.
    let ns = unsafe { Retained::<NSString>::from_raw(p.cast()) }?;
    Some(ns.to_string())
}

pub fn default_output_device() -> Option<ObjectId> {
    get::<ObjectId>(SYSTEM_OBJECT, sel::DEFAULT_OUTPUT_DEVICE, SCOPE_GLOBAL)
        .ok()
        .filter(|&d| d != 0)
}

pub fn device_uid(dev: ObjectId) -> Option<String> {
    get_string(dev, sel::DEVICE_UID)
}

pub fn object_name(obj: ObjectId) -> Option<String> {
    get_string(obj, sel::NAME)
}

pub fn nominal_sample_rate(dev: ObjectId) -> Option<f64> {
    get::<f64>(dev, sel::NOMINAL_SAMPLE_RATE, SCOPE_GLOBAL)
        .ok()
        .filter(|r| *r > 0.0)
}

/// Total channels of a device in one scope (from its stream configuration).
pub fn channel_count(dev: ObjectId, scope: u32) -> u32 {
    // The property is a variable-length AudioBufferList; read it as words.
    let Ok(words) = get_vec::<u32>(dev, sel::STREAM_CONFIGURATION, scope) else {
        return 0;
    };
    // Layout: mNumberBuffers (u32), then per buffer {mNumberChannels u32,
    // mDataByteSize u32, mData *mut c_void (8 bytes, 8-aligned)}: the first
    // buffer starts at byte 8 on 64-bit.
    let Some(&n) = words.first() else { return 0 };
    let mut total = 0u32;
    for b in 0..n as usize {
        let idx = 2 + b * 4; // 16 bytes per AudioBuffer = 4 words
        match words.get(idx) {
            Some(&ch) => total = total.saturating_add(ch),
            None => break,
        }
    }
    total
}

/// Every audio process object known to the HAL (macOS 14.2+).
pub fn process_objects() -> Vec<ObjectId> {
    get_vec(SYSTEM_OBJECT, sel::PROCESS_OBJECT_LIST, SCOPE_GLOBAL).unwrap_or_default()
}

pub fn process_pid(obj: ObjectId) -> Option<i32> {
    get::<i32>(obj, sel::PROCESS_PID, SCOPE_GLOBAL)
        .ok()
        .filter(|&p| p > 0)
}

pub fn process_bundle_id(obj: ObjectId) -> Option<String> {
    get_string(obj, sel::PROCESS_BUNDLE_ID).filter(|s| !s.is_empty())
}

pub fn process_running_output(obj: ObjectId) -> bool {
    get::<u32>(obj, sel::PROCESS_RUNNING_OUTPUT, SCOPE_GLOBAL).is_ok_and(|v| v != 0)
}

/// The process object of `pid` (exists even before it plays).
pub fn translate_pid(pid: i32) -> Option<ObjectId> {
    let mut a = address(sel::TRANSLATE_PID, SCOPE_GLOBAL);
    let mut obj: ObjectId = 0;
    let mut size = std::mem::size_of::<ObjectId>() as u32;
    // SAFETY: qualifier is the pid (4 bytes); out is one object id.
    let s = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            NonNull::from(&mut a),
            std::mem::size_of::<i32>() as u32,
            (&pid as *const i32).cast(),
            NonNull::from(&mut size),
            NonNull::from(&mut obj).cast(),
        )
    };
    (s == 0 && obj != 0).then_some(obj)
}

/// Creates a private stereo-mixdown tap of `processes`.
///
/// **Shows the system "System Audio Recording" prompt the first time.** With
/// the permission refused the tap is still created but only delivers
/// silence (there is no public way to ask the TCC state).
pub fn create_tap(processes: &[ObjectId], muted: bool, name: &str) -> Result<Tap, Status> {
    let numbers: Vec<Retained<NSNumber>> =
        processes.iter().map(|&p| NSNumber::new_u32(p)).collect();
    let array = NSArray::from_retained_slice(&numbers);
    // SAFETY: designated initializer with an NSArray<NSNumber> of process
    // object ids, as documented for CATapDescription (macOS 14.2+).
    let desc = unsafe {
        CATapDescription::initStereoMixdownOfProcesses(CATapDescription::alloc(), &array)
    };
    // SAFETY: plain property setters on a live description.
    unsafe {
        desc.setName(&NSString::from_str(name));
        desc.setPrivate(true);
        desc.setMuteBehavior(if muted {
            CATapMuteBehavior::Muted
        } else {
            CATapMuteBehavior::Unmuted
        });
    }
    let mut id: ObjectId = 0;
    // SAFETY: valid description and out pointer.
    check(unsafe { AudioHardwareCreateProcessTap(Some(&desc), &mut id) })?;
    if id == 0 {
        return Err(Status(-1));
    }
    // SAFETY: getter on a live description.
    let uuid = unsafe { desc.UUID() }.UUIDString().to_string();
    let channels = get::<Asbd>(id, sel::TAP_FORMAT, SCOPE_GLOBAL)
        .map(|f| f.0.mChannelsPerFrame)
        .unwrap_or(2)
        .max(1);
    Ok(Tap { id, uuid, channels })
}

/// An owned process tap, destroyed on drop.
#[derive(Debug)]
pub struct Tap {
    pub id: ObjectId,
    pub uuid: String,
    pub channels: u32,
}

impl Drop for Tap {
    fn drop(&mut self) {
        // SAFETY: we created this tap and destroy it once.
        let s = unsafe { AudioHardwareDestroyProcessTap(self.id) };
        if s != 0 {
            log::debug!("destroy tap {}: {}", self.id, Status(s));
        }
    }
}

/// A private aggregate device on `main_uid` (and optionally a tap), destroyed
/// on drop. Private = invisible to other apps and gone if Nox dies.
#[derive(Debug)]
pub struct Aggregate {
    pub id: ObjectId,
}

pub fn create_aggregate(
    name: &str,
    main_uid: &str,
    tap_uuid: Option<&str>,
) -> Result<Aggregate, Status> {
    let key = |k: &std::ffi::CStr| NSString::from_str(k.to_str().unwrap_or_default());
    let s = |v: &str| -> Retained<AnyObject> { NSString::from_str(v).into() };
    let n = |v: u32| -> Retained<AnyObject> { NSNumber::new_u32(v).into() };
    let dict = |pairs: Vec<(Retained<NSString>, Retained<AnyObject>)>| {
        let d = NSMutableDictionary::<NSString, AnyObject>::new();
        for (k, v) in pairs {
            // SAFETY: NSString keys and NSObject values; copied/retained by the dictionary.
            unsafe { d.setObject_forKey(&v, objc2::runtime::ProtocolObject::from_ref(&*k)) };
        }
        d
    };
    use objc2_core_audio as ca;
    let uid = format!("io.github.martingaltier.nox.audio.{}", uuid_like());
    let sub = dict(vec![(key(ca::kAudioSubDeviceUIDKey), s(main_uid))]);
    let mut pairs = vec![
        (key(ca::kAudioAggregateDeviceNameKey), s(name)),
        (key(ca::kAudioAggregateDeviceUIDKey), s(&uid)),
        (key(ca::kAudioAggregateDeviceIsPrivateKey), n(1)),
        (key(ca::kAudioAggregateDeviceMainSubDeviceKey), s(main_uid)),
        (
            key(ca::kAudioAggregateDeviceSubDeviceListKey),
            NSArray::from_retained_slice(&[Retained::<AnyObject>::from(sub)]).into(),
        ),
    ];
    if let Some(t) = tap_uuid {
        let tap = dict(vec![
            (key(ca::kAudioSubTapUIDKey), s(t)),
            (key(ca::kAudioSubTapDriftCompensationKey), n(1)),
        ]);
        pairs.push((
            key(ca::kAudioAggregateDeviceTapListKey),
            NSArray::from_retained_slice(&[Retained::<AnyObject>::from(tap)]).into(),
        ));
        pairs.push((key(ca::kAudioAggregateDeviceTapAutoStartKey), n(1)));
    }
    let d = dict(pairs);
    let mut id: ObjectId = 0;
    // SAFETY: NSDictionary is toll-free bridged to CFDictionary; it outlives
    // the call; `id` is a valid out pointer.
    check(unsafe {
        let cf: &CFDictionary = &*(Retained::as_ptr(&d) as *const CFDictionary);
        AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut id))
    })?;
    if id == 0 {
        return Err(Status(-1));
    }
    Ok(Aggregate { id })
}

impl Drop for Aggregate {
    fn drop(&mut self) {
        // SAFETY: we created this aggregate and destroy it once.
        let s = unsafe { AudioHardwareDestroyAggregateDevice(self.id) };
        if s != 0 {
            log::debug!("destroy aggregate {}: {}", self.id, Status(s));
        }
    }
}

fn uuid_like() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{t:x}", std::process::id())
}

/// The raw IOProc signature (input/output buffer lists may be empty).
pub type IoProcFn = unsafe extern "C-unwind" fn(
    ObjectId,
    NonNull<objc2_core_audio_types::AudioTimeStamp>,
    NonNull<objc2_core_audio_types::AudioBufferList>,
    NonNull<objc2_core_audio_types::AudioTimeStamp>,
    NonNull<objc2_core_audio_types::AudioBufferList>,
    NonNull<objc2_core_audio_types::AudioTimeStamp>,
    *mut c_void,
) -> i32;

/// A started IOProc on a device. Stopped and destroyed on drop, before the
/// client data it points to may be freed.
pub struct IoProc {
    device: ObjectId,
    id: AudioDeviceIOProcID,
}

// SAFETY: the IOProc id is an opaque handle usable from any thread.
unsafe impl Send for IoProc {}

impl IoProc {
    /// # Safety
    /// `client` must stay valid until the returned `IoProc` is dropped, and
    /// `f` must be real-time safe and only use `client` as its own type.
    pub unsafe fn start(
        device: ObjectId,
        f: IoProcFn,
        client: *mut c_void,
    ) -> Result<Self, Status> {
        let mut id: AudioDeviceIOProcID = None;
        let proc_: AudioDeviceIOProc = Some(f);
        // SAFETY: caller contract on `client`; out pointer is valid.
        check(unsafe { AudioDeviceCreateIOProcID(device, proc_, client, NonNull::from(&mut id)) })?;
        let this = Self { device, id };
        // SAFETY: id created above for this device.
        check(unsafe { AudioDeviceStart(device, this.id) })?;
        Ok(this)
    }
}

impl Drop for IoProc {
    fn drop(&mut self) {
        // SAFETY: stop then destroy our own IOProc once; AudioDeviceStop
        // returns after the current cycle, so no callback runs afterwards.
        unsafe {
            AudioDeviceStop(self.device, self.id);
            AudioDeviceDestroyIOProcID(self.device, self.id);
        }
    }
}

/// Raw buffer view used by IOProcs (no allocation).
///
/// # Safety
/// `list` must point to a valid AudioBufferList for the duration of use.
pub unsafe fn buffers<'a>(
    list: NonNull<objc2_core_audio_types::AudioBufferList>,
) -> &'a mut [objc2_core_audio_types::AudioBuffer] {
    // SAFETY: AudioBufferList is a u32 count followed by that many
    // AudioBuffers (declared as a 1-element array).
    unsafe {
        let l = list.as_ptr();
        let n = (*l).mNumberBuffers as usize;
        std::slice::from_raw_parts_mut((&raw mut (*l).mBuffers).cast(), n)
    }
}
