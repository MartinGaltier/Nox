// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/NowPlayingAdapter/NowPlayingSelection.swift, commit e916961.

//! Per-player MediaRemote access: every registered Now Playing client, its
//! own info and playback state, and commands addressed to one player
//! (`MRPlayerPath`) instead of the system's current one. All symbols are
//! optional: without them the adapter keeps following the system player.

use super::{Client, ClientString, Info, MediaRemote, ObjBlock, info_from, string_constant, sym};
use block2::{Block, RcBlock};
use dispatch2::DispatchQueue;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString};
use std::ffi::c_void;
use std::sync::mpsc;
use std::time::Duration;

pub(super) type GetClients = unsafe extern "C" fn(*const DispatchQueue, *const ObjBlock);
type InfoBlock = Block<dyn Fn(*mut AnyObject, *const c_void)>;
pub(super) type InfoForPlayer =
    unsafe extern "C" fn(*mut AnyObject, bool, *const DispatchQueue, *const InfoBlock);
pub(super) type StateForPlayer =
    unsafe extern "C" fn(*mut AnyObject, *const DispatchQueue, *const Block<dyn Fn(u32)>);
type SendBlock = Block<dyn Fn(u32, *mut AnyObject)>;
pub(super) type SendToPlayer = unsafe extern "C" fn(
    u32,
    *const AnyObject,
    *mut AnyObject,
    u32,
    *const DispatchQueue,
    *const SendBlock,
) -> u8;
pub(super) type CopyArtwork = unsafe extern "C" fn(*const c_void) -> *mut NSData;

/// One registered Now Playing client and the player path addressing it.
pub struct Player {
    pub client: Client,
    path: Retained<AnyObject>,
}

// SAFETY: the MRPlayerPath is never mutated after `make_path`; MediaRemote
// functions taking it are callable from any thread.
unsafe impl Send for Player {}

pub(super) struct PlayerFns {
    pub get_clients: Option<GetClients>,
    pub info_for: Option<InfoForPlayer>,
    pub state_for: Option<StateForPlayer>,
    pub send_to: Option<SendToPlayer>,
    pub copy_artwork: Option<CopyArtwork>,
}

impl PlayerFns {
    pub fn load(handle: *mut c_void) -> Self {
        Self {
            get_clients: sym(handle, "MRMediaRemoteGetNowPlayingClients"),
            info_for: sym(handle, "MRMediaRemoteGetNowPlayingInfoForPlayer"),
            state_for: sym(handle, "MRMediaRemoteGetPlaybackStateForPlayer"),
            send_to: sym(handle, "MRMediaRemoteSendCommandToPlayer"),
            copy_artwork: sym(handle, "MRNowPlayingArtworkCopyImageData"),
        }
    }
}

/// Whether a process exists (signal 0 only checks).
fn alive(pid: i32) -> bool {
    // SAFETY: kill with signal 0 sends nothing.
    unsafe { libc::kill(pid, 0) == 0 || *libc::__error() == libc::EPERM }
}

fn responds(obj: &AnyObject, s: Sel) -> bool {
    // SAFETY: `respondsToSelector:` is defined on every NSObject.
    unsafe { msg_send![obj, respondsToSelector: s] }
}

/// A private copy of the local player path pointing at one client (by
/// bundle id + pid). The shared `localPlayerPath` itself is never mutated.
fn make_path(bundle: &str, pid: i32) -> Option<Retained<AnyObject>> {
    let path_cls = AnyClass::get(c"MRPlayerPath")?;
    let client_cls = AnyClass::get(c"MRClient")?;
    // SAFETY: each selector is checked with respondsToSelector: before use;
    // argument types follow the class's (private) declarations, the same
    // calls Vorssaint makes on macOS 27.
    unsafe {
        let local: Option<Retained<AnyObject>> = msg_send![path_cls, localPlayerPath];
        let local = local?;
        let path: Option<Retained<AnyObject>> = msg_send![&*local, copy];
        let path = path?;
        let alloc: Allocated<AnyObject> = msg_send![client_cls, alloc];
        let ns = NSString::from_str(bundle);
        let client: Option<Retained<AnyObject>> = msg_send![alloc, initWithBundleIdentifier: &*ns];
        let client = client?;
        if !responds(&client, sel!(setProcessIdentifier:))
            || !responds(&path, sel!(setClient:))
            || !responds(&path, sel!(setPlayer:))
        {
            return None;
        }
        let _: () = msg_send![&*client, setProcessIdentifier: pid];
        let _: () = msg_send![&*path, setClient: &*client];
        // nil = this app's active player ("default" is a different one).
        let _: () = msg_send![&*path, setPlayer: std::ptr::null::<AnyObject>()];
        Some(path)
    }
}

fn read_client(
    c: *mut AnyObject,
    getters: [Option<ClientString>; 3],
    pid: Option<super::ClientPid>,
) -> Client {
    let read = |g: Option<ClientString>| -> Option<String> {
        let g = g?;
        // SAFETY: `c` is a live MRNowPlayingClientRef during the callback.
        let s = unsafe { g(c) };
        // SAFETY: a non-null result is a valid CFString (Get rule).
        unsafe { s.as_ref() }
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };
    Client {
        bundle_id: read(getters[0]),
        parent_bundle_id: read(getters[1]),
        display_name: read(getters[2]),
        // SAFETY: as above, `int (MRNowPlayingClientRef)`.
        pid: pid.map(|g| unsafe { g(c) }).filter(|&p| p > 0),
    }
}

impl MediaRemote {
    /// Registered Now Playing clients (at most 16). `None` when the API is
    /// missing or did not answer in time.
    pub fn players(&self, timeout: Duration) -> Option<Vec<Player>> {
        let f = self.players_fns.get_clients?;
        let getters = [self.client_bundle, self.client_parent, self.client_name];
        let pid = self.client_pid;
        let (tx, rx) = mpsc::channel::<Vec<Client>>();
        let block = RcBlock::new(move |arr: *mut AnyObject| {
            // SAFETY: MediaRemote passes a CFArrayRef (possibly null) valid
            // for the duration of the callback.
            let clients = unsafe { (arr as *const NSArray<AnyObject>).as_ref() }
                .map(|a| {
                    a.iter()
                        .take(16)
                        .map(|c| read_client(Retained::as_ptr(&c) as *mut AnyObject, getters, pid))
                        .collect()
                })
                .unwrap_or_default();
            let _ = tx.send(clients);
        });
        // SAFETY: `void (dispatch_queue_t, void (^)(CFArrayRef))`.
        unsafe { f(&*self.queue, &*block as *const _ as *const ObjBlock) };
        let clients = rx.recv_timeout(timeout).ok()?;
        Some(
            clients
                .into_iter()
                // MediaRemote keeps clients of exited processes for a while.
                .filter(|c| c.pid.is_some_and(alive))
                .filter_map(|c| {
                    let path = make_path(c.bundle_id.as_deref()?, c.pid?)?;
                    Some(Player { client: c, path })
                })
                .collect(),
        )
    }

    pub fn player_info(&self, p: &Player, artwork: bool, timeout: Duration) -> Option<Info> {
        let f = self.players_fns.info_for?;
        let copy = self.players_fns.copy_artwork;
        let (tx, rx) = mpsc::channel::<Option<Info>>();
        let block = RcBlock::new(move |dict: *mut AnyObject, cover: *const c_void| {
            if dict.is_null() {
                let _ = tx.send(None);
                return;
            }
            // SAFETY: a CFDictionaryRef with string keys, valid for the call.
            let d = unsafe { &*(dict as *const NSDictionary<NSString, AnyObject>) };
            let mut info = info_from(d);
            if info.artwork.is_none()
                && !cover.is_null()
                && let Some(copy) = copy
            {
                // SAFETY: `cover` is the MRNowPlayingArtwork passed with the
                // info; the copy follows the Create rule.
                let data = unsafe { Retained::from_raw(copy(cover)) };
                info.artwork = data.map(|d| d.to_vec()).filter(|v| !v.is_empty());
            }
            let _ = tx.send(Some(info));
        });
        // SAFETY: `void (MRPlayerPath *, Boolean, dispatch_queue_t,
        // void (^)(CFDictionaryRef, MRNowPlayingArtwork *))`.
        unsafe {
            f(
                Retained::as_ptr(&p.path) as *mut AnyObject,
                artwork,
                &*self.queue,
                &*block,
            )
        };
        rx.recv_timeout(timeout).ok().flatten()
    }

    /// The player's own state: playing (1) / paused, stopped, interrupted
    /// (2–4); unknown and seeking leave the rate to decide.
    pub fn player_playing(&self, p: &Player, timeout: Duration) -> Option<bool> {
        let f = self.players_fns.state_for?;
        let (tx, rx) = mpsc::channel::<Option<bool>>();
        let block = RcBlock::new(move |state: u32| {
            let _ = tx.send(match state {
                1 => Some(true),
                2..=4 => Some(false),
                _ => None,
            });
        });
        // SAFETY: `void (MRPlayerPath *, dispatch_queue_t, void (^)(MRPlaybackState))`.
        unsafe {
            f(
                Retained::as_ptr(&p.path) as *mut AnyObject,
                &*self.queue,
                &*block,
            )
        };
        rx.recv_timeout(timeout).ok().flatten()
    }

    /// Sends `cmd` to this player only. MediaRemote may redirect
    /// unprivileged requests to the system player, so the request is scoped
    /// to the playing item when its id is known.
    pub fn send_to_player(
        &self,
        p: &Player,
        cmd: u32,
        item_id: Option<&str>,
        position: Option<f64>,
        timeout: Duration,
    ) -> bool {
        let Some(f) = self.players_fns.send_to else {
            return false;
        };
        let mut keys: Vec<Retained<NSString>> = vec![];
        let mut vals: Vec<Retained<AnyObject>> = vec![];
        if let (Some(id), Some(k)) = (
            item_id,
            string_constant(self.handle, "kMRMediaRemoteOptionNowPlayingContentItemID"),
        ) {
            keys.push(k);
            vals.push(Retained::into_super(Retained::into_super(
                NSString::from_str(id),
            )));
        }
        if let Some(pos) = position {
            let Some(k) = string_constant(self.handle, "kMRMediaRemoteOptionPlaybackPosition")
            else {
                return false;
            };
            keys.push(k);
            vals.push(Retained::into_super(Retained::into_super(
                Retained::into_super(NSNumber::new_f64(pos)),
            )));
        }
        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let val_refs: Vec<&AnyObject> = vals.iter().map(|v| &**v).collect();
        let options = (!keys.is_empty())
            .then(|| NSDictionary::<NSString, AnyObject>::from_slices(&key_refs, &val_refs));
        let (tx, rx) = mpsc::channel::<bool>();
        let block = RcBlock::new(move |err: u32, responses: *mut AnyObject| {
            // SAFETY: a CFArrayRef of CFNumbers (send statuses) or null.
            let any_ok =
                unsafe { (responses as *const NSArray<AnyObject>).as_ref() }.is_some_and(|a| {
                    a.iter().any(|r| {
                        r.downcast_ref::<NSNumber>()
                            .is_some_and(|n| n.intValue() == 0)
                    })
                });
            let _ = tx.send(err == 0 && any_ok);
        });
        // SAFETY: `Boolean MRMediaRemoteSendCommandToPlayer(MRMediaRemoteCommand,
        // CFDictionaryRef, MRPlayerPath *, MRSendCommandOptions, dispatch_queue_t,
        // void (^)(MRSendError, CFArrayRef))`.
        let queued = unsafe {
            f(
                cmd,
                options.as_ref().map_or(std::ptr::null(), |o| {
                    Retained::as_ptr(o) as *const AnyObject
                }),
                Retained::as_ptr(&p.path) as *mut AnyObject,
                0,
                &*self.queue,
                &*block,
            )
        } != 0;
        queued && rx.recv_timeout(timeout).unwrap_or(false)
    }
}
