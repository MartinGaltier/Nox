// Adapted from vorssaint-utils (GPL-3.0-or-later), Services/AgentUsage/Harness/ClaudeReader.swift, commit e916961.
//! Claude Code session logs (`~/.claude/projects/<project>/<session>.jsonl`
//! and `<session>/subagents/agent-*.jsonl`). A prompt opens a turn, a reply
//! keeps it going and records its usage, and an end reason, an interruption
//! or a `turn_duration` marker closes it. Only envelope fields, usage, model
//! and tool names are decoded; message text and tool output are skipped.

use super::parse::{Entry, FileState, contains, parse_iso, project_name};
use super::pricing::{Billable, Tokens};
use nox_core::model::Timestamp;
use serde::Deserialize;
use serde_json::value::RawValue;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Line<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    session_id: Option<String>,
    cwd: Option<String>,
    timestamp: Option<&'a str>,
    is_sidechain: Option<bool>,
    is_meta: Option<bool>,
    is_api_error_message: Option<bool>,
    request_id: Option<String>,
    tool_ends_turn: Option<bool>,
    subtype: Option<&'a str>,
    duration_ms: Option<f64>,
    #[serde(borrow)]
    message: Option<Message<'a>>,
}

#[derive(Deserialize)]
struct Message<'a> {
    id: Option<String>,
    model: Option<String>,
    stop_reason: Option<&'a str>,
    usage: Option<Usage>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: f64,
    #[serde(default)]
    cache_creation_input_tokens: f64,
    #[serde(default)]
    cache_read_input_tokens: f64,
    #[serde(default)]
    output_tokens: f64,
    output_tokens_details: Option<OutputDetails>,
    cache_creation: Option<CacheCreation>,
    speed: Option<String>,
    inference_geo: Option<String>,
    server_tool_use: Option<ServerTools>,
}

#[derive(Deserialize)]
struct OutputDetails {
    #[serde(default)]
    thinking_tokens: f64,
}
#[derive(Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_1h_input_tokens: f64,
}
#[derive(Deserialize)]
struct ServerTools {
    #[serde(default)]
    web_search_requests: f64,
}

#[derive(Deserialize)]
struct Block<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    name: Option<&'a str>,
    id: Option<&'a str>,
    tool_use_id: Option<&'a str>,
    /// Only user text blocks are kept, to spot interruptions by their prefix.
    text: Option<std::borrow::Cow<'a, str>>,
}

fn n(v: f64) -> u64 {
    if v.is_finite() && v > 0.0 {
        v.min(1e12) as u64
    } else {
        0
    }
}

/// Tools that hand the conversation back to the user.
const QUESTION_TOOLS: [&str; 2] = ["AskUserQuestion", "ExitPlanMode"];

pub fn parse(line: &[u8], st: &mut FileState, now: Timestamp) -> Vec<Entry> {
    let assistant = contains(line, br#""type":"assistant""#);
    let user = !assistant && contains(line, br#""type":"user""#);
    let system = !assistant && !user && contains(line, br#""subtype":"turn_duration""#);
    if !(assistant || user || system) {
        return Vec::new();
    }
    // Tool results inside an open turn can be large; while no tool call waits
    // for its result they only say that work goes on.
    if user
        && st.turn_open
        && st.pending_tools.is_empty()
        && !contains(line, b"[Request interrupted by user")
        && !contains(line, b"<local-command-std")
        && !contains(line, br#""toolEndsTurn":true"#)
    {
        return vec![Entry::TurnActive(byte_timestamp(line).unwrap_or(now))];
    }
    let Ok(l) = serde_json::from_slice::<Line>(line) else {
        return Vec::new();
    };
    let at = l.timestamp.and_then(parse_iso).unwrap_or(now);
    if let Some(s) = &l.session_id {
        st.session.clone_from(s);
    }
    if let Some(c) = &l.cwd {
        st.project = project_name(c);
    }
    let sidechain = l.is_sidechain == Some(true);
    match l.kind {
        Some("assistant") => assistant_line(l, at, sidechain, st),
        Some("user") => user_line(l, at, sidechain, st),
        Some("system") if l.subtype == Some("turn_duration") && !sidechain => {
            if !st.turn_open {
                return Vec::new();
            }
            st.turn_open = false;
            st.pending_tools.clear();
            vec![Entry::TurnEnded {
                at,
                completed: true,
                duration: l
                    .duration_ms
                    .filter(|d| d.is_finite() && *d >= 0.0)
                    .map(|d| d / 1000.0),
            }]
        }
        _ => Vec::new(),
    }
}

fn blocks<'a>(content: Option<&'a RawValue>) -> Vec<Block<'a>> {
    match content {
        Some(raw) if raw.get().starts_with('[') => {
            serde_json::from_str(raw.get()).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn assistant_line(l: Line, at: Timestamp, sidechain: bool, st: &mut FileState) -> Vec<Entry> {
    let Some(msg) = l.message else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let model = msg.model.clone().unwrap_or_default();
    if let Some(u) = &msg.usage
        && !model.is_empty()
        && !model.starts_with('<')
    {
        if !sidechain {
            st.model.clone_from(&model);
        }
        let tokens = Tokens {
            input: n(u.input_tokens),
            cache_write: n(u.cache_creation_input_tokens),
            cache_read: n(u.cache_read_input_tokens),
            output: n(u.output_tokens),
            reasoning: u
                .output_tokens_details
                .as_ref()
                .map_or(0, |d| n(d.thinking_tokens)),
        };
        let billable = Billable {
            tokens,
            aggregate: false,
            long_cache_write: u
                .cache_creation
                .as_ref()
                .map_or(0, |c| n(c.ephemeral_1h_input_tokens)),
            fast: u.speed.as_deref() == Some("fast"),
            domestic: u.inference_geo.as_deref() == Some("us"),
            web_searches: u
                .server_tool_use
                .as_ref()
                .map_or(0, |s| n(s.web_search_requests)),
        };
        let id = msg.id.clone().unwrap_or_default();
        let req = l.request_id.clone().unwrap_or_default();
        let key = if id.is_empty() && req.is_empty() {
            format!("claude:{}:{at}", st.session)
        } else {
            format!("claude:{id}:{req}")
        };
        out.push(Entry::Usage {
            key,
            at,
            model,
            billable,
        });
    }
    // A subagent's own ending is not the end of the turn it serves.
    if sidechain {
        out.push(Entry::TurnActive(at));
        return out;
    }
    match msg.stop_reason {
        Some("end_turn" | "stop_sequence" | "max_tokens" | "refusal") => {
            let failed = l.is_api_error_message == Some(true)
                || msg.model.as_deref().is_some_and(|m| m.starts_with('<'));
            if st.turn_open {
                out.push(Entry::TurnEnded {
                    at,
                    completed: !failed,
                    duration: None,
                });
            }
            st.turn_open = false;
            st.pending_tools.clear();
        }
        _ => {
            // Ahead of the usage, so a reply that opens the turn again counts toward it.
            out.insert(
                0,
                if st.turn_open {
                    Entry::TurnActive(at)
                } else {
                    Entry::TurnBegan(at)
                },
            );
            st.turn_open = true;
            for b in blocks(msg.content) {
                if b.kind != Some("tool_use") {
                    continue;
                }
                let name = b.name.unwrap_or("outil");
                if let Some(id) = b.id
                    && st.pending_tools.len() < 64
                {
                    st.pending_tools.push(id.to_string());
                }
                if QUESTION_TOOLS.contains(&name) {
                    out.push(Entry::Question {
                        at,
                        what: name.to_string(),
                    });
                } else {
                    out.push(Entry::Tool {
                        at,
                        name: name.to_string(),
                    });
                }
            }
        }
    }
    out
}

fn user_line(l: Line, at: Timestamp, sidechain: bool, st: &mut FileState) -> Vec<Entry> {
    if sidechain || l.is_meta == Some(true) {
        return Vec::new();
    }
    let content = l.message.as_ref().and_then(|m| m.content);
    let is_string = content.is_some_and(|c| c.get().starts_with('"'));
    let bl = blocks(content);
    let ends = |t: &str| {
        t.starts_with("[Request interrupted by user") || t.starts_with("<local-command-std")
    };
    let text_ends = if is_string {
        content
            .and_then(|c| serde_json::from_str::<std::borrow::Cow<str>>(c.get()).ok())
            .is_some_and(|t| ends(&t))
    } else {
        bl.iter()
            .any(|b| b.kind == Some("text") && b.text.as_deref().is_some_and(ends))
    };
    if text_ends {
        let open = st.turn_open;
        st.turn_open = false;
        st.pending_tools.clear();
        return if open {
            vec![Entry::TurnEnded {
                at,
                completed: false,
                duration: None,
            }]
        } else {
            Vec::new()
        };
    }
    // A tool that ends the turn (scripted structured output) gets no reply.
    if l.tool_ends_turn == Some(true) {
        let open = st.turn_open;
        st.turn_open = false;
        st.pending_tools.clear();
        return if open {
            vec![Entry::TurnEnded {
                at,
                completed: true,
                duration: None,
            }]
        } else {
            Vec::new()
        };
    }
    let results: Vec<&str> = bl
        .iter()
        .filter(|b| b.kind == Some("tool_result"))
        .filter_map(|b| b.tool_use_id)
        .collect();
    if st.turn_open {
        let mut out = vec![Entry::TurnActive(at)];
        if !results.is_empty() {
            let before = st.pending_tools.len();
            st.pending_tools
                .retain(|id| !results.contains(&id.as_str()));
            if before > 0 && st.pending_tools.is_empty() {
                out.push(Entry::ToolsDone(at));
            }
        } else {
            // A prompt inside an open turn (session resumed mid-command).
            st.pending_tools.clear();
        }
        return out;
    }
    if !results.is_empty() {
        // A result after the turn closed (offline end): work goes on.
        st.turn_open = true;
        return vec![Entry::TurnBegan(at)];
    }
    st.turn_open = true;
    st.pending_tools.clear();
    vec![Entry::TurnBegan(at)]
}

/// The envelope's time without decoding a large line.
fn byte_timestamp(line: &[u8]) -> Option<Timestamp> {
    const KEY: &[u8] = br#""timestamp":""#;
    let at = memchr::memmem::rfind(line, KEY)? + KEY.len();
    let end = at + memchr::memchr(b'"', line.get(at..(at + 40).min(line.len()))?)?;
    parse_iso(std::str::from_utf8(&line[at..end]).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic lines written for these tests; no real transcript content.
    const PROMPT: &str = r#"{"type":"user","sessionId":"s1","cwd":"/tmp/demo","timestamp":"2026-10-24T10:00:00.000Z","message":{"role":"user","content":"hello"}}"#;
    const TOOL: &str = r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-10-24T10:00:05.000Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-5-5","stop_reason":"tool_use","usage":{"input_tokens":10,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000,"output_tokens":50,"cache_creation":{"ephemeral_1h_input_tokens":40}},"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
    const RESULT: &str = r#"{"type":"user","sessionId":"s1","timestamp":"2026-10-24T10:00:09.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]}}"#;
    const END: &str = r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-10-24T10:01:00.000Z","requestId":"r2","message":{"id":"m2","model":"claude-opus-5-5","stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":7,"speed":"fast"},"content":[{"type":"text","text":"done"}]}}"#;
    const DURATION: &str = r#"{"type":"system","subtype":"turn_duration","durationMs":60000,"sessionId":"s1","timestamp":"2026-10-24T10:01:00.100Z"}"#;

    fn run(lines: &[&str]) -> (Vec<Entry>, FileState) {
        let mut st = FileState::default();
        let mut out = Vec::new();
        for l in lines {
            out.extend(parse(l.as_bytes(), &mut st, 0.0));
        }
        (out, st)
    }

    #[test]
    fn full_turn() {
        let (e, st) = run(&[PROMPT, TOOL, RESULT, END, DURATION]);
        let t0 = parse_iso("2026-10-24T10:00:00.000Z").unwrap();
        assert_eq!(e[0], Entry::TurnBegan(t0));
        assert!(matches!(e[1], Entry::TurnActive(_)));
        let Entry::Usage {
            key,
            model,
            billable,
            ..
        } = &e[2]
        else {
            panic!("{:?}", e[2])
        };
        assert_eq!(key, "claude:m1:r1");
        assert_eq!(model, "claude-opus-5-5");
        assert_eq!(billable.tokens.total(), 1160);
        assert_eq!(billable.long_cache_write, 40);
        assert!(matches!(&e[3], Entry::Tool { name, .. } if name == "Bash"));
        assert!(matches!(e[4], Entry::TurnActive(_)));
        assert!(matches!(e[5], Entry::ToolsDone(_)));
        assert!(matches!(&e[6], Entry::Usage { billable, .. } if billable.fast));
        assert!(matches!(
            e[7],
            Entry::TurnEnded {
                completed: true,
                ..
            }
        ));
        assert_eq!(
            e.len(),
            8,
            "turn_duration after end_turn adds nothing: {e:?}"
        );
        assert_eq!(st.project, "demo");
        assert_eq!(st.session, "s1");
        assert!(!st.turn_open);
    }

    #[test]
    fn turn_duration_closes_with_duration() {
        let (e, _) = run(&[PROMPT, TOOL, DURATION]);
        assert!(matches!(
            e.last(),
            Some(Entry::TurnEnded { completed: true, duration: Some(d), .. }) if *d == 60.0
        ));
    }

    #[test]
    fn interruption_and_questions() {
        let ask = r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-10-24T10:00:05.000Z","message":{"id":"m3","model":"claude-opus-5-5","stop_reason":"tool_use","usage":{"input_tokens":1,"output_tokens":1},"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{}}]}}"#;
        let stop = r#"{"type":"user","sessionId":"s1","timestamp":"2026-10-24T10:00:30.000Z","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}"#;
        let (e, st) = run(&[PROMPT, ask, stop]);
        assert!(
            e.iter()
                .any(|x| matches!(x, Entry::Question { what, .. } if what == "AskUserQuestion"))
        );
        assert!(matches!(
            e.last(),
            Some(Entry::TurnEnded {
                completed: false,
                ..
            })
        ));
        assert!(!st.turn_open);
    }

    #[test]
    fn sidechain_counts_usage_but_not_turns() {
        let side = r#"{"type":"assistant","isSidechain":true,"sessionId":"s1","timestamp":"2026-10-24T10:00:05.000Z","message":{"id":"m9","model":"claude-haiku-5-5","stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":4}}}"#;
        let (e, st) = run(&[side]);
        assert!(matches!(e[0], Entry::Usage { .. }));
        assert!(matches!(e[1], Entry::TurnActive(_)));
        assert!(!st.turn_open);
        assert_eq!(
            st.model, "",
            "subagent model does not replace the session's"
        );
    }

    #[test]
    fn ignores_noise_and_api_errors() {
        let (e, _) = run(&[
            r#"{"type":"permission-mode","permissionMode":"default","sessionId":"s1"}"#,
            "not json",
            r#"{"type":"assistant","message":{"model":"<synthetic>","stop_reason":"end_turn","usage":{"input_tokens":0}}}"#,
        ]);
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn big_tool_results_are_not_decoded() {
        let (_, mut st) = run(&[PROMPT]);
        let huge = format!(
            r#"{{"type":"user","timestamp":"2026-10-24T10:00:09.000Z","message":{{"content":[{{"type":"tool_result","content":"{}"}}]}}}}"#,
            "x".repeat(1 << 20)
        );
        let e = parse(huge.as_bytes(), &mut st, 0.0);
        assert_eq!(
            e,
            vec![Entry::TurnActive(
                parse_iso("2026-10-24T10:00:09.000Z").unwrap()
            )]
        );
    }
}
