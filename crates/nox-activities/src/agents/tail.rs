//! Incremental JSONL reading: each file keeps its offset and identity, so a
//! tick only reads what was appended. Lines longer than `MAX_LINE` (tool
//! output, compacted history) are skipped without being kept in memory; only
//! their first bytes are handed over so a byte filter can still classify them.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Lines are kept whole up to this size.
pub const MAX_LINE: usize = 8 << 20;
/// Prefix handed over for a skipped line.
const PREFIX: usize = 4096;
const CHUNK: usize = 256 << 10;

#[derive(Debug, Clone)]
pub struct Cursor {
    pub path: PathBuf,
    pub offset: u64,
    pub inode: u64,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadOutcome {
    Unchanged,
    /// New bytes were read (`restarted` when the file was replaced/truncated).
    Read {
        restarted: bool,
    },
    Gone,
}

impl Cursor {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            inode: 0,
            modified: None,
            size: 0,
        }
    }

    /// Cheap check: has the file changed since the last read?
    pub fn changed(&self) -> Option<bool> {
        let md = std::fs::metadata(&self.path).ok()?;
        Some(md.ino() != self.inode || md.len() != self.offset)
    }

    /// Reads complete lines appended since the last call. A trailing partial
    /// line is left for next time (the offset stops at the last newline).
    pub fn read_new(&mut self, mut line: impl FnMut(&[u8], bool)) -> ReadOutcome {
        let Ok(mut f) = File::open(&self.path) else {
            return ReadOutcome::Gone;
        };
        let Ok(md) = f.metadata() else {
            return ReadOutcome::Gone;
        };
        let mut restarted = false;
        if md.ino() != self.inode || md.len() < self.offset {
            restarted = self.inode != 0;
            self.inode = md.ino();
            self.offset = 0;
        }
        self.modified = md.modified().ok();
        self.size = md.len();
        if md.len() == self.offset {
            return if restarted {
                ReadOutcome::Read { restarted }
            } else {
                ReadOutcome::Unchanged
            };
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return ReadOutcome::Gone;
        }
        let mut buf: Vec<u8> = Vec::with_capacity(CHUNK);
        let mut chunk = vec![0u8; CHUNK];
        // File offset of `data[0]`.
        let mut pos = self.offset;
        let mut skipping = false;
        let mut prefix: Vec<u8> = Vec::new();
        loop {
            let n = match f.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let mut data = &chunk[..n];
            while !data.is_empty() {
                match memchr::memchr(b'\n', data) {
                    Some(i) => {
                        if skipping {
                            line(&prefix, true);
                            prefix.clear();
                            skipping = false;
                        } else if buf.len() + i > MAX_LINE {
                            prefix.clear();
                            prefix.extend_from_slice(&buf[..buf.len().min(PREFIX)]);
                            let room = PREFIX - prefix.len();
                            prefix.extend_from_slice(&data[..room.min(i)]);
                            line(&prefix, true);
                            prefix.clear();
                            buf = Vec::new();
                        } else {
                            buf.extend_from_slice(&data[..i]);
                            if !buf.is_empty() {
                                line(&buf, false);
                            }
                            buf.clear();
                        }
                        pos += (i + 1) as u64;
                        self.offset = pos;
                        data = &data[i + 1..];
                    }
                    None => {
                        if !skipping && buf.len() + data.len() > MAX_LINE {
                            skipping = true;
                            prefix.clear();
                            prefix.extend_from_slice(&buf[..buf.len().min(PREFIX)]);
                            let room = PREFIX - prefix.len();
                            prefix.extend_from_slice(&data[..room.min(data.len())]);
                            buf = Vec::new();
                        } else if !skipping {
                            buf.extend_from_slice(data);
                        }
                        pos += data.len() as u64;
                        data = &[];
                    }
                }
            }
        }
        // `self.offset` sits after the last newline; partial data is re-read later.
        ReadOutcome::Read { restarted }
    }
}

/// Files under `dir` (recursively, up to `depth`) accepted by `accept`,
/// modified at or after `since`.
pub fn discover(
    dir: &Path,
    depth: usize,
    since: SystemTime,
    accept: &dyn Fn(&Path) -> bool,
    out: &mut Vec<PathBuf>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            if depth > 0 {
                discover(&p, depth - 1, since, accept, out);
            }
        } else if ft.is_file()
            && accept(&p)
            && e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .is_some_and(|m| m >= since)
        {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nox-tail-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("log.jsonl")
    }

    fn collect(c: &mut Cursor) -> (Vec<(String, bool)>, ReadOutcome) {
        let mut v = Vec::new();
        let o = c.read_new(|l, skipped| v.push((String::from_utf8_lossy(l).into_owned(), skipped)));
        (v, o)
    }

    #[test]
    fn reads_only_appended_complete_lines() {
        let p = tmp("append");
        std::fs::write(&p, "a\nb\npart").unwrap();
        let mut c = Cursor::new(p.clone());
        let (v, _) = collect(&mut c);
        assert_eq!(v, vec![("a".into(), false), ("b".into(), false)]);
        assert_eq!(c.offset, 4);
        assert_eq!(collect(&mut c).0, vec![], "partial line waits");
        assert_eq!(c.changed(), Some(true));
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"ial\nc\n").unwrap();
        let (v, _) = collect(&mut c);
        assert_eq!(v, vec![("partial".into(), false), ("c".into(), false)]);
        assert_eq!(collect(&mut c).1, ReadOutcome::Unchanged);
        assert_eq!(c.changed(), Some(false));
    }

    #[test]
    fn restarts_on_truncation_and_replacement() {
        let p = tmp("rotate");
        std::fs::write(&p, "one\ntwo\n").unwrap();
        let mut c = Cursor::new(p.clone());
        collect(&mut c);
        std::fs::write(&p, "x\n").unwrap();
        let (v, o) = collect(&mut c);
        assert_eq!(o, ReadOutcome::Read { restarted: true });
        assert_eq!(v, vec![("x".into(), false)]);
        let p2 = p.with_extension("new");
        std::fs::write(&p2, "y\n").unwrap();
        std::fs::rename(&p2, &p).unwrap();
        let (v, o) = collect(&mut c);
        assert_eq!(o, ReadOutcome::Read { restarted: true });
        assert_eq!(v, vec![("y".into(), false)]);
        std::fs::remove_file(&p).unwrap();
        assert_eq!(collect(&mut c).1, ReadOutcome::Gone);
    }

    #[test]
    fn oversized_lines_are_skipped_with_prefix() {
        let p = tmp("huge");
        let mut body = String::from(r#"{"type":"response_item","x":""#);
        body.push_str(&"z".repeat(MAX_LINE + 10));
        body.push_str("\"}\nsmall\n");
        std::fs::write(&p, &body).unwrap();
        let mut c = Cursor::new(p.clone());
        let (v, _) = collect(&mut c);
        assert_eq!(v.len(), 2);
        assert!(v[0].1, "flagged as skipped");
        assert!(v[0].0.starts_with(r#"{"type":"response_item""#));
        assert!(v[0].0.len() <= PREFIX);
        assert_eq!(v[1], ("small".into(), false));
        assert_eq!(c.offset, body.len() as u64);
    }
}
