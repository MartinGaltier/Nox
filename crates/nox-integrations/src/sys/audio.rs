//! CoreAudio default devices: output volume / mute, input mute, device
//! names, and change listeners (no polling).

use block2::RcBlock;
use crossbeam_channel::Sender;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2_foundation::NSString;
use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const SYSTEM_OBJECT: u32 = 1;
const DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
const DEFAULT_INPUT: u32 = fourcc(b"dIn ");
const VIRTUAL_MAIN_VOLUME: u32 = fourcc(b"vmvc");
const MUTE: u32 = fourcc(b"mute");
const NAME: u32 = fourcc(b"lnam");
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_OUTPUT: u32 = fourcc(b"outp");
const SCOPE_INPUT: u32 = fourcc(b"inpt");
const MAIN: u32 = 0;

/// `void (^)(UInt32 count, const AudioObjectPropertyAddress *addrs)`.
type ListenerBlock = block2::DynBlock<dyn Fn(u32, *const c_void)>;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectHasProperty(id: u32, addr: *const Address) -> u8;
    fn AudioObjectIsPropertySettable(id: u32, addr: *const Address, out: *mut u8) -> i32;
    fn AudioObjectGetPropertyData(
        id: u32,
        addr: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        id: u32,
        addr: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
    fn AudioObjectAddPropertyListenerBlock(
        id: u32,
        addr: *const Address,
        queue: *const DispatchQueue,
        block: *const ListenerBlock,
    ) -> i32;
    fn AudioObjectRemovePropertyListenerBlock(
        id: u32,
        addr: *const Address,
        queue: *const DispatchQueue,
        block: *const ListenerBlock,
    ) -> i32;
}

fn get<T: Copy + Default>(id: u32, addr: Address) -> Option<T> {
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `v` is a plain value of the size we pass.
    let err = unsafe {
        if AudioObjectHasProperty(id, &addr) == 0 {
            return None;
        }
        AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            (&mut v as *mut T).cast(),
        )
    };
    (err == 0).then_some(v)
}

fn set<T: Copy>(id: u32, addr: Address, v: T) -> bool {
    let mut settable = 0u8;
    // SAFETY: plain value of the declared size.
    unsafe {
        if AudioObjectHasProperty(id, &addr) == 0
            || AudioObjectIsPropertySettable(id, &addr, &mut settable) != 0
            || settable == 0
        {
            return false;
        }
        AudioObjectSetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            std::mem::size_of::<T>() as u32,
            (&v as *const T).cast(),
        ) == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Output,
    Input,
}

impl Dir {
    fn scope(self) -> u32 {
        match self {
            Dir::Output => SCOPE_OUTPUT,
            Dir::Input => SCOPE_INPUT,
        }
    }
}

pub fn default_device(dir: Dir) -> Option<u32> {
    let sel = match dir {
        Dir::Output => DEFAULT_OUTPUT,
        Dir::Input => DEFAULT_INPUT,
    };
    get::<u32>(
        SYSTEM_OBJECT,
        Address {
            selector: sel,
            scope: SCOPE_GLOBAL,
            element: MAIN,
        },
    )
    .filter(|&d| d != 0)
}

pub fn device_name(dev: u32) -> Option<String> {
    let p: *mut NSString = get::<usize>(
        dev,
        Address {
            selector: NAME,
            scope: SCOPE_GLOBAL,
            element: MAIN,
        },
    )? as *mut NSString;
    // SAFETY: the HAL returns a +1 CFStringRef (Copy semantics).
    unsafe { Retained::from_raw(p) }.map(|s| s.to_string())
}

pub fn volume(dev: u32) -> Option<f32> {
    get::<f32>(
        dev,
        Address {
            selector: VIRTUAL_MAIN_VOLUME,
            scope: SCOPE_OUTPUT,
            element: MAIN,
        },
    )
    .filter(|v| v.is_finite())
}

pub fn set_volume(dev: u32, v: f32) -> bool {
    set(
        dev,
        Address {
            selector: VIRTUAL_MAIN_VOLUME,
            scope: SCOPE_OUTPUT,
            element: MAIN,
        },
        v.clamp(0.0, 1.0),
    )
}

pub fn muted(dev: u32, dir: Dir) -> Option<bool> {
    get::<u32>(
        dev,
        Address {
            selector: MUTE,
            scope: dir.scope(),
            element: MAIN,
        },
    )
    .map(|m| m != 0)
}

pub fn set_muted(dev: u32, dir: Dir, on: bool) -> bool {
    set(
        dev,
        Address {
            selector: MUTE,
            scope: dir.scope(),
            element: MAIN,
        },
        u32::from(on),
    )
}

/// Listeners for default-device, volume and mute changes. Each change sends
/// `()`; re-create after the default device changes. Removed on drop.
pub struct Listeners {
    regs: Vec<(u32, Address)>,
    block: RcBlock<dyn Fn(u32, *const c_void)>,
    queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: the block only captures a Sender; registration handles are plain
// integers; CoreAudio listener APIs are thread-safe.
unsafe impl Send for Listeners {}

impl Listeners {
    pub fn new(tx: Sender<()>) -> Self {
        let block = RcBlock::new(move |_n: u32, _a: *const c_void| {
            let _ = tx.try_send(());
        });
        let queue = DispatchQueue::new("io.github.martingaltier.nox.audio", None);
        let mut regs = vec![
            (
                SYSTEM_OBJECT,
                Address {
                    selector: DEFAULT_OUTPUT,
                    scope: SCOPE_GLOBAL,
                    element: MAIN,
                },
            ),
            (
                SYSTEM_OBJECT,
                Address {
                    selector: DEFAULT_INPUT,
                    scope: SCOPE_GLOBAL,
                    element: MAIN,
                },
            ),
        ];
        if let Some(o) = default_device(Dir::Output) {
            regs.push((
                o,
                Address {
                    selector: VIRTUAL_MAIN_VOLUME,
                    scope: SCOPE_OUTPUT,
                    element: MAIN,
                },
            ));
            regs.push((
                o,
                Address {
                    selector: MUTE,
                    scope: SCOPE_OUTPUT,
                    element: MAIN,
                },
            ));
        }
        if let Some(i) = default_device(Dir::Input) {
            regs.push((
                i,
                Address {
                    selector: MUTE,
                    scope: SCOPE_INPUT,
                    element: MAIN,
                },
            ));
        }
        let mut ok = vec![];
        for (id, addr) in regs {
            // SAFETY: valid object id and address; the block and queue
            // outlive the registration (both owned by `Listeners`).
            let added = unsafe {
                AudioObjectHasProperty(id, &addr) != 0
                    && AudioObjectAddPropertyListenerBlock(id, &addr, &*queue, &*block) == 0
            };
            if added {
                ok.push((id, addr));
            }
        }
        Self {
            regs: ok,
            block,
            queue,
        }
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        for (id, addr) in &self.regs {
            // SAFETY: removes exactly the registrations made in `new`.
            unsafe {
                AudioObjectRemovePropertyListenerBlock(*id, addr, &*self.queue, &*self.block)
            };
        }
    }
}
