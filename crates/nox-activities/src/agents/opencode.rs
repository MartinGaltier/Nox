// Adapted from vorssaint-utils (GPL-3.0-or-later), Services/AgentUsage/Harness/OpenCodeReader.swift
// and AgentOpenCodeReader.swift, commit e916961.
//! OpenCode keeps one record per message: as JSON files in
//! `~/.local/share/opencode/storage/message/<session>/<message>.json` (older
//! releases) or as rows of `~/.local/share/opencode/opencode.db`. Only role,
//! model, token counts, reported cost, times, finish reason and working folder
//! are read; for the database they are extracted in SQL so prompt and reply
//! text never leave it.

use super::parse::{Entry, FileState, int, project_name, seconds, str_of};
use super::pricing::{Billable, Tokens};
use nox_core::model::Timestamp;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One message record → entries. `st.session` is set to the message's session
/// so the caller attributes the entries to it.
pub fn parse(msg: &Value, st: &mut FileState, now: Timestamp) -> Vec<Entry> {
    let Some(role) = str_of(msg, "role") else {
        return Vec::new();
    };
    let id = str_of(msg, "id").unwrap_or_default().to_string();
    let session = str_of(msg, "sessionID")
        .or_else(|| str_of(msg, "session_id"))
        .unwrap_or_default()
        .to_string();
    if session.is_empty() {
        return Vec::new();
    }
    st.session.clone_from(&session);
    let time = msg.get("time");
    let created = seconds(time.and_then(|t| t.get("created")))
        .or_else(|| seconds(msg.get("time_created")))
        .unwrap_or(now);
    let completed = seconds(time.and_then(|t| t.get("completed")));
    if let Some(cwd) = msg
        .get("path")
        .and_then(|p| str_of(p, "cwd"))
        .or_else(|| str_of(msg, "directory"))
    {
        st.project = project_name(cwd);
    }
    let model = str_of(msg, "modelID")
        .or_else(|| str_of(msg, "model_id"))
        .or_else(|| msg.get("model").and_then(|m| str_of(m, "modelID")))
        .or_else(|| str_of(msg, "model"));
    if let Some(m) = model {
        st.model = m.to_string();
    }
    let open = st.opencode_open.get(&session).copied().unwrap_or(false);
    // Subagent sessions never own a turn; their usage still counts.
    let child = str_of(msg, "parent_session_id").is_some();
    let mut out = Vec::new();
    match role {
        "user" => {
            if child || !seen_insert(st, &id) {
                return out;
            }
            out.push(if open {
                Entry::TurnActive(created)
            } else {
                Entry::TurnBegan(created)
            });
            st.opencode_open.insert(session, true);
        }
        "assistant" => {
            if let Some(t) = msg.get("tokens").filter(|t| t.is_object()) {
                let cache = t.get("cache");
                let tokens = Tokens {
                    input: int(t.get("input")),
                    output: int(t.get("output")) + int(t.get("reasoning")),
                    reasoning: int(t.get("reasoning")),
                    cache_read: int(cache.and_then(|c| c.get("read"))),
                    cache_write: int(cache.and_then(|c| c.get("write"))),
                };
                if tokens.total() > 0 && !id.is_empty() {
                    let reported = msg
                        .get("cost")
                        .and_then(Value::as_f64)
                        .filter(|c| c.is_finite() && *c > 0.0);
                    out.push(Entry::Usage {
                        key: format!("opencode:{id}"),
                        at: created,
                        model: st.model.clone(),
                        billable: Billable {
                            tokens,
                            reported_cost: reported,
                            ..Default::default()
                        },
                    });
                }
            }
            if child {
                out.push(Entry::TurnActive(completed.unwrap_or(created)));
                return out;
            }
            let finish = str_of(msg, "finish");
            let error = msg
                .get("error")
                .is_some_and(|e| !e.is_null() && e != &Value::Bool(false));
            let ends = completed.is_some()
                && (error || !matches!(finish, Some("tool-calls" | "unknown") | None));
            if ends {
                if open {
                    out.push(Entry::TurnEnded {
                        at: completed.unwrap_or(created),
                        completed: !error,
                        duration: None,
                    });
                }
                st.opencode_open.insert(session, false);
            } else {
                out.push(if open {
                    Entry::TurnActive(completed.unwrap_or(created))
                } else {
                    Entry::TurnBegan(created)
                });
                st.opencode_open.insert(session, true);
            }
        }
        _ => {}
    }
    out
}

/// Remembers a prompt id; false when it was seen already (rewritten record).
fn seen_insert(st: &mut FileState, id: &str) -> bool {
    if id.is_empty() {
        return true;
    }
    if st.opencode_seen.len() > 4096 {
        st.opencode_seen.clear();
    }
    st.opencode_seen.insert(id.to_string())
}

/// A legacy storage message file, read whole (they are small and rewritten),
/// reduced to the fields the parser reads.
pub fn read_message_file(path: &Path) -> Option<Value> {
    let md = std::fs::metadata(path).ok()?;
    if md.len() > 1 << 20 {
        return None;
    }
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let mut o = serde_json::Map::new();
    for k in [
        "id",
        "sessionID",
        "role",
        "time",
        "modelID",
        "model",
        "tokens",
        "cost",
        "finish",
    ] {
        if let Some(x) = v.get(k) {
            o.insert(k.into(), x.clone());
        }
    }
    if let Some(cwd) = v.get("path").and_then(|p| p.get("cwd")) {
        o.insert("path".into(), json!({ "cwd": cwd }));
    }
    o.insert(
        "error".into(),
        Value::Bool(v.get("error").is_some_and(|e| !e.is_null())),
    );
    Some(Value::Object(o))
}

/// Incremental reader for `opencode.db` (read-only connection).
pub struct Database {
    pub path: PathBuf,
    last_rowid: i64,
    /// Replies still being written, re-read until they complete.
    open: Vec<String>,
    modified: Option<SystemTime>,
}

impl Database {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_rowid: 0,
            open: Vec::new(),
            modified: None,
        }
    }

    /// Newest change of the database or its write-ahead log.
    fn stamp(&self) -> Option<SystemTime> {
        let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let mut wal = self.path.clone().into_os_string();
        wal.push("-wal");
        m(&self.path).max(m(Path::new(&wal)))
    }

    /// Calls `row` with a compact JSON record for each new or still-open
    /// message created after `since_ms`. Returns false when unreadable.
    pub fn read_new(&mut self, since_ms: i64, mut row: impl FnMut(&Value)) -> bool {
        let stamp = self.stamp();
        if stamp.is_some() && stamp == self.modified {
            return true;
        }
        let Ok(db) = rusqlite::Connection::open_with_flags(
            &self.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return false;
        };
        let _ = db.busy_timeout(std::time::Duration::from_millis(500));
        let has = |sql: &str| db.query_row(sql, [], |_| Ok(())).is_ok();
        let v2 = has("SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_v2'");
        let (mt, stb, role) = if v2 {
            ("session_message", "session_v2", "m.type")
        } else {
            ("message", "session", "json_extract(m.data,'$.role')")
        };
        let parent = if has(&format!(
            "SELECT 1 FROM pragma_table_info('{stb}') WHERE name='parent_id'"
        )) {
            "s.parent_id"
        } else {
            "NULL"
        };
        let sql = format!(
            "SELECT m.rowid, m.id, m.session_id, m.time_created, {role},
                json_extract(m.data,'$.modelID'), json_extract(m.data,'$.tokens'),
                json_extract(m.data,'$.cost'), json_extract(m.data,'$.finish'),
                json_extract(m.data,'$.time'), json_extract(m.data,'$.path.cwd'),
                json_type(m.data,'$.error'), s.directory, {parent}
             FROM {mt} m JOIN {stb} s ON s.id = m.session_id
             WHERE (m.rowid > ?1 AND m.time_created >= ?2)
                OR m.id IN (SELECT value FROM json_each(?3))
             ORDER BY m.rowid"
        );
        let open = serde_json::to_string(&self.open).unwrap_or_else(|_| "[]".into());
        let Ok(mut stmt) = db.prepare(&sql) else {
            return false;
        };
        let rows = stmt.query_map(rusqlite::params![self.last_rowid, since_ms, open], |r| {
            let text = |i: usize| r.get::<_, Option<String>>(i).ok().flatten();
            let parse = |i: usize| {
                text(i)
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .unwrap_or(Value::Null)
            };
            Ok((
                r.get::<_, i64>(0)?,
                json!({
                    "id": text(1),
                    "session_id": text(2),
                    "time_created": r.get::<_, Option<i64>>(3).ok().flatten(),
                    "role": text(4),
                    "modelID": text(5),
                    "tokens": parse(6),
                    "cost": r.get::<_, Option<f64>>(7).ok().flatten(),
                    "finish": text(8),
                    "time": parse(9),
                    "path": { "cwd": text(10) },
                    "error": text(11).is_some_and(|t| t != "null"),
                    "directory": text(12),
                    "parent_session_id": text(13),
                }),
            ))
        });
        let Ok(rows) = rows else {
            return false;
        };
        let mut still_open = Vec::new();
        for (rowid, rec) in rows.flatten() {
            self.last_rowid = self.last_rowid.max(rowid);
            let writing = rec.get("role").and_then(Value::as_str) == Some("assistant")
                && rec
                    .get("time")
                    .and_then(|t| t.get("completed"))
                    .is_none_or(Value::is_null);
            if writing
                && still_open.len() < 64
                && let Some(id) = rec.get("id").and_then(Value::as_str)
            {
                still_open.push(id.to_string());
            }
            row(&rec);
        }
        self.open = still_open;
        self.modified = stamp;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(msgs: &[Value]) -> Vec<Entry> {
        let mut st = FileState::default();
        msgs.iter().flat_map(|m| parse(m, &mut st, 0.0)).collect()
    }

    // Synthetic records with the documented OpenCode shape.
    fn user(id: &str, t: u64) -> Value {
        json!({"id": id, "sessionID": "ses_1", "role": "user", "time": {"created": t}})
    }
    fn reply(id: &str, t: u64, done: Option<u64>, finish: Option<&str>) -> Value {
        json!({"id": id, "sessionID": "ses_1", "role": "assistant", "modelID": "claude-sonnet-4-5",
            "time": {"created": t, "completed": done}, "finish": finish, "cost": 0.0123,
            "path": {"cwd": "/tmp/oc"},
            "tokens": {"input": 100, "output": 20, "reasoning": 5, "cache": {"read": 300, "write": 10}}})
    }

    #[test]
    fn turn_and_usage() {
        let e = run(&[
            user("u1", 1_792_800_000_000u64),
            reply(
                "a1",
                1_792_800_001_000u64,
                Some(1_792_800_005_000u64),
                Some("tool-calls"),
            ),
            reply(
                "a2",
                1_792_800_006_000u64,
                Some(1_792_800_009_000u64),
                Some("stop"),
            ),
            user("u1", 1_792_800_000_000u64),
        ]);
        assert_eq!(e[0], Entry::TurnBegan(1_792_800_000.0));
        let Entry::Usage {
            key,
            model,
            billable,
            ..
        } = &e[1]
        else {
            panic!("{:?}", e[1])
        };
        assert_eq!(key, "opencode:a1");
        assert_eq!(model, "claude-sonnet-4-5");
        assert_eq!(billable.tokens.output, 25);
        assert_eq!(billable.tokens.cache_read, 300);
        assert_eq!(billable.reported_cost, Some(0.0123));
        assert!(matches!(e[2], Entry::TurnActive(_)));
        assert!(matches!(
            e[4],
            Entry::TurnEnded {
                completed: true,
                ..
            }
        ));
        assert_eq!(e.len(), 5, "repeated prompt ignored: {e:?}");
    }

    #[test]
    fn legacy_file_keeps_only_needed_fields() {
        let dir = std::env::temp_dir().join(format!("nox-ocf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("msg.json");
        let mut r = reply("a1", 1, Some(2), Some("stop"));
        r["summary"] = json!({"title": "private"});
        r["path"]["root"] = json!("/secret");
        std::fs::write(&p, r.to_string()).unwrap();
        let v = read_message_file(&p).unwrap();
        assert!(v.get("summary").is_none());
        assert_eq!(v["path"], json!({"cwd": "/tmp/oc"}));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sqlite_database_is_read_incrementally() {
        let dir = std::env::temp_dir().join(format!("nox-oc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, parent_id TEXT);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO session VALUES ('ses_1', '/tmp/oc', NULL);",
        )
        .unwrap();
        let ins = |id: &str, t: i64, data: Value| {
            db.execute(
                "INSERT INTO message VALUES (?1, 'ses_1', ?2, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET data = excluded.data",
                rusqlite::params![id, t, data.to_string()],
            )
            .unwrap();
        };
        ins(
            "u1",
            1_792_800_000_000,
            json!({"role": "user", "time": {"created": 1_792_800_000_000u64}, "summary": {"title": "secret"}}),
        );
        ins(
            "a1",
            1_792_800_001_000,
            json!({"role": "assistant", "modelID": "gpt-5.4", "time": {"created": 1_792_800_001_000u64},
            "tokens": {"input": 5, "output": 1, "cache": {"read": 0, "write": 0}}}),
        );
        let mut reader = Database::new(path.clone());
        let mut st = FileState::default();
        let mut seen = Vec::new();
        assert!(reader.read_new(0, |r| {
            assert!(r.get("summary").is_none(), "prompt data never extracted");
            seen.extend(parse(r, &mut st, 0.0));
        }));
        assert_eq!(seen[0], Entry::TurnBegan(1_792_800_000.0));
        assert!(seen.iter().any(|e| matches!(e, Entry::Usage { .. })));
        // The reply completes in place: re-read by id although its rowid is old.
        ins(
            "a1",
            1_792_800_001_000,
            json!({"role": "assistant", "modelID": "gpt-5.4", "finish": "stop",
            "time": {"created": 1_792_800_001_000u64, "completed": 1_792_800_004_000u64},
            "tokens": {"input": 50, "output": 10, "cache": {"read": 0, "write": 0}}}),
        );
        reader.modified = None;
        let mut more = Vec::new();
        assert!(reader.read_new(0, |r| more.extend(parse(r, &mut st, 0.0))));
        assert!(
            more.iter().any(|e| matches!(
                e,
                Entry::TurnEnded {
                    completed: true,
                    ..
                }
            )),
            "{more:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
