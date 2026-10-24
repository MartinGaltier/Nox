//! Runs the agents collector once on this Mac's real logs and prints
//! AGGREGATES ONLY: counts, token totals, estimated costs, limit percentages.
//! No session id, project, prompt, path or tool name is printed.
//!
//! `cargo run -p nox-activities --example agents_dump`

use nox_activities::agents::collector::Collector;
use nox_core::model::agents::{AgentPeriod, AgentSettings, SessionPhase};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    // Local logs only: never the OAuth query, whatever the saved settings say.
    let period = match std::env::var("NOX_PERIOD").as_deref() {
        Ok("week") => AgentPeriod::Week,
        Ok("month") => AgentPeriod::Month,
        _ => AgentPeriod::Today,
    };
    let s = AgentSettings {
        allow_limit_queries: false,
        period,
        ..Default::default()
    };
    let mut c = Collector::new(home);
    let t0 = Instant::now();
    let now = nox_core::model::now();
    let first = c.tick(&s, now);
    let scan = t0.elapsed();
    let t1 = Instant::now();
    let second = c.tick(&s, nox_core::model::now());
    let inc = t1.elapsed();
    let (state, _) = c.snapshot(&s, nox_core::model::now());
    let (files, records, sessions) = c.stats();
    println!(
        "first pass: {:.2} s, {files} log files, {records} responses kept, {sessions} sessions tracked",
        scan.as_secs_f64()
    );
    println!(
        "second (incremental) pass: {:.1} ms; events: first {} / second {}",
        inc.as_secs_f64() * 1e3,
        first.len(),
        second.len()
    );
    println!("price table: {}", state.prices_updated);
    for p in &state.providers {
        println!("\n== {} (detected: {})", p.provider.label(), p.detected);
        let count = |ph: SessionPhase| p.sessions.iter().filter(|x| x.phase == ph).count();
        println!(
            "  visible sessions: {} (working {}, waiting {}, finished {}, idle {})",
            p.sessions.len(),
            count(SessionPhase::Working),
            count(SessionPhase::WaitingForUser),
            count(SessionPhase::Finished),
            count(SessionPhase::Idle)
        );
        let tok = |r: &nox_core::Reading<nox_core::model::agents::TokenUsage>| match r.value() {
            Some(t) => format!(
                "{} total (in {}, out {}, cache read {}, cache write {})",
                t.total(),
                t.input,
                t.output,
                t.cache_read,
                t.cache_write
            ),
            None => format!("{r:?}"),
        };
        println!("  tokens today: {}", tok(&p.tokens_today));
        println!(
            "  tokens {}: {}",
            state.period.label_fr(),
            tok(&p.tokens_period)
        );
        println!(
            "  API value today (estimate): {:?} | period: {:?} | unpriced responses: {}",
            p.cost_today_usd.value().map(|v| format!("{v:.2} $")),
            p.cost_period_usd.value().map(|v| format!("{v:.2} $")),
            p.unpriced_requests
        );
        if let Some(plan) = &p.plan {
            println!("  plan: {plan}");
        }
        match p.limits.value() {
            Some(w) => {
                for w in w {
                    let left = w.resets_at.map(|r| (r - nox_core::model::now()) / 3600.0);
                    println!(
                        "  limit {:<14} used {:?} resets in {} ({:?}{})",
                        w.name,
                        w.used_percent.value(),
                        left.map(|h| format!("{h:.1} h"))
                            .unwrap_or_else(|| "?".into()),
                        w.source,
                        if w.resets_estimated {
                            ", estimated"
                        } else {
                            ""
                        }
                    );
                }
            }
            None => println!("  limits: {:?}", p.limits),
        }
        println!("  extra accounts: {}", p.accounts.len());
        if let Some(n) = &p.note {
            println!("  note: {n}");
        }
    }
}
