//! Usage records, one per response, deduplicated by key and bucketed by
//! local day. Bounded: records older than `KEEP_DAYS` are dropped and the
//! store never exceeds `MAX_RECORDS`.

use super::pricing::{Billable, PriceList, Tokens};
use nox_core::model::Timestamp;
use nox_core::model::agents::AgentProvider;
use std::collections::HashMap;

pub const KEEP_DAYS: i64 = 31;
const MAX_RECORDS: usize = 400_000;

/// Local calendar day (days since 1970-01-01 in the Mac's time zone).
pub fn local_day(t: Timestamp) -> i64 {
    let secs = t.floor() as libc::time_t;
    // SAFETY: `localtime_r` writes into the zeroed `tm` we own and reads only
    // `secs`; it is the thread-safe variant.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return (t / 86_400.0).floor() as i64;
        }
        tm
    };
    super::parse::days_from_civil(
        i64::from(tm.tm_year) + 1900,
        i64::from(tm.tm_mon) + 1,
        i64::from(tm.tm_mday),
    )
}

#[derive(Debug, Clone)]
pub struct Record {
    pub provider: AgentProvider,
    pub account: usize,
    pub at: Timestamp,
    pub day: i64,
    pub model: String,
    pub billable: Billable,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Totals {
    pub tokens: Tokens,
    pub cost: f64,
    /// Responses whose model has no known price (left out of `cost`).
    pub unpriced: u64,
    pub requests: u64,
}

impl Totals {
    fn add(&mut self, r: &Record) {
        self.tokens.add(&r.billable.tokens);
        self.requests += 1;
        match r.cost {
            Some(c) => self.cost += c,
            None => self.unpriced += 1,
        }
    }
}

#[derive(Default)]
pub struct UsageStore {
    records: HashMap<String, Record>,
}

/// What adding a record changed, for the live turn it counts toward.
pub struct Added {
    pub tokens: Tokens,
    pub cost: Option<f64>,
}

impl UsageStore {
    /// Adds or merges a response. Returns the growth (zero when it repeats a
    /// line already counted, as resumed or duplicated logs do).
    pub fn add(
        &mut self,
        key: String,
        (provider, account): (AgentProvider, usize),
        at: Timestamp,
        model: &str,
        billable: Billable,
        today: i64,
    ) -> Option<Added> {
        let prices = PriceList::embedded();
        let day = local_day(at);
        if day < today - KEEP_DAYS {
            return None;
        }
        if let Some(old) = self.records.get_mut(&key) {
            let merged = old.billable.tokens.merged(&billable.tokens);
            if merged == old.billable.tokens && billable.reported_cost == old.billable.reported_cost
            {
                return None;
            }
            let grown = merged.saturating_sub(&old.billable.tokens);
            let before = old.cost;
            old.billable.tokens = merged;
            if billable.reported_cost.is_some() {
                old.billable.reported_cost = billable.reported_cost;
            }
            if old.model.is_empty() && !model.is_empty() {
                old.model = model.to_string();
            }
            old.cost = prices.cost(&old.billable, &old.model);
            let cost = match (old.cost, before) {
                (Some(a), Some(b)) => Some(a - b),
                (a, _) => a,
            };
            return Some(Added {
                tokens: grown,
                cost,
            });
        }
        if self.records.len() >= MAX_RECORDS {
            self.prune(today, true);
        }
        let cost = prices.cost(&billable, model);
        self.records.insert(
            key,
            Record {
                provider,
                account,
                at,
                day,
                model: model.to_string(),
                billable,
                cost,
            },
        );
        Some(Added {
            tokens: billable.tokens,
            cost,
        })
    }

    /// Drops old records; `hard` also drops the oldest tenth when still full.
    pub fn prune(&mut self, today: i64, hard: bool) {
        self.records.retain(|_, r| r.day >= today - KEEP_DAYS);
        if hard && self.records.len() >= MAX_RECORDS {
            let mut ats: Vec<f64> = self.records.values().map(|r| r.at).collect();
            ats.sort_by(f64::total_cmp);
            let cut = ats[ats.len() / 10];
            self.records.retain(|_, r| r.at > cut);
        }
    }

    /// Totals for `provider`/`account` over the last `days` local days
    /// (1 = today only).
    pub fn totals(
        &self,
        provider: AgentProvider,
        account: Option<usize>,
        today: i64,
        days: i64,
    ) -> Totals {
        let mut t = Totals::default();
        for r in self.records.values() {
            if r.provider == provider
                && account.is_none_or(|a| a == r.account)
                && r.day > today - days
                && r.day <= today
            {
                t.add(r);
            }
        }
        t
    }

    pub fn any(&self, provider: AgentProvider) -> bool {
        self.records.values().any(|r| r.provider == provider)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bill(input: u64, out: u64) -> Billable {
        Billable {
            tokens: Tokens {
                input,
                output: out,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn dedup_merge_and_periods() {
        let now = 1_792_843_200.0; // 2026-10-24 12:00 UTC
        let today = local_day(now);
        let mut s = UsageStore::default();
        let c = AgentProvider::Claude;
        assert!(
            s.add(
                "k1".into(),
                (c, 0),
                now,
                "claude-opus-5-5",
                bill(100, 10),
                today
            )
            .is_some()
        );
        // Streamed duplicate with a larger output: only the growth counts.
        let g = s
            .add(
                "k1".into(),
                (c, 0),
                now,
                "claude-opus-5-5",
                bill(100, 30),
                today,
            )
            .unwrap();
        assert_eq!(g.tokens.output, 20);
        assert!(
            s.add(
                "k1".into(),
                (c, 0),
                now,
                "claude-opus-5-5",
                bill(100, 30),
                today
            )
            .is_none()
        );
        s.add(
            "k2".into(),
            (c, 0),
            now - 3.0 * 86_400.0,
            "claude-opus-5-5",
            bill(1000, 0),
            today,
        );
        s.add("k3".into(), (c, 0), now, "mystery", bill(5, 5), today);
        s.add(
            "k4".into(),
            (AgentProvider::Codex, 0),
            now,
            "gpt-5.4",
            bill(1, 1),
            today,
        );
        let t = s.totals(c, None, today, 1);
        assert_eq!(t.tokens.total(), 140);
        assert_eq!(t.unpriced, 1);
        assert!((t.cost - (100.0 * 4.0 + 30.0 * 20.0) / 1e6).abs() < 1e-12);
        assert_eq!(s.totals(c, None, today, 7).tokens.total(), 1140);
        // Too old to keep.
        assert!(
            s.add(
                "k5".into(),
                (c, 0),
                now - 40.0 * 86_400.0,
                "x",
                bill(1, 1),
                today
            )
            .is_none()
        );
        s.prune(today + 30, false);
        assert_eq!(s.len(), 3, "the 3-day-old record falls out first");
    }
}
