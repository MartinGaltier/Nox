// Adapted from vorssaint-utils (GPL-3.0-or-later), Services/AgentUsage/AgentPricing.swift, commit e916961.
//! API-equivalent value of agent traffic, from an embedded, dated price list
//! (`data/agent-prices.json`, copied from vorssaint-utils Resources at e916961).
//! Every figure produced here is an estimate (`Reading::Estimated`).

use serde::Deserialize;
use std::sync::OnceLock;

/// Token counts as both logs can be reduced to. `input` excludes cache
/// traffic; `output` already contains reasoning tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.cache_write + self.cache_read + self.output
    }
    pub fn prompt(&self) -> u64 {
        self.input + self.cache_write + self.cache_read
    }
    pub fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.cache_write += o.cache_write;
        self.cache_read += o.cache_read;
        self.output += o.output;
        self.reasoning += o.reasoning;
    }
    /// Streamed replies are logged once per block; the largest reading is final.
    pub fn merged(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.max(o.input),
            cache_write: self.cache_write.max(o.cache_write),
            cache_read: self.cache_read.max(o.cache_read),
            output: self.output.max(o.output),
            reasoning: self.reasoning.max(o.reasoning),
        }
    }
    pub fn saturating_sub(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.saturating_sub(o.input),
            cache_write: self.cache_write.saturating_sub(o.cache_write),
            cache_read: self.cache_read.saturating_sub(o.cache_read),
            output: self.output.saturating_sub(o.output),
            reasoning: self.reasoning.saturating_sub(o.reasoning),
        }
    }
    pub fn to_usage(self) -> nox_core::model::agents::TokenUsage {
        nox_core::model::agents::TokenUsage {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
        }
    }
}

/// What one response bills, beyond its model.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Billable {
    pub tokens: Tokens,
    /// Session totals: price at base rates (no long-context inference).
    pub aggregate: bool,
    /// Part of `cache_write` kept for an hour.
    pub long_cache_write: u64,
    pub fast: bool,
    /// Inference pinned to the United States.
    pub domestic: bool,
    pub web_searches: u64,
    /// Cost the agent itself reported (OpenCode), preferred over list prices.
    pub reported_cost: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawModel {
    id: String,
    input: f64,
    output: f64,
    #[serde(rename = "cacheRead")]
    cache_read: f64,
    #[serde(rename = "cacheWrite")]
    cache_write: Option<f64>,
    #[serde(rename = "cacheWriteLong")]
    cache_write_long: Option<f64>,
    #[serde(rename = "fastMultiplier")]
    fast_multiplier: Option<f64>,
    #[serde(rename = "longContext")]
    long_context: Option<RawLong>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawLong {
    above: u64,
    #[serde(rename = "inputMultiplier")]
    input: f64,
    #[serde(rename = "outputMultiplier")]
    output: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPlan {
    #[serde(rename = "match")]
    matches: String,
    name: String,
    monthly: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProvider {
    models: Vec<RawModel>,
    plans: Vec<RawPlan>,
    #[serde(rename = "webSearch", default)]
    web_search: f64,
    #[serde(rename = "usOnlyMultiplier", default)]
    us_only: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawList {
    schema: u32,
    updated: String,
    claude: RawProvider,
    codex: RawProvider,
}

/// USD per million tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub cache_write_long: f64,
    pub fast_multiplier: f64,
    pub long_context: Option<(u64, f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub matches: String,
    pub name: String,
    pub monthly: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct PriceList {
    pub updated: String,
    claude: Vec<(String, Price)>,
    codex: Vec<(String, Price)>,
    pub claude_plans: Vec<Plan>,
    pub codex_plans: Vec<Plan>,
    web_search: f64,
    us_only: f64,
}

const EMBEDDED: &str = include_str!("../../data/agent-prices.json");

fn finite(v: f64, max: f64) -> bool {
    v.is_finite() && (0.0..=max).contains(&v)
}

impl PriceList {
    /// Validates like the reference: a list that breaks a rule is refused whole.
    pub fn decode(json: &str) -> Option<PriceList> {
        let raw: RawList = serde_json::from_str(json).ok()?;
        if raw.schema != 1 {
            return None;
        }
        let models = |p: &RawProvider, claude: bool| -> Option<Vec<(String, Price)>> {
            let mut out = Vec::new();
            for m in &p.models {
                if m.id.is_empty() || m.id.starts_with("claude-") != claude {
                    return None;
                }
                let cw = m.cache_write.unwrap_or(m.input);
                let price = Price {
                    input: m.input,
                    output: m.output,
                    cache_read: m.cache_read,
                    cache_write: cw,
                    cache_write_long: m.cache_write_long.unwrap_or(cw),
                    fast_multiplier: m.fast_multiplier.unwrap_or(1.0),
                    long_context: m
                        .long_context
                        .as_ref()
                        .map(|l| (l.above, l.input, l.output)),
                };
                let ok = [
                    price.input,
                    price.output,
                    price.cache_read,
                    price.cache_write,
                    price.cache_write_long,
                ]
                .iter()
                .all(|v| finite(*v, 1000.0))
                    && (1.0..=10.0).contains(&price.fast_multiplier);
                if !ok {
                    return None;
                }
                out.push((m.id.clone(), price));
            }
            (!out.is_empty()).then_some(out)
        };
        let plans = |p: &RawProvider| {
            p.plans
                .iter()
                .map(|r| Plan {
                    matches: r.matches.clone(),
                    name: r.name.clone(),
                    monthly: r.monthly,
                })
                .collect()
        };
        Some(PriceList {
            updated: raw.updated,
            claude: models(&raw.claude, true)?,
            codex: models(&raw.codex, false)?,
            claude_plans: plans(&raw.claude),
            codex_plans: plans(&raw.codex),
            web_search: raw.claude.web_search,
            us_only: raw.claude.us_only.unwrap_or(1.0),
        })
    }

    pub fn embedded() -> &'static PriceList {
        static LIST: OnceLock<PriceList> = OnceLock::new();
        LIST.get_or_init(|| PriceList::decode(EMBEDDED).expect("embedded price list is valid"))
    }

    /// Longest matching family wins; an unknown sibling ("mini", "pro"…) is
    /// unpriced rather than wrongly priced.
    pub fn price(&self, model: &str) -> Option<&Price> {
        let id = normalized(model);
        let table = if id.starts_with("claude-") {
            &self.claude
        } else {
            &self.codex
        };
        let (fam, price) = table
            .iter()
            .filter(|(fam, _)| matches_family(&id, fam))
            .max_by_key(|(fam, _)| fam.len())?;
        let rest = &id[fam.len()..];
        const SIBLINGS: [&str; 12] = [
            "mini",
            "nano",
            "pro",
            "lite",
            "research",
            "search",
            "audio",
            "realtime",
            "transcribe",
            "tts",
            "cyber",
            "image",
        ];
        if SIBLINGS.iter().any(|s| rest.contains(s)) {
            return None;
        }
        Some(price)
    }

    /// USD at API list price; `None` when the model has no known price.
    pub fn cost(&self, b: &Billable, model: &str) -> Option<f64> {
        if let Some(c) = b.reported_cost {
            return Some(c);
        }
        if b.aggregate && b.tokens.total() == 0 {
            return Some(0.0);
        }
        let p = self.price(model)?;
        let t = &b.tokens;
        let long = b.long_cache_write.min(t.cache_write);
        let mut mult = if b.fast { p.fast_multiplier } else { 1.0 };
        if b.domestic {
            mult *= self.us_only;
        }
        let (mut ir, mut or) = (mult, mult);
        if !b.aggregate
            && let Some((above, li, lo)) = p.long_context
            && t.prompt() > above
        {
            ir *= li;
            or *= lo;
        }
        let f = |n: u64| n as f64;
        Some(
            (f(t.input) * p.input * ir
                + f(t.cache_write - long) * p.cache_write * ir
                + f(long) * p.cache_write_long * ir
                + f(t.cache_read) * p.cache_read * ir
                + f(t.output) * p.output * or)
                / 1_000_000.0
                + f(b.web_searches) * self.web_search,
        )
    }

    pub fn claude_plan(&self, recorded: &str) -> Option<String> {
        let r = recorded.to_lowercase();
        self.claude_plans
            .iter()
            .find(|p| r.contains(&p.matches))
            .map(|p| p.name.clone())
    }

    pub fn codex_plan(&self, plan_type: &str) -> Option<String> {
        let raw = plan_type.trim().to_lowercase();
        if raw.is_empty() {
            return None;
        }
        Some(
            self.codex_plans
                .iter()
                .find(|p| p.matches == raw)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| {
                    let mut c = raw.chars();
                    c.next()
                        .map(|f| f.to_uppercase().chain(c).collect())
                        .unwrap_or_default()
                }),
        )
    }
}

/// Provider prefixes, snapshot tags and router suffixes vary; the family does not.
pub fn normalized(model: &str) -> String {
    let mut id = model.trim().to_lowercase();
    if let Some(i) = id.find("claude-") {
        id = id[i..].to_string();
    }
    if id.starts_with("claude-") {
        id = id.replace('.', "-");
    }
    if let Some(i) = id.rfind('/') {
        id = id[i + 1..].to_string();
    }
    for marker in ['@', '['] {
        if let Some(i) = id.find(marker) {
            id.truncate(i);
        }
    }
    id
}

/// "gpt-5" is not "gpt-5.1"; a snapshot date after the family is the same model.
fn matches_family(id: &str, fam: &str) -> bool {
    let Some(rest) = id.strip_prefix(fam) else {
        return false;
    };
    let Some(next) = rest.chars().next() else {
        return true;
    };
    if !matches!(next, '-' | '_' | ':') {
        return false;
    }
    let digits = rest[1..].chars().take_while(|c| c.is_ascii_digit()).count();
    digits == 0 || digits >= 4
}

/// "claude-opus-5-5-20261001" → "Opus 5.5"; "gpt-6-astra" → "GPT-6 Astra".
pub fn display_name(model: &str) -> String {
    let id = normalized(model);
    if id.is_empty() {
        return String::new();
    }
    let cap = |w: &str| {
        let mut c = w.chars();
        c.next()
            .map(|f| f.to_uppercase().chain(c).collect::<String>())
            .unwrap_or_default()
    };
    let is_num = |p: &str| p.chars().all(|c| c.is_ascii_digit() || c == '.');
    if let Some(rest) = id.strip_prefix("claude-") {
        let family = rest.split(':').next().unwrap_or(rest);
        let parts: Vec<&str> = family
            .split('-')
            .filter(|p| !(p.len() >= 6 && p.chars().all(|c| c.is_ascii_digit())))
            .filter(|p| *p != "latest")
            .filter(|p| !(p.starts_with('v') && p[1..].starts_with(|c: char| c.is_ascii_digit())))
            .collect();
        let words: Vec<&str> = parts.iter().copied().filter(|p| !is_num(p)).collect();
        let version = parts
            .iter()
            .copied()
            .filter(|p| is_num(p))
            .collect::<Vec<_>>()
            .join(".");
        let mut out: Vec<String> = Vec::new();
        if let Some(w) = words.first() {
            out.push(cap(w));
        }
        if !version.is_empty() {
            out.push(version);
        }
        out.extend(words.iter().skip(1).map(|w| cap(w)));
        return out.join(" ");
    }
    let Some(rest) = id.strip_prefix("gpt-") else {
        return id;
    };
    let mut parts: Vec<&str> = rest
        .split('-')
        .filter(|p| *p != "latest")
        .filter(|p| !(p.len() >= 6 && p.chars().all(|c| c.is_ascii_digit())))
        .collect();
    if parts.len() >= 4 {
        let n = parts.len();
        let year = parts[n - 3].parse::<u32>().ok();
        if year.is_some_and(|y| (2000..=2100).contains(&y))
            && parts[n - 2..]
                .iter()
                .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit()))
        {
            parts.truncate(n - 3);
        }
    }
    let Some(first) = parts.first() else {
        return "GPT".into();
    };
    let head = if first.starts_with(|c: char| c.is_ascii_digit()) {
        format!("GPT-{first}")
    } else {
        format!("GPT {}", cap(first))
    };
    std::iter::once(head)
        .chain(parts.iter().skip(1).map(|p| cap(p)))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(input: u64, cw: u64, cr: u64, out: u64) -> Billable {
        Billable {
            tokens: Tokens {
                input,
                cache_write: cw,
                cache_read: cr,
                output: out,
                reasoning: 0,
            },
            ..Default::default()
        }
    }

    #[test]
    fn embedded_list_loads_and_is_dated() {
        let l = PriceList::embedded();
        assert_eq!(l.updated, "2026-10-09");
    }

    #[test]
    fn family_matching() {
        let l = PriceList::embedded();
        assert_eq!(l.price("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(l.price("claude-opus-5-5-20261001").unwrap().input, 4.0);
        assert_eq!(l.price("anthropic/claude-opus-5.5").unwrap().input, 4.0);
        // "claude-opus-5" must not price a later "claude-opus-5-6".
        assert!(l.price("claude-opus-5-6").is_none());
        assert_eq!(l.price("gpt-5.4").unwrap().input, 2.5);
        assert_eq!(l.price("gpt-5.4-mini").unwrap().input, 0.75);
        assert!(
            l.price("gpt-5.7-mini").is_none(),
            "unknown sibling stays unpriced"
        );
        assert!(l.price("totally-unknown").is_none());
    }

    #[test]
    fn costs() {
        let l = PriceList::embedded();
        // Opus 5.5: 4 in / 20 out / 0.2 read / 5 write per MTok.
        let c = l
            .cost(&b(1_000_000, 0, 0, 1_000_000), "claude-opus-5-5")
            .unwrap();
        assert!((c - 24.0).abs() < 1e-9);
        let c = l
            .cost(&b(0, 1_000_000, 1_000_000, 0), "claude-opus-5-5")
            .unwrap();
        assert!((c - 5.2).abs() < 1e-9);
        let mut fast = b(1_000_000, 0, 0, 0);
        fast.fast = true;
        assert!((l.cost(&fast, "claude-opus-5-5").unwrap() - 8.0).abs() < 1e-9);
        let mut long = b(0, 1_000_000, 0, 0);
        long.long_cache_write = 1_000_000;
        assert!((l.cost(&long, "claude-opus-5-5").unwrap() - 8.0).abs() < 1e-9);
        // Long-context premium on Sonnet 4.5 past 200k prompt tokens.
        let c = l.cost(&b(300_000, 0, 0, 0), "claude-sonnet-4-5").unwrap();
        assert!((c - 300_000.0 * 3.0 * 2.0 / 1e6).abs() < 1e-9);
        let mut agg = b(300_000, 0, 0, 0);
        agg.aggregate = true;
        assert!((l.cost(&agg, "claude-sonnet-4-5").unwrap() - 0.9).abs() < 1e-9);
        assert_eq!(l.cost(&b(1, 0, 0, 1), "mystery-model"), None);
    }

    #[test]
    fn names_and_plans() {
        assert_eq!(display_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(display_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(display_name("gpt-6-astra"), "GPT-6 Astra");
        assert_eq!(display_name("gpt-5.4-mini-2026-03-01"), "GPT-5.4 Mini");
        let l = PriceList::embedded();
        assert_eq!(
            l.claude_plan("default_claude_max_20x").as_deref(),
            Some("Max 20×")
        );
        assert_eq!(l.codex_plan("plus").as_deref(), Some("Plus"));
        assert_eq!(l.codex_plan("galaxy").as_deref(), Some("Galaxy"));
    }

    #[test]
    fn refuses_broken_lists() {
        assert!(PriceList::decode("{}").is_none());
        let bad = EMBEDDED.replacen("\"schema\": 1", "\"schema\": 2", 1);
        assert!(PriceList::decode(&bad).is_none());
    }
}
