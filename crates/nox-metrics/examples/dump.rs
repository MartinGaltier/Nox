//! Prints one real snapshot: `cargo run -p nox-metrics --example dump [--json]`.
use std::time::Duration;

fn main() {
    let snap = nox_metrics::diag::snapshot(Duration::from_secs(1));
    if std::env::args().any(|a| a == "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&snap).expect("serialise")
        );
    } else {
        println!("{snap:#?}");
    }
}
