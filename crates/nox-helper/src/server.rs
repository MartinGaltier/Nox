//! Single-threaded Unix socket server: `poll(2)` over a stop pipe, the
//! listener and at most [`MAX_CLIENTS`] connections. Each request line is
//! answered with one response line by [`HelperCore`].

use crate::backend::SleepBackend;
use crate::core::{ClientId, HelperCore};
use crate::log_line;
use crate::peer::{Peer, PeerPolicy};
use crate::protocol::{ErrorCode, MAX_LINE, Request, Response};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const MAX_CLIENTS: usize = 8;
/// `staff`: every local user account is a member.
const STAFF_GID: u32 = 20;

struct Client {
    id: ClientId,
    stream: UnixStream,
    peer: Peer,
    buf: Vec<u8>,
}

pub struct Server<P: PeerPolicy> {
    listener: UnixListener,
    path: PathBuf,
    policy: P,
    next_id: ClientId,
}

/// Wakes the loop and makes `run` return (tests, signal handler).
pub struct StopHandle(UnixStream);

impl StopHandle {
    pub fn stop(&self) {
        let _ = (&self.0).write_all(&[1]);
    }
    pub fn raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

pub struct StopReceiver(UnixStream);

pub fn stop_pair() -> std::io::Result<(StopHandle, StopReceiver)> {
    let (a, b) = UnixStream::pair()?;
    b.set_nonblocking(true)?;
    Ok((StopHandle(a), StopReceiver(b)))
}

impl<P: PeerPolicy> Server<P> {
    /// Binds `path` (replacing a stale socket, never another kind of file),
    /// mode 0660; group `staff` when running as root.
    pub fn bind(path: &Path, policy: P) -> std::io::Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_socket() => std::fs::remove_file(path)?,
            Ok(_) => {
                return Err(std::io::Error::new(
                    ErrorKind::AlreadyExists,
                    format!("{} exists and is not a socket", path.display()),
                ));
            }
            Err(_) => {}
        }
        let listener = UnixListener::bind(path)?;
        if crate::sys::euid() == 0 {
            std::os::unix::fs::chown(path, Some(0), Some(STAFF_GID))?;
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path: path.to_owned(),
            policy,
            next_id: 1,
        })
    }

    /// Serves until `stop` fires; then returns (the caller runs
    /// `core.shutdown()`).
    pub fn run<B: SleepBackend>(
        &mut self,
        core: &mut HelperCore<B>,
        stop: &StopReceiver,
    ) -> std::io::Result<()> {
        let mut clients: Vec<Client> = Vec::new();
        loop {
            let timeout = core.tick(Instant::now());
            let mut fds = vec![stop.0.as_raw_fd(), self.listener.as_raw_fd()];
            fds.extend(clients.iter().map(|c| c.stream.as_raw_fd()));
            let ready = crate::sys::poll_readable(&fds, timeout)?;
            if ready[0] {
                let mut b = [0u8; 16];
                let _ = (&stop.0).read(&mut b);
                return Ok(());
            }
            if ready[1] {
                self.accept_all(&mut clients);
            }
            let now = Instant::now();
            let mut closed = Vec::new();
            for (i, c) in clients.iter_mut().enumerate() {
                if ready.get(i + 2).copied().unwrap_or(false) && !serve(core, c, now) {
                    closed.push(i);
                }
            }
            for i in closed.into_iter().rev() {
                let c = clients.remove(i);
                core.disconnected(c.id, Instant::now());
            }
        }
    }

    fn accept_all(&mut self, clients: &mut Vec<Client>) {
        loop {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    let peer = match self.policy.check(stream.as_raw_fd()) {
                        Ok(p) => p,
                        Err(e) => {
                            log_line(&format!("connection refused: {e}"));
                            let r = Response::err(ErrorCode::Forbidden, e);
                            let _ = stream.write_all(format!("{r}\n").as_bytes());
                            continue;
                        }
                    };
                    if clients.len() >= MAX_CLIENTS {
                        let r = Response::err(ErrorCode::Busy, "too many connections");
                        let _ = stream.write_all(format!("{r}\n").as_bytes());
                        continue;
                    }
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    let id = self.next_id;
                    self.next_id += 1;
                    clients.push(Client {
                        id,
                        stream,
                        peer,
                        buf: Vec::with_capacity(MAX_LINE + 2),
                    });
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    log_line(&format!("accept: {e}"));
                    return;
                }
            }
        }
    }
}

impl<P: PeerPolicy> Drop for Server<P> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Reads what is available and answers complete lines. `false` = close.
fn serve<B: SleepBackend>(core: &mut HelperCore<B>, c: &mut Client, now: Instant) -> bool {
    let mut chunk = [0u8; 256];
    loop {
        match c.stream.read(&mut chunk) {
            Ok(0) => return false,
            Ok(n) => c.buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
        if c.buf.len() > 4 * MAX_LINE {
            break;
        }
    }
    while let Some(pos) = c.buf.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = c.buf.drain(..=pos).collect();
        let line = String::from_utf8_lossy(&line[..line.len() - 1]);
        let line = line.trim_end_matches('\r');
        let resp = match Request::parse(line) {
            Ok(req) => core.handle(c.id, c.peer.uid, req, now),
            Err(e) => Response::err(ErrorCode::BadRequest, e),
        };
        if c.stream.write_all(format!("{resp}\n").as_bytes()).is_err() {
            return false;
        }
    }
    if c.buf.len() > MAX_LINE + 1 {
        let r = Response::err(ErrorCode::BadRequest, "line too long");
        let _ = c.stream.write_all(format!("{r}\n").as_bytes());
        return false;
    }
    true
}
