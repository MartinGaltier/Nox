//! Who may talk to the helper.
//!
//! The socket file is `root:staff 0660`; on top of that every connection is
//! checked here: effective uid of the peer (`getpeereid`) must be the user
//! logged in at the console (or root), and — when the helper was installed
//! for a given app — the peer's executable (`LOCAL_PEERPID` →
//! `proc_pidpath`) must be that app's binary.
//!
//! Limits (documented in docs/SLEEP_RECOVERY.md): the pid → path lookup is
//! not atomic with the connection (pid reuse is theoretically racy), and Nox
//! is ad-hoc signed, so no Team ID based code requirement is possible. The
//! worst a same-user impostor can do is the whitelisted operation itself —
//! keep lid sleep disabled while it heartbeats — still bounded by the battery
//! and thermal guards.

use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub uid: u32,
    pub pid: Option<i32>,
}

pub trait PeerPolicy: Send {
    fn check(&self, fd: RawFd) -> Result<Peer, String>;
}

/// Production policy.
pub struct ConsolePolicy {
    allowed_client: Option<PathBuf>,
}

impl ConsolePolicy {
    pub fn new(allowed_client: Option<&Path>) -> Self {
        Self {
            allowed_client: allowed_client.map(|p| p.canonicalize().unwrap_or_else(|_| p.into())),
        }
    }
}

impl PeerPolicy for ConsolePolicy {
    fn check(&self, fd: RawFd) -> Result<Peer, String> {
        let uid = crate::sys::peer_euid(fd).ok_or("getpeereid failed")?;
        let pid = crate::sys::peer_pid(fd);
        if uid == 0 {
            return Ok(Peer { uid, pid });
        }
        if crate::sys::console_uid() != Some(uid) {
            return Err(format!("uid {uid} is not the console user"));
        }
        check_path(self.allowed_client.as_deref(), pid)?;
        Ok(Peer { uid, pid })
    }
}

fn check_path(allowed: Option<&Path>, pid: Option<i32>) -> Result<(), String> {
    let Some(allowed) = allowed else {
        return Ok(());
    };
    let pid = pid.ok_or("peer pid unavailable")?;
    let exe = crate::sys::pid_path(pid).ok_or("peer path unavailable")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    if exe == allowed {
        Ok(())
    } else {
        Err(format!("client {} is not the installed app", exe.display()))
    }
}

/// Test / development policy: same uid as this process, optional path check.
pub struct SameUserPolicy {
    pub allowed_client: Option<PathBuf>,
}

impl PeerPolicy for SameUserPolicy {
    fn check(&self, fd: RawFd) -> Result<Peer, String> {
        let uid = crate::sys::peer_euid(fd).ok_or("getpeereid failed")?;
        if uid != crate::sys::euid() {
            return Err(format!("uid {uid} not allowed"));
        }
        let pid = crate::sys::peer_pid(fd);
        let allowed = self
            .allowed_client
            .as_ref()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()));
        check_path(allowed.as_deref(), pid)?;
        Ok(Peer { uid, pid })
    }
}
