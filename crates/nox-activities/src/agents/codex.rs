// Adapted from vorssaint-utils (GPL-3.0-or-later), Services/AgentUsage/Harness/CodexReader.swift
// and AgentLogParser.swift (codexWindows), commit e916961.
//! Codex rollouts (`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`). Token
//! records and running totals become usage, task events open and close turns,
//! and `rate_limits` readings become the account's limit windows — measured
//! locally, without any network request.

use super::parse::{
    Entry, FileState, codex_tokens, first_type, object, project_name, seconds, str_of, timestamp,
};
use super::pricing::Billable;
use nox_core::Reading;
use nox_core::model::Timestamp;
use nox_core::model::agents::{LimitKind, LimitSource, LimitWindow};
use serde_json::Value;

const EVENTS: [&str; 8] = [
    "token_count",
    "task_started",
    "task_complete",
    "turn_aborted",
    "thread_settings_applied",
    "exec_command_begin",
    "patch_apply_begin",
    "mcp_tool_call_begin",
];

pub fn parse(line: &[u8], st: &mut FileState, now: Timestamp) -> Vec<Entry> {
    let Some((kind, end)) = first_type(line, 0) else {
        return Vec::new();
    };
    match kind {
        "token_usage_record" | "turn_context" | "session_meta" => {}
        "event_msg" => match first_type(line, end) {
            Some((ev, _)) if EVENTS.contains(&ev) => {}
            _ => return Vec::new(),
        },
        // Tool calls name the current activity; their output is never read.
        "response_item" => {
            return match first_type(line, end) {
                Some((t @ ("function_call" | "custom_tool_call" | "local_shell_call"), _)) => {
                    tool_call(line, t, st, now)
                }
                _ => Vec::new(),
            };
        }
        _ => return Vec::new(),
    }
    let Some(json) = object(line) else {
        return Vec::new();
    };
    let Some(p) = json.get("payload").filter(|p| p.is_object()) else {
        return Vec::new();
    };
    let at = timestamp(json.get("timestamp")).unwrap_or(now);
    match json.get("type").and_then(Value::as_str) {
        Some("session_meta") => {
            let id = str_of(p, "id").unwrap_or_default().to_string();
            if !id.is_empty() {
                st.codex_totals_session.clone_from(&id);
            }
            // History copied from another thread repeats that thread's metadata.
            if st.codex_header_read {
                return Vec::new();
            }
            st.codex_header_read = true;
            if !id.is_empty() {
                st.session = id;
            }
            if let Some(cwd) = str_of(p, "cwd") {
                st.project = project_name(cwd);
            }
            // Subagents, reviews and memory tasks Codex starts itself.
            let src = p.get("source");
            st.codex_subagent =
                src.is_some_and(|s| s.get("subagent").is_some() || s.get("internal").is_some());
            Vec::new()
        }
        Some("turn_context") => {
            if let Some(m) = str_of(p, "model") {
                st.model = m.to_string();
            }
            if let Some(cwd) = str_of(p, "cwd") {
                st.project = project_name(cwd);
            }
            if let Some(t) = str_of(p, "service_tier") {
                st.fast = fast_tier(t);
            }
            Vec::new()
        }
        Some("token_usage_record") => {
            let Some(u) = p.get("usage") else {
                return Vec::new();
            };
            st.saw_usage_records = true;
            if st.session.is_empty()
                && let Some(s) = str_of(p, "session_id")
            {
                st.session = s.to_string();
            }
            let key = match str_of(p, "response_id") {
                Some(r) => format!("codex:{r}"),
                None => format!("codex:{}:{at}", st.session),
            };
            vec![usage(codex_tokens(u), key, at, st)]
        }
        Some("event_msg") => event(p, at, st),
        _ => Vec::new(),
    }
}

fn fast_tier(t: &str) -> bool {
    matches!(t.to_lowercase().as_str(), "fast" | "priority")
}

fn usage(tokens: super::pricing::Tokens, key: String, at: Timestamp, st: &FileState) -> Entry {
    Entry::Usage {
        key,
        at,
        model: st.model.clone(),
        billable: Billable {
            tokens,
            fast: st.fast,
            ..Default::default()
        },
    }
}

fn tool_call(line: &[u8], kind: &str, st: &mut FileState, now: Timestamp) -> Vec<Entry> {
    if !st.turn_open || st.codex_subagent {
        return Vec::new();
    }
    // Arguments can be large but the line is bounded by the reader; only the
    // name is kept.
    let Some(json) = object(line) else {
        return Vec::new();
    };
    let at = timestamp(json.get("timestamp")).unwrap_or(now);
    let name = json
        .get("payload")
        .and_then(|p| str_of(p, "name"))
        .unwrap_or(if kind == "local_shell_call" {
            "shell"
        } else {
            "outil"
        });
    vec![Entry::Tool {
        at,
        name: name.chars().take(40).collect(),
    }]
}

fn event(p: &Value, at: Timestamp, st: &mut FileState) -> Vec<Entry> {
    match p.get("type").and_then(Value::as_str) {
        Some("token_count") => {
            let mut out = Vec::new();
            if let Some(rl) = p.get("rate_limits").filter(|v| v.is_object()) {
                if is_main_bucket(rl) {
                    let w = windows(rl, at);
                    if !w.is_empty() {
                        out.push(Entry::Limits { windows: w, at });
                    }
                }
                if let Some(plan) = str_of(rl, "plan_type") {
                    out.push(Entry::Plan {
                        plan: plan.to_string(),
                        at,
                    });
                }
                if let Some(c) = rl.get("credits").filter(|c| c.is_object()) {
                    let text = if c.get("unlimited").and_then(Value::as_bool) == Some(true) {
                        Some("crédits illimités".to_string())
                    } else if c.get("has_credits").and_then(Value::as_bool) == Some(true) {
                        str_of(c, "balance").map(|b| {
                            format!("crédits : {}", b.chars().take(16).collect::<String>())
                        })
                    } else {
                        None
                    };
                    if let Some(text) = text {
                        out.push(Entry::Credits { text, at });
                    }
                }
            }
            // Running totals repeat when only the limits changed; a response
            // is whatever the total grew by since the previous reading.
            if !st.saw_usage_records
                && let Some(total) = p
                    .get("info")
                    .and_then(|i| i.get("total_token_usage"))
                    .map(codex_tokens)
            {
                let prev = st.last_total.unwrap_or_default();
                if total.total() > prev.total() {
                    let delta = p
                        .get("info")
                        .and_then(|i| i.get("last_token_usage"))
                        .map(codex_tokens)
                        .filter(|d| d.total() > 0)
                        .unwrap_or_else(|| total.saturating_sub(&prev));
                    let key = format!("codex:{}:total:{}", st.codex_totals_session, total.total());
                    out.push(usage(delta, key, at, st));
                }
                st.last_total = Some(total);
            }
            out
        }
        Some("thread_settings_applied") => {
            if let Some(t) = p
                .get("thread_settings")
                .and_then(|s| str_of(s, "service_tier"))
            {
                st.fast = fast_tier(t);
            }
            Vec::new()
        }
        Some("task_started") if !st.codex_subagent => {
            st.turn_open = true;
            vec![Entry::TurnBegan(seconds(p.get("started_at")).unwrap_or(at))]
        }
        Some(t @ ("task_complete" | "turn_aborted")) if !st.codex_subagent => {
            let open = st.turn_open;
            st.turn_open = false;
            if !open {
                return Vec::new();
            }
            let duration = p
                .get("duration_ms")
                .and_then(Value::as_f64)
                .filter(|d| d.is_finite() && *d >= 0.0)
                .map(|d| d / 1000.0);
            vec![Entry::TurnEnded {
                at: seconds(p.get("completed_at")).unwrap_or(at),
                completed: t == "task_complete",
                duration,
            }]
        }
        Some(t @ ("exec_command_begin" | "patch_apply_begin" | "mcp_tool_call_begin"))
            if st.turn_open && !st.codex_subagent =>
        {
            let name = match t {
                "exec_command_begin" => "shell",
                "patch_apply_begin" => "apply_patch",
                _ => "mcp",
            };
            vec![Entry::Tool {
                at,
                name: name.into(),
            }]
        }
        _ => Vec::new(),
    }
}

/// Only the main allowance ("codex", or unnamed in older logs) is kept, so a
/// model's own windows never replace it or warn again.
fn is_main_bucket(rl: &Value) -> bool {
    let id = rl
        .get("limit_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    id.is_empty() || id == "codex"
}

pub fn kind(minutes: Option<u32>) -> LimitKind {
    match minutes {
        Some(m) if m > 0 && m <= 12 * 60 => LimitKind::Session,
        Some(m) if (6 * 1440..=8 * 1440).contains(&m) => LimitKind::Weekly,
        _ => LimitKind::Other,
    }
}

pub fn window_name(kind: LimitKind, minutes: Option<u32>) -> String {
    match (kind, minutes) {
        (LimitKind::Session, Some(m)) if m % 60 == 0 => format!("{} h", m / 60),
        (LimitKind::Session, Some(m)) => format!("{m} min"),
        (LimitKind::Weekly, _) => "Semaine".into(),
        (_, Some(m)) if m >= 1440 => format!("{} j", m / 1440),
        _ => "Limite".into(),
    }
}

/// Windows are told apart by their length, never by their slot.
pub fn windows(rl: &Value, observed: Timestamp) -> Vec<LimitWindow> {
    let mut out = Vec::new();
    for slot in ["primary", "secondary"] {
        let Some(w) = rl.get(slot).filter(|w| w.is_object()) else {
            continue;
        };
        let Some(used) = w
            .get("used_percent")
            .and_then(Value::as_f64)
            .filter(|u| u.is_finite())
        else {
            continue;
        };
        let minutes = w
            .get("window_minutes")
            .and_then(Value::as_u64)
            .map(|m| m.min(u64::from(u32::MAX)) as u32);
        let resets = seconds(w.get("resets_at")).or_else(|| {
            w.get("resets_in_seconds")
                .and_then(Value::as_f64)
                .filter(|d| d.is_finite())
                .map(|d| observed + d.max(0.0))
        });
        let k = kind(minutes);
        out.push(LimitWindow {
            name: window_name(k, minutes),
            used_percent: Reading::Measured(used.clamp(0.0, 100.0)),
            resets_at: resets,
            kind: k,
            window_minutes: minutes,
            scope: None,
            source: LimitSource::SessionLog,
            observed_at: observed,
            resets_estimated: false,
        });
    }
    if out.is_empty()
        && let Some(ind) = rl.get("individual_limit").filter(|v| v.is_object())
        && let Some(rem) = ind
            .get("remaining_percent")
            .and_then(Value::as_f64)
            .filter(|r| r.is_finite())
    {
        out.push(LimitWindow {
            name: "Limite".into(),
            used_percent: Reading::Measured((100.0 - rem).clamp(0.0, 100.0)),
            resets_at: seconds(ind.get("resets_at")),
            kind: LimitKind::Other,
            window_minutes: None,
            scope: None,
            source: LimitSource::SessionLog,
            observed_at: observed,
            resets_estimated: false,
        });
    }
    out.sort_by_key(|w| w.window_minutes.unwrap_or(u32::MAX));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic rollout lines (structure only; no real content).
    const META: &str = r#"{"timestamp":"2026-10-24T09:00:00.000Z","type":"session_meta","payload":{"id":"th1","cwd":"/tmp/proj","source":"cli"}}"#;
    const CTX: &str = r#"{"timestamp":"2026-10-24T09:00:01.000Z","type":"turn_context","payload":{"model":"gpt-5.4","cwd":"/tmp/proj","service_tier":"priority"}}"#;
    const START: &str = r#"{"timestamp":"2026-10-24T09:00:02.000Z","type":"event_msg","payload":{"type":"task_started","started_at":1792832402}}"#;
    const CALL: &str = r#"{"timestamp":"2026-10-24T09:00:03.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{}"}}"#;
    const COUNT: &str = r#"{"timestamp":"2026-10-24T09:00:04.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":600,"output_tokens":50,"reasoning_output_tokens":20},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":600,"output_tokens":50}},"rate_limits":{"limit_id":"codex","plan_type":"plus","primary":{"used_percent":42.5,"window_minutes":300,"resets_at":1792850000},"secondary":{"used_percent":12.0,"window_minutes":10080,"resets_at":1793400000},"credits":{"has_credits":false,"unlimited":false,"balance":"0"}}}}"#;
    const COUNT_AGAIN: &str = r#"{"timestamp":"2026-10-24T09:00:05.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":600,"output_tokens":50}},"rate_limits":{"limit_id":"codex_other","primary":{"used_percent":99,"window_minutes":300}}}}"#;
    const DONE: &str = r#"{"timestamp":"2026-10-24T09:01:00.000Z","type":"event_msg","payload":{"type":"task_complete","completed_at":1792832460,"duration_ms":58000}}"#;

    fn run(lines: &[&str]) -> (Vec<Entry>, FileState) {
        let mut st = FileState::default();
        let mut out = Vec::new();
        for l in lines {
            out.extend(parse(l.as_bytes(), &mut st, 0.0));
        }
        (out, st)
    }

    #[test]
    fn turn_usage_and_limits() {
        let (e, st) = run(&[META, CTX, START, CALL, COUNT, COUNT_AGAIN, DONE]);
        assert_eq!(st.session, "th1");
        assert_eq!(st.project, "proj");
        assert!(st.fast);
        assert_eq!(e[0], Entry::TurnBegan(1_792_832_402.0));
        assert!(matches!(&e[1], Entry::Tool { name, .. } if name == "exec_command"));
        let Entry::Limits { windows, .. } = &e[2] else {
            panic!("{:?}", e[2])
        };
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, LimitKind::Session);
        assert_eq!(windows[0].name, "5 h");
        assert_eq!(windows[0].used_percent, Reading::Measured(42.5));
        assert_eq!(windows[0].resets_at, Some(1_792_850_000.0));
        assert_eq!(windows[1].kind, LimitKind::Weekly);
        assert!(matches!(&e[3], Entry::Plan { plan, .. } if plan == "plus"));
        let Entry::Usage {
            model, billable, ..
        } = &e[4]
        else {
            panic!("{:?}", e[4])
        };
        assert_eq!(model, "gpt-5.4");
        assert_eq!(billable.tokens.input, 400);
        assert_eq!(billable.tokens.cache_read, 600);
        assert!(billable.fast);
        // Same total again + another bucket: no usage, no limits.
        assert!(matches!(
            e[5],
            Entry::TurnEnded {
                completed: true,
                duration: Some(58.0),
                ..
            }
        ));
        assert_eq!(e.len(), 6, "{e:?}");
    }

    #[test]
    fn usage_records_replace_totals() {
        let rec = r#"{"timestamp":"2026-10-24T09:00:04.000Z","type":"token_usage_record","payload":{"response_id":"resp_1","session_id":"th1","usage":{"input_tokens":10,"output_tokens":2}}}"#;
        let (e, _) = run(&[META, CTX, rec, COUNT]);
        let usages: Vec<_> = e
            .iter()
            .filter_map(|x| match x {
                Entry::Usage { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(usages, vec!["codex:resp_1".to_string()]);
    }

    #[test]
    fn subagents_own_no_turn() {
        let meta = r#"{"timestamp":"2026-10-24T09:00:00.000Z","type":"session_meta","payload":{"id":"th2","source":{"subagent":"review"}}}"#;
        let (e, _) = run(&[meta, START, DONE]);
        assert!(e.is_empty());
    }

    #[test]
    fn individual_limit_and_unknown_lines() {
        let rl = serde_json::json!({"individual_limit":{"remaining_percent":30.0}});
        let w = windows(&rl, 0.0);
        assert_eq!(w[0].used_percent, Reading::Measured(70.0));
        let (e, _) = run(&[
            r#"{"type":"response_item","payload":{"type":"message","content":"zzz"}}"#,
            "{bad",
        ]);
        assert!(e.is_empty());
    }
}
