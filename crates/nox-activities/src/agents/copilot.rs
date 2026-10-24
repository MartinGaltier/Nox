// Adapted from vorssaint-utils (GPL-3.0-or-later), Services/AgentUsage/Harness/CopilotReader.swift, commit e916961.
//! GitHub Copilot CLI / app event log (`~/.copilot/session-state/<id>/events.jsonl`).
//! Only session context, turn boundaries, tool names and the cumulative
//! model counters written at `session.shutdown` are decoded; prompts,
//! replies, reasoning and tool arguments are ignored. Token counts are
//! therefore known only once a session ends.

use super::parse::{Entry, FileState, int, object, project_name, str_of, timestamp};
use super::pricing::{Billable, Tokens};
use nox_core::model::Timestamp;
use serde_json::Value;

const EVENTS: [&[u8]; 9] = [
    br#""type":"user.message""#,
    br#""type":"assistant.turn_start""#,
    br#""type":"assistant.message""#,
    br#""type":"assistant.turn_end""#,
    br#""type":"session.start""#,
    br#""type":"session.context_changed""#,
    br#""type":"session.model_change""#,
    br#""type":"session.shutdown""#,
    br#""type":"abort""#,
];

fn context_path(v: &Value) -> Option<&str> {
    ["gitRoot", "cwd", "repository"]
        .iter()
        .find_map(|k| str_of(v, k))
}

pub fn parse(line: &[u8], st: &mut FileState, now: Timestamp) -> Vec<Entry> {
    if !EVENTS.iter().any(|e| super::parse::contains(line, e)) {
        return Vec::new();
    }
    let Some(env) = object(line) else {
        return Vec::new();
    };
    let Some(kind) = str_of(&env, "type") else {
        return Vec::new();
    };
    let Some(data) = env.get("data").filter(|d| d.is_object()) else {
        return Vec::new();
    };
    // Subagents share the root log; only their messages count as activity.
    let root = str_of(&env, "agentId").is_none();
    if !root && kind != "assistant.message" {
        return Vec::new();
    }
    let at = timestamp(env.get("timestamp")).unwrap_or(now);
    match kind {
        "user.message" | "assistant.turn_start" => {
            st.copilot_final = false;
            st.copilot_turn = (kind == "assistant.turn_start")
                .then(|| str_of(data, "turnId").map(str::to_string))
                .flatten();
            if let Some(m) = str_of(data, "model") {
                st.model = m.to_string();
            }
            let e = if st.turn_open {
                Entry::TurnActive(at)
            } else {
                Entry::TurnBegan(at)
            };
            st.turn_open = true;
            vec![e]
        }
        "assistant.message" => {
            let mut out = Vec::new();
            let tools = data.get("toolRequests").and_then(Value::as_array);
            if root {
                if let Some(m) = str_of(data, "model") {
                    st.model = m.to_string();
                }
                st.copilot_final = tools.is_none_or(Vec::is_empty)
                    && !matches!(str_of(data, "phase"), Some("thinking" | "commentary"));
            }
            if st.turn_open {
                out.push(Entry::TurnActive(at));
                if let Some(name) = tools
                    .and_then(|t| t.first())
                    .and_then(|t| str_of(t, "name"))
                {
                    out.push(Entry::Tool {
                        at,
                        name: name.chars().take(40).collect(),
                    });
                }
            }
            out
        }
        "assistant.turn_end" => {
            if !st.turn_open
                || st
                    .copilot_turn
                    .as_deref()
                    .is_some_and(|t| Some(t) != str_of(data, "turnId"))
            {
                return Vec::new();
            }
            // Each tool iteration has its own turn_end; only a final answer finishes.
            if !st.copilot_final {
                return vec![Entry::TurnActive(at)];
            }
            st.turn_open = false;
            st.copilot_final = false;
            vec![Entry::TurnEnded {
                at,
                completed: true,
                duration: None,
            }]
        }
        "session.start" => {
            if let Some(s) = str_of(data, "sessionId") {
                st.session = s.to_string();
            }
            if let Some(m) = str_of(data, "selectedModel") {
                st.model = m.to_string();
            }
            if let Some(p) = data.get("context").and_then(context_path) {
                st.project = project_name(p);
            }
            Vec::new()
        }
        "session.context_changed" => {
            if let Some(p) = context_path(data) {
                st.project = project_name(p);
            }
            Vec::new()
        }
        "session.model_change" => {
            if let Some(m) = str_of(data, "newModel") {
                st.model = m.to_string();
            }
            Vec::new()
        }
        "abort" => {
            if !st.turn_open {
                return Vec::new();
            }
            st.turn_open = false;
            vec![Entry::TurnEnded {
                at,
                completed: false,
                duration: None,
            }]
        }
        "session.shutdown" => {
            let mut out = Vec::new();
            if let Some(metrics) = data.get("modelMetrics").and_then(Value::as_object) {
                let event = str_of(&env, "id")
                    .map(str::to_string)
                    .unwrap_or_else(|| at.to_string());
                for (model, m) in metrics {
                    let total = tokens(m);
                    // Counters are cumulative for the life of a session.
                    let prev = st.copilot_totals.get(model).copied();
                    let growth = match prev {
                        Some(p) if total.total() >= p.total() => total.saturating_sub(&p),
                        _ => total,
                    };
                    st.copilot_totals.insert(model.clone(), total);
                    if growth.total() == 0 {
                        continue;
                    }
                    out.push(Entry::Usage {
                        key: format!("copilot:{}:{event}:{model}", st.session),
                        at,
                        model: model.clone(),
                        billable: Billable {
                            tokens: growth,
                            aggregate: true,
                            ..Default::default()
                        },
                    });
                }
            }
            if st.turn_open {
                st.turn_open = false;
                out.push(Entry::TurnEnded {
                    at,
                    completed: false,
                    duration: None,
                });
            }
            out
        }
        _ => Vec::new(),
    }
}

fn tokens(m: &Value) -> Tokens {
    let details = m.get("tokenDetails");
    let usage = m.get("usage");
    let count = |detail: &str, fallback: &str| -> u64 {
        details
            .and_then(|d| d.get(detail))
            .and_then(|d| d.get("tokenCount"))
            .map(|v| int(Some(v)))
            .unwrap_or_else(|| int(usage.and_then(|u| u.get(fallback))))
    };
    Tokens {
        input: count("input", "inputTokens"),
        cache_write: count("cache_write", "cacheWriteTokens"),
        cache_read: count("cache_read", "cacheReadTokens"),
        output: count("output", "outputTokens"),
        reasoning: int(usage.and_then(|u| u.get("reasoningTokens"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic events in the Copilot CLI event-log shape.
    const LINES: [&str; 6] = [
        r#"{"type":"session.start","timestamp":"2026-10-24T08:00:00Z","data":{"sessionId":"cp1","selectedModel":"gpt-5.4","context":{"cwd":"/tmp/cop"}}}"#,
        r#"{"type":"user.message","timestamp":"2026-10-24T08:00:01Z","data":{"content":"x"}}"#,
        r#"{"type":"assistant.message","timestamp":"2026-10-24T08:00:02Z","data":{"toolRequests":[{"name":"bash","arguments":{}}]}}"#,
        r#"{"type":"assistant.turn_end","timestamp":"2026-10-24T08:00:03Z","data":{}}"#,
        r#"{"type":"assistant.message","timestamp":"2026-10-24T08:00:04Z","data":{"content":"done"}}"#,
        r#"{"type":"assistant.turn_end","timestamp":"2026-10-24T08:00:05Z","data":{}}"#,
    ];

    #[test]
    fn turns_tools_and_shutdown_metrics() {
        let mut st = FileState::default();
        let mut e: Vec<Entry> = LINES
            .iter()
            .flat_map(|l| parse(l.as_bytes(), &mut st, 0.0))
            .collect();
        assert_eq!(st.session, "cp1");
        assert_eq!(st.project, "cop");
        assert!(matches!(e[0], Entry::TurnBegan(_)));
        assert!(matches!(&e[2], Entry::Tool { name, .. } if name == "bash"));
        assert!(matches!(e[3], Entry::TurnActive(_)), "tool iteration end");
        assert!(matches!(
            e.last(),
            Some(Entry::TurnEnded {
                completed: true,
                ..
            })
        ));
        let shut = r#"{"type":"session.shutdown","id":"ev9","timestamp":"2026-10-24T08:10:00Z","data":{"modelMetrics":{"gpt-5.4":{"usage":{"inputTokens":100,"outputTokens":30,"cacheReadTokens":50}}}}}"#;
        e = parse(shut.as_bytes(), &mut st, 0.0);
        let Entry::Usage { key, billable, .. } = &e[0] else {
            panic!("{e:?}")
        };
        assert_eq!(key, "copilot:cp1:ev9:gpt-5.4");
        assert_eq!(billable.tokens.total(), 180);
        assert!(billable.aggregate);
        // A second shutdown with the same cumulative counters adds nothing.
        let again = shut.replace("ev9", "ev10");
        assert!(parse(again.as_bytes(), &mut st, 0.0).is_empty());
    }
}
