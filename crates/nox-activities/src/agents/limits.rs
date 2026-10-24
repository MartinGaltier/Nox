// Adapted from vorssaint-utils (GPL-3.0-or-later), Services/AgentUsage/AgentClaudeAppUsage.swift
// and AgentClaudeServer.swift (limits), commit e916961.
//! Claude plan limits. Two sources:
//! * the Claude app's own history, `~/Library/Application Support/Claude/plan-usage-history.json`
//!   (a percentage per window every few minutes; read locally, nothing sent).
//!   Percentages are measured; renewal times are inferred, hence `resets_estimated`.
//! * the account usage endpoint, only when the user opted in (see `oauth`).

use super::parse::{parse_iso, seconds};
use nox_core::Reading;
use nox_core::model::Timestamp;
use nox_core::model::agents::{LimitKind, LimitSource, LimitWindow};
use serde_json::Value;

const SESSION: f64 = 5.0 * 3600.0;
const WEEK: f64 = 7.0 * 86_400.0;
/// The app checks every 5–15 minutes; a longer gap means it missed checks.
const FRESH_GAP: f64 = 30.0 * 60.0;

struct Spec {
    key: &'static str,
    kind: LimitKind,
    minutes: u32,
    scope: Option<&'static str>,
    name: &'static str,
}

const WINDOWS: [Spec; 4] = [
    Spec {
        key: "fh",
        kind: LimitKind::Session,
        minutes: 300,
        scope: None,
        name: "5 h",
    },
    Spec {
        key: "sd",
        kind: LimitKind::Weekly,
        minutes: 10_080,
        scope: None,
        name: "Semaine",
    },
    Spec {
        key: "so",
        kind: LimitKind::Weekly,
        minutes: 10_080,
        scope: Some("Opus"),
        name: "Opus semaine",
    },
    Spec {
        key: "sn",
        kind: LimitKind::Weekly,
        minutes: 10_080,
        scope: Some("Sonnet"),
        name: "Sonnet semaine",
    },
];

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub at: Timestamp,
    pub org: Option<String>,
    pub used: Vec<(&'static str, f64)>,
}

impl Sample {
    fn get(&self, key: &str) -> Option<f64> {
        self.used.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
}

/// Samples oldest first; `None` for a file version this reader does not know.
pub fn samples(bytes: &[u8]) -> Option<Vec<Sample>> {
    if bytes.len() > 4 << 20 {
        return None;
    }
    let json: Value = serde_json::from_slice(bytes).ok()?;
    let version = json.get("version")?.as_u64()?;
    if version != 1 && version != 2 {
        return None;
    }
    let mut out: Vec<Sample> = json
        .get("samples")?
        .as_array()?
        .iter()
        .filter_map(|e| {
            let at = seconds(e.get("t"))?;
            let values = if version == 1 { e } else { e.get("u")? };
            let used = WINDOWS
                .iter()
                .filter_map(|w| {
                    let v = values.get(w.key)?.as_f64().filter(|v| v.is_finite())?;
                    Some((w.key, v.clamp(0.0, 100.0)))
                })
                .collect();
            Some(Sample {
                at,
                org: e.get("org").and_then(Value::as_str).map(str::to_string),
                used,
            })
        })
        .collect();
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    Some(out)
}

/// The latest reading as windows. `org` filters samples of another account.
pub fn from_samples(all: &[Sample], now: Timestamp, org: Option<&str>) -> Vec<LimitWindow> {
    let mine: Vec<&Sample> = all
        .iter()
        .filter(|s| org.is_none_or(|o| s.org.as_deref().is_none_or(|so| so == o)))
        .collect();
    let Some(latest) = mine.last() else {
        return Vec::new();
    };
    let age = now - latest.at;
    if !(-300.0..7.0 * 86_400.0).contains(&age) {
        return Vec::new();
    }
    let history: Vec<&Sample> = mine
        .iter()
        .copied()
        .filter(|s| s.org == latest.org)
        .collect();
    let mut out = Vec::new();
    for w in &WINDOWS {
        let Some(used) = latest.get(w.key) else {
            continue;
        };
        let resets = match w.kind {
            LimitKind::Session => {
                // An old reading says nothing about a session that renewed since.
                if age >= SESSION {
                    continue;
                }
                session_end(&history, w.key, used)
            }
            _ => week_renewal(&history, w.key, latest.at),
        };
        if resets.is_some_and(|r| r <= now) {
            continue;
        }
        if resets.is_none() && age >= 86_400.0 {
            continue;
        }
        out.push(LimitWindow {
            name: w.name.into(),
            used_percent: Reading::Measured(used),
            resets_at: resets,
            kind: w.kind,
            window_minutes: Some(w.minutes),
            scope: w.scope.map(str::to_string),
            source: LimitSource::ClaudeApp,
            observed_at: latest.at,
            resets_estimated: resets.is_some(),
        });
    }
    out
}

/// A session renews five hours after its first request: the first reading of
/// the run above zero that ends with the latest is the latest it can start.
fn session_end(h: &[&Sample], key: &str, used: f64) -> Option<Timestamp> {
    if used <= 0.0 || h.is_empty() {
        return None;
    }
    let mut first = h.len() - 1;
    while first > 0 {
        let (Some(before), Some(cur)) = (h[first - 1].get(key), h[first].get(key)) else {
            break;
        };
        // A drop is a renewal too: work across one never reads zero.
        if before <= 0.0 || before > cur + 1.0 || h[first].at - h[first - 1].at >= SESSION {
            break;
        }
        first -= 1;
    }
    let latest_start = h[first].at;
    // After a long gap the start lies somewhere in it: take the middle.
    let start = if first > 0 && latest_start - h[first - 1].at > FRESH_GAP {
        let lower = h[first - 1].at.max(latest_start - SESSION);
        lower + (latest_start - lower) / 2.0
    } else {
        latest_start
    };
    Some(start + SESSION)
}

/// The weekly window renews on a fixed schedule; the last drop places it (on
/// the hour, UTC), then whole weeks are added until it lies after `reading`.
fn week_renewal(h: &[&Sample], key: &str, reading: Timestamp) -> Option<Timestamp> {
    let drop = h.windows(2).rev().find_map(|p| {
        let (b, a) = (p[0].get(key)?, p[1].get(key)?);
        (a + 1.0 < b && p[1].at - p[0].at < 86_400.0).then_some((p[0].at, p[1].at))
    })?;
    let next_hour = (drop.0 / 3600.0).floor() * 3600.0 + 3600.0;
    let mut m = if next_hour <= drop.1 {
        next_hour
    } else {
        drop.1
    };
    while m <= reading {
        m += WEEK;
    }
    Some(m)
}

/// Windows from the usage endpoint (`five_hour`, `seven_day`, …), as the
/// account reports them: percentages and renewal times are both measured.
pub fn from_usage_response(v: &Value, observed: Timestamp) -> Vec<LimitWindow> {
    let root = v.get("rate_limits").unwrap_or(v);
    let keys = [
        ("five_hour", 0usize),
        ("seven_day", 1),
        ("seven_day_opus", 2),
        ("seven_day_sonnet", 3),
    ];
    keys.iter()
        .filter_map(|(k, i)| {
            let w = root.get(*k)?;
            let used = w.get("utilization")?.as_f64().filter(|u| u.is_finite())?;
            let resets = match w.get("resets_at") {
                Some(Value::String(s)) => parse_iso(s),
                other => seconds(other),
            };
            if resets.is_some_and(|r| r <= observed) {
                return None;
            }
            let s = &WINDOWS[*i];
            Some(LimitWindow {
                name: s.name.into(),
                used_percent: Reading::Measured(used.clamp(0.0, 100.0)),
                resets_at: resets,
                kind: s.kind,
                window_minutes: Some(s.minutes),
                scope: s.scope.map(str::to_string),
                source: LimitSource::OAuth,
                observed_at: observed,
                resets_estimated: false,
            })
        })
        .collect()
}

/// A window that renewed after its reading is unknown, not freshly empty.
pub fn expire(windows: &[LimitWindow], now: Timestamp) -> Vec<LimitWindow> {
    windows
        .iter()
        .map(|w| {
            let mut w = w.clone();
            if w.resets_at.is_some_and(|r| r <= now) {
                w.used_percent = Reading::Unavailable(nox_core::Unavailable::Pending);
            }
            w
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f64 = 3600.0;

    // Synthetic history in the Claude app's version-2 format.
    fn file(points: &[(f64, f64, f64)]) -> Vec<u8> {
        let s: Vec<Value> = points
            .iter()
            .map(|(t, fh, sd)| serde_json::json!({"t": t * 1000.0, "org": "o1", "u": {"fh": fh, "sd": sd}}))
            .collect();
        serde_json::json!({"version": 2, "samples": s})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn app_history_windows() {
        let t0 = 1_792_800_000.0;
        let f = file(&[
            (t0, 0.0, 60.0),
            (t0 + 0.2 * H, 0.0, 2.0), // weekly renewed between these two readings
            (t0 + 24.0 * H, 0.0, 10.0),
            (t0 + 24.1 * H, 5.0, 11.0), // a session starts
            (t0 + 25.0 * H, 20.0, 15.0),
        ]);
        let s = samples(&f).unwrap();
        let now = t0 + 25.1 * H;
        let w = from_samples(&s, now, Some("o1"));
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].kind, LimitKind::Session);
        assert_eq!(w[0].used_percent, Reading::Measured(20.0));
        assert_eq!(w[0].resets_at, Some(t0 + 24.1 * H + 5.0 * H));
        assert!(w[0].resets_estimated);
        assert_eq!(w[0].source, LimitSource::ClaudeApp);
        let next_hour = ((t0 / H).floor() + 1.0) * H;
        let expected = if next_hour <= t0 + 0.2 * H {
            next_hour
        } else {
            t0 + 0.2 * H
        } + WEEK;
        assert_eq!(w[1].resets_at, Some(expected));
        // Another account's history is ignored; stale history yields nothing.
        assert!(from_samples(&s, now, Some("other")).is_empty());
        assert!(from_samples(&s, now + 8.0 * 86_400.0, Some("o1")).is_empty());
        // Six hours later the session window is unknown.
        assert_eq!(from_samples(&s, t0 + 31.0 * H, Some("o1")).len(), 1);
        assert!(samples(br#"{"version":9,"samples":[]}"#).is_none());
    }

    #[test]
    fn usage_response() {
        let v = serde_json::json!({
            "five_hour": {"utilization": 42.0, "resets_at": "2026-10-24T15:00:00Z"},
            "seven_day": {"utilization": 10.5, "resets_at": null},
            "seven_day_opus": null
        });
        let w = from_usage_response(&v, parse_iso("2026-10-24T12:00:00Z").unwrap());
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].source, LimitSource::OAuth);
        assert!(!w[0].resets_estimated);
        assert_eq!(w[0].resets_at, parse_iso("2026-10-24T15:00:00Z"));
        let later = expire(&w, parse_iso("2026-10-24T16:00:00Z").unwrap());
        assert!(
            later[0].used_percent.value().is_none(),
            "renewed: unknown, not 0"
        );
        assert_eq!(later[1].used_percent, Reading::Measured(10.5));
    }
}
