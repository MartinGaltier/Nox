//! Claude Code's live session registry: while it runs, the CLI keeps
//! `~/.claude/sessions/<pid>.json` with its `sessionId`, `status`
//! (`busy` / `idle` / `waiting`) and `waitingFor`. This is the measured source
//! of a Claude session's phase. A file left behind by a crashed CLI is
//! ignored once its process is gone (`kill(pid, 0)`).

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
    Waiting,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub pid: i32,
    pub status: Status,
    pub waiting_for: Option<String>,
    pub cwd: Option<String>,
    /// Milliseconds since the epoch.
    pub status_updated_ms: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    pid: Option<i32>,
    session_id: Option<String>,
    status: Option<String>,
    waiting_for: Option<String>,
    cwd: Option<String>,
    status_updated_at: Option<f64>,
}

/// Whether a process with this pid exists (EPERM still means it exists).
pub fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence/permission check.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Live Claude sessions by session id.
pub fn read(dir: &Path, alive: &dyn Fn(i32) -> bool) -> HashMap<String, Entry> {
    let mut out = HashMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten().take(256) {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        if md.len() > 64 << 10 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        let Ok(raw) = serde_json::from_slice::<Raw>(&bytes) else {
            continue;
        };
        let (Some(pid), Some(sid)) = (raw.pid, raw.session_id) else {
            continue;
        };
        if !alive(pid) {
            continue;
        }
        let status = match raw.status.as_deref() {
            Some("busy") => Status::Busy,
            Some("idle") => Status::Idle,
            Some("waiting") => Status::Waiting,
            _ => Status::Unknown,
        };
        out.insert(
            sid,
            Entry {
                pid,
                status,
                waiting_for: raw.waiting_for.map(|w| w.chars().take(40).collect()),
                cwd: raw.cwd,
                status_updated_ms: raw.status_updated_at,
            },
        );
    }
    out
}

/// French wording of `waitingFor`.
pub fn waiting_label(w: Option<&str>) -> String {
    match w {
        Some(w) if w.contains("permission") || w.contains("approval") => {
            "Autorisation demandée".into()
        }
        Some(w) if w.contains("question") || w.contains("input") => "Question posée".into(),
        Some(w) if !w.is_empty() => format!("En attente : {w}"),
        _ => "En attente de vous".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_live_entries_and_skips_dead_pids() {
        let dir = std::env::temp_dir().join(format!("nox-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("100.json"),
            r#"{"pid":100,"sessionId":"s-live","status":"waiting","waitingFor":"permission","cwd":"/tmp/a","statusUpdatedAt":1}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("200.json"),
            r#"{"pid":200,"sessionId":"s-dead","status":"busy"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("300.key"), "x").unwrap();
        let r = read(&dir, &|pid| pid == 100);
        assert_eq!(r.len(), 1);
        let e = &r["s-live"];
        assert_eq!(e.status, Status::Waiting);
        assert_eq!(
            waiting_label(e.waiting_for.as_deref()),
            "Autorisation demandée"
        );
        assert!(pid_alive(std::process::id() as i32));
        assert!(!pid_alive(-1));
        let _ = std::fs::remove_dir_all(dir);
    }
}
