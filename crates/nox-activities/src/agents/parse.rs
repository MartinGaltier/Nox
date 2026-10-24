// Adapted from vorssaint-utils (GPL-3.0-or-later), Services/AgentUsage/AgentLogParser.swift, commit e916961.
//! What one log line says, reduced to the few facts Nox keeps: usage
//! counters, model names, times, folder names and tool names. Prompts,
//! replies and tool output are never decoded into anything that is stored.

use super::pricing::{Billable, Tokens};
use nox_core::model::Timestamp;
use nox_core::model::agents::LimitWindow;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// `key` identifies the response across duplicate lines and files.
    Usage {
        key: String,
        at: Timestamp,
        model: String,
        billable: Billable,
    },
    /// Limits as the agent copied them into its log (Codex).
    Limits {
        windows: Vec<LimitWindow>,
        at: Timestamp,
    },
    Plan {
        plan: String,
        at: Timestamp,
    },
    /// Account credit balance (Codex), shown as a note.
    Credits {
        text: String,
        at: Timestamp,
    },
    TurnBegan(Timestamp),
    TurnActive(Timestamp),
    TurnEnded {
        at: Timestamp,
        completed: bool,
        duration: Option<f64>,
    },
    /// A tool call started (name only).
    Tool {
        at: Timestamp,
        name: String,
    },
    /// The agent asked the user something (question tool, plan approval).
    Question {
        at: Timestamp,
        what: String,
    },
    /// All tool calls of the turn returned.
    ToolsDone(Timestamp),
}

/// Per-file context carried from line to line.
#[derive(Debug, Clone, Default)]
pub struct FileState {
    pub session: String,
    pub project: String,
    pub model: String,
    pub turn_open: bool,
    /// Tool calls still waiting for their result, by id.
    pub pending_tools: Vec<String>,
    // Codex
    pub codex_header_read: bool,
    pub codex_subagent: bool,
    pub saw_usage_records: bool,
    pub last_total: Option<Tokens>,
    pub fast: bool,
    pub codex_totals_session: String,
    // Copilot
    pub copilot_totals: std::collections::HashMap<String, Tokens>,
    pub copilot_final: bool,
    pub copilot_turn: Option<String>,
}

/// Structural keys are written without spaces and never appear unescaped
/// inside a string value, so a byte match is a safe first filter.
pub fn contains(line: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(line, needle).is_some()
}

/// The first `"type":"…"` value at or after `start` (within 64 bytes of the key).
pub fn first_type(line: &[u8], start: usize) -> Option<(&str, usize)> {
    const KEY: &[u8] = br#""type":""#;
    let hay = line.get(start..)?;
    let at = start + memchr::memmem::find(hay, KEY)? + KEY.len();
    let limit = line.len().min(at + 64);
    let end = at + memchr::memchr(b'"', &line[at..limit])?;
    Some((std::str::from_utf8(&line[at..end]).ok()?, end + 1))
}

pub fn object(line: &[u8]) -> Option<Value> {
    let v: Value = serde_json::from_slice(line).ok()?;
    v.is_object().then_some(v)
}

pub fn int(v: Option<&Value>) -> u64 {
    match v.and_then(Value::as_f64) {
        // A damaged line must not overflow the sums it joins.
        Some(d) if d.is_finite() && d > 0.0 => d.min(1e12) as u64,
        _ => 0,
    }
}

pub fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Unix seconds, or milliseconds from agents that write those.
pub fn seconds(v: Option<&Value>) -> Option<Timestamp> {
    let raw = v?.as_f64()?;
    (raw.is_finite() && raw > 0.0).then(|| if raw > 1e11 { raw / 1000.0 } else { raw })
}

pub fn timestamp(v: Option<&Value>) -> Option<Timestamp> {
    match v? {
        Value::String(s) => parse_iso(s),
        other => seconds(Some(other)),
    }
}

/// "2026-09-21T23:42:45.078Z" or with a "+hh:mm" offset, without a formatter.
pub fn parse_iso(s: &str) -> Option<Timestamp> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let mut v = 0i64;
        for &c in &b[r] {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + i64::from(c - b'0');
        }
        Some(v)
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut i = 19;
    let mut frac = 0.0;
    if b[i] == b'.' {
        let mut scale = 0.1;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            frac += f64::from(b[i] - b'0') * scale;
            scale /= 10.0;
            i += 1;
        }
    }
    let offset = match b.get(i)? {
        b'Z' if i == b.len() - 1 => 0,
        c @ (b'+' | b'-') if b.len() == i + 6 && b[i + 3] == b':' => {
            let o = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
            if *c == b'+' { o } else { -o }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3600 + minute * 60 + second - offset) as f64 + frac)
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The folder an agent ran in names the project; a worktree kept inside the
/// repository (`.claude/worktrees`, `.worktrees`) still belongs to it.
pub fn project_name(path: &str) -> String {
    let mut p = path;
    for marker in ["/.claude/worktrees/", "/.worktrees/"] {
        if let Some(i) = p.find(marker) {
            p = &p[..i];
        }
    }
    let p = p.trim_end_matches('/');
    p.rsplit('/').next().unwrap_or(p).chars().take(80).collect()
}

/// Codex input counts include what came from the cache.
pub fn codex_tokens(u: &Value) -> Tokens {
    let input = int(u.get("input_tokens"));
    let cached = input.min(int(u.get("cached_input_tokens")));
    let written = (input - cached).min(int(u.get("cache_write_input_tokens")));
    Tokens {
        input: input - cached - written,
        cache_write: written,
        cache_read: cached,
        output: int(u.get("output_tokens")),
        reasoning: int(u.get("reasoning_output_tokens")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_iso("2026-10-24T12:00:00.500Z"), Some(1_792_843_200.5));
        assert_eq!(
            parse_iso("2026-10-24T14:00:00+02:00"),
            parse_iso("2026-10-24T12:00:00Z")
        );
        assert_eq!(parse_iso("garbage"), None);
        assert_eq!(parse_iso("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn projects_and_types() {
        assert_eq!(project_name("/Users/x/Code/nox/"), "nox");
        assert_eq!(project_name("/Users/x/Code/nox/.worktrees/agent"), "nox");
        assert_eq!(project_name("/Users/x/r/.claude/worktrees/w1"), "r");
        let l = br#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count"}}"#;
        let (t, end) = first_type(l, 0).unwrap();
        assert_eq!(t, "event_msg");
        assert_eq!(first_type(l, end).unwrap().0, "token_count");
        assert_eq!(
            seconds(Some(&serde_json::json!(1_700_000_000_000u64))),
            Some(1.7e9)
        );
        assert_eq!(int(Some(&serde_json::json!(-4))), 0);
    }
}
