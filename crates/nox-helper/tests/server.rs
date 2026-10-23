//! The real server loop, unprivileged, on a temporary socket with the fake
//! backend: protocol, peer checks, crash of a client process (SIGKILL),
//! graceful stop.

use nox_helper::backend::FakeBackend;
use nox_helper::core::HelperCore;
use nox_helper::journal::{JournalStore, Loaded};
use nox_helper::peer::SameUserPolicy;
use nox_helper::server::{Server, StopHandle, stop_pair};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct Running {
    stop: StopHandle,
    thread: Option<JoinHandle<()>>,
    fake: FakeBackend,
    journal: JournalStore,
    sock: PathBuf,
    dir: PathBuf,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start(name: &str, allowed_client: Option<PathBuf>) -> Running {
    // sun_path is limited to 104 bytes.
    let mut dir = std::env::temp_dir().join(format!("nxh-{name}-{}", std::process::id()));
    if dir.as_os_str().len() > 80 {
        dir = PathBuf::from(format!("/tmp/nxh-{name}-{}", std::process::id()));
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("h.sock");
    let journal = JournalStore::new(dir.join("helper-state.json"));
    let fake = FakeBackend::new();
    let (stop, rx) = stop_pair().unwrap();
    let mut server = Server::bind(&sock, SameUserPolicy { allowed_client }).unwrap();
    let mut core = HelperCore::new(fake.clone(), journal.clone());
    let thread = std::thread::spawn(move || {
        core.startup();
        server.run(&mut core, &rx).unwrap();
        core.shutdown();
    });
    Running {
        stop,
        thread: Some(thread),
        fake,
        journal,
        sock,
        dir,
    }
}

struct Conn(BufReader<UnixStream>);

impl Conn {
    fn open(r: &Running) -> Self {
        let s = UnixStream::connect(&r.sock).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        Conn(BufReader::new(s))
    }
    fn ask(&mut self, line: &str) -> String {
        self.0
            .get_mut()
            .write_all(format!("{line}\n").as_bytes())
            .unwrap();
        let mut out = String::new();
        self.0.read_line(&mut out).unwrap();
        out.trim_end().to_string()
    }
}

fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

#[test]
fn protocol_over_socket() {
    let r = start("proto", None);
    let mut c = Conn::open(&r);
    assert!(c.ask("version").starts_with("ok version 1 "));
    assert_eq!(
        c.ask("status"),
        "ok status sleep_disabled=0 lease=none applied=0 revoked=none"
    );
    assert!(
        c.ask("pmset -a disablesleep 1")
            .starts_with("err bad-request")
    );
    assert!(c.ask("acquire 1 0").starts_with("err bad-request"));
    assert_eq!(c.ask("heartbeat"), "err no-lease");
    assert_eq!(c.ask("acquire 30 10"), "ok acquired applied=1");
    assert!(matches!(r.journal.load(), Loaded::Present(_)));
    assert_eq!(c.ask("heartbeat"), "ok heartbeat");
    assert_eq!(
        c.ask("status"),
        "ok status sleep_disabled=1 lease=held applied=1 revoked=none"
    );
    assert_eq!(c.ask("release"), "ok released restored");
    assert_eq!(r.fake.with(|s| s.writes.clone()), vec![true, false]);
    assert_eq!(r.journal.load(), Loaded::Absent);
}

#[test]
fn rejects_wrong_client_binary() {
    let r = start(
        "peer",
        Some(PathBuf::from("/Applications/Nox.app/Contents/MacOS/nox")),
    );
    let mut c = Conn::open(&r);
    let mut line = String::new();
    c.0.read_line(&mut line).unwrap();
    assert!(line.starts_with("err forbidden"), "{line}");
    assert!(r.fake.with(|s| s.writes.is_empty()));
}

#[test]
fn accepts_allowed_client_binary() {
    let me = std::env::current_exe().unwrap();
    let r = start("peerok", Some(me));
    let mut c = Conn::open(&r);
    assert!(c.ask("version").starts_with("ok version"));
}

#[test]
fn long_lines_are_refused_and_closed() {
    let r = start("long", None);
    let mut c = Conn::open(&r);
    let reply = c.ask(&"x".repeat(5000));
    assert!(reply.starts_with("err bad-request"), "{reply}");
    let mut rest = String::new();
    assert_eq!(c.0.read_line(&mut rest).unwrap_or(0), 0, "closed");
}

/// A real client process holding the lease is SIGKILLed: the helper restores
/// after the grace period without any cooperation from the client.
#[test]
fn killed_client_process_is_restored_after_grace() {
    let r = start("kill9", None);
    let mut child = std::process::Command::new("/usr/bin/nc")
        .arg("-U")
        .arg(&r.sock)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"acquire 5 0\n")
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(line.trim_end(), "ok acquired applied=1");
    let killed_at = Instant::now();
    // SIGKILL: no destructor, no goodbye.
    child.kill().unwrap();
    child.wait().unwrap();

    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        r.fake.with(|s| s.sleep_disabled),
        Some(true),
        "within grace"
    );
    assert!(wait_until(Duration::from_secs(10), || r
        .fake
        .with(|s| s.sleep_disabled)
        == Some(false)));
    assert!(killed_at.elapsed() >= Duration::from_secs(5));
    assert_eq!(r.journal.load(), Loaded::Absent);
    let mut c = Conn::open(&r);
    assert_eq!(
        c.ask("status"),
        "ok status sleep_disabled=0 lease=none applied=0 revoked=client-gone"
    );
}

#[test]
fn stopping_the_helper_restores() {
    let mut r = start("stop", None);
    let mut c = Conn::open(&r);
    assert_eq!(c.ask("acquire 60 0"), "ok acquired applied=1");
    r.stop.stop();
    r.thread.take().unwrap().join().unwrap();
    assert_eq!(r.fake.with(|s| s.sleep_disabled), Some(false));
    assert!(!r.sock.exists(), "socket removed");
}

#[test]
fn journal_left_by_previous_run_is_restored_at_start() {
    let dir = PathBuf::from(format!("/tmp/nxh-boot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let journal = JournalStore::new(dir.join("helper-state.json"));
    journal
        .store(&nox_helper::journal::Journal::new(501))
        .unwrap();
    let fake = FakeBackend::new();
    fake.with(|s| s.sleep_disabled = Some(true));
    let mut core = HelperCore::new(fake.clone(), journal.clone());
    assert_eq!(
        core.startup(),
        nox_helper::protocol::RestoreOutcome::Restored
    );
    assert_eq!(fake.with(|s| s.sleep_disabled), Some(false));
    let _ = std::fs::remove_dir_all(&dir);
}
