//! Owns the single long-lived `/usr/bin/perl` process that hosts
//! `libnox_nowplaying.dylib` (see `crates/nox-nowplaying`). Justification:
//! MediaRemote answers platform binaries only since macOS 15.4; perl is one.
//! The process is started once when the music module starts and ended when
//! it stops (stdin closed, then SIGKILL after a grace period).

use crossbeam_channel::Sender;
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const SCRIPT: &str = "now-playing.pl";
pub const LIBRARY: &str = "libnox_nowplaying.dylib";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Line {
    Hello {
        version: u32,
        #[serde(default)]
        notifications: bool,
    },
    State(Box<StateLine>),
    Empty,
    Sent {
        cmd: String,
        ok: bool,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StateLine {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub playing: bool,
    pub rate: f64,
    pub elapsed: Option<f64>,
    pub observed_at: Option<f64>,
    pub item_id: Option<String>,
    pub shuffle_mode: Option<i64>,
    pub repeat_mode: Option<i64>,
    pub queue_index: Option<i64>,
    pub queue_count: Option<i64>,
    pub pid: Option<i32>,
    pub bundle_id: Option<String>,
    pub parent_bundle_id: Option<String>,
    pub player_name: Option<String>,
    pub artwork_id: Option<u64>,
    pub artwork_mime: Option<String>,
    /// Base64, only present when the artwork changed.
    pub artwork: Option<String>,
}

impl StateLine {
    /// The app that owns the session (a browser rather than its web helper).
    pub fn app_bundle_id(&self) -> Option<&str> {
        self.parent_bundle_id
            .as_deref()
            .or(self.bundle_id.as_deref())
    }
}

pub fn parse_line(line: &str) -> Option<Line> {
    serde_json::from_str(line.trim()).ok()
}

#[derive(Debug)]
pub enum Event {
    Line(Line),
    /// The process ended (crash, kill or EOF).
    Exited,
}

/// Finds the perl script and the adapter library:
/// 1. `NOX_NOWPLAYING_DIR` (both files inside; debug builds only);
/// 2. the app bundle (`Contents/Resources`);
/// 3. a development tree (`target/<profile>` + `packaging/resources`).
pub fn locate() -> Result<(PathBuf, PathBuf), String> {
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();
    // Development override only: in release, nothing in the environment can
    // choose the library perl loads.
    #[cfg(debug_assertions)]
    if let Some(d) = std::env::var_os("NOX_NOWPLAYING_DIR") {
        let d = PathBuf::from(d);
        candidates.push((d.join(SCRIPT), d.join(LIBRARY)));
    }
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.canonicalize().unwrap_or(exe);
        if let Some(dir) = exe.parent() {
            let resources = dir.join("../Resources");
            candidates.push((resources.join(SCRIPT), resources.join(LIBRARY)));
            let dev_script = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../packaging/resources")
                .join(SCRIPT);
            // target/<profile>/nox, or target/<profile>/deps|examples/<bin>.
            for lib_dir in [dir.to_path_buf(), dir.join("..")] {
                candidates.push((dev_script.clone(), lib_dir.join(LIBRARY)));
            }
        }
    }
    for (script, lib) in candidates {
        if script.is_file() && lib.is_file() {
            let script = script.canonicalize().map_err(|e| e.to_string())?;
            let lib = lib.canonicalize().map_err(|e| e.to_string())?;
            return Ok((script, lib));
        }
    }
    Err(format!("{SCRIPT} / {LIBRARY} introuvables"))
}

pub struct Adapter {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
}

impl Adapter {
    pub fn spawn(script: &Path, library: &Path, tx: Sender<Event>) -> std::io::Result<Self> {
        let mut child = Command::new("/usr/bin/perl")
            .arg(script)
            .arg(library)
            .arg("watch")
            // A clean environment: nothing (PERL5LIB, PERL5OPT, PERLIO…) may
            // inject code into the adapter. HOME/TMPDIR for Foundation.
            .env_clear()
            .envs(
                ["HOME", "TMPDIR", "USER", "LOGNAME"]
                    .iter()
                    .filter_map(|k| std::env::var_os(k).map(|v| (*k, v))),
            )
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stdin = child.stdin.take();
        let reader = std::thread::Builder::new()
            .name("nox-music-adapter".into())
            .spawn(move || {
                let mut r = BufReader::new(stdout);
                let mut buf = String::new();
                loop {
                    buf.clear();
                    match r.read_line(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            if let Some(l) = parse_line(&buf)
                                && tx.send(Event::Line(l)).is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                let _ = tx.send(Event::Exited);
            })?;
        Ok(Self {
            child,
            stdin,
            reader: Some(reader),
        })
    }

    /// Sends one request line; `false` if the process is gone.
    pub fn request(&mut self, json: &serde_json::Value) -> bool {
        let Some(stdin) = self.stdin.as_mut() else {
            return false;
        };
        let mut line = json.to_string();
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.flush())
            .is_ok()
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Closes stdin (the adapter exits on EOF), then kills after 1 s.
    pub fn shutdown(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_adapter_lines() {
        assert_eq!(
            parse_line(r#"{"type":"hello","version":1,"notifications":true}"#),
            Some(Line::Hello {
                version: 1,
                notifications: true
            })
        );
        assert_eq!(parse_line(r#"{"type":"empty"}"#), Some(Line::Empty));
        assert_eq!(
            parse_line(r#"{"type":"sent","cmd":"next","ok":false}"#),
            Some(Line::Sent {
                cmd: "next".into(),
                ok: false
            })
        );
        let Some(Line::State(s)) = parse_line(
            r#"{"type":"state","title":"Song","artist":"A","duration":180.5,"playing":true,
               "rate":1.0,"elapsed":12.0,"observed_at":1000.0,"pid":42,
               "bundle_id":"com.google.Chrome.helper","parent_bundle_id":"com.google.Chrome",
               "artwork_id":7,"artwork":"Zm9v","unknown_future_field":1}"#,
        ) else {
            panic!("state line")
        };
        assert_eq!(s.title.as_deref(), Some("Song"));
        assert_eq!(s.app_bundle_id(), Some("com.google.Chrome"));
        assert_eq!(s.artwork_id, Some(7));
        assert!(parse_line("not json").is_none());
        assert!(parse_line(r#"{"type":"surprise"}"#).is_none());
    }
}
