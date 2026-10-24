//! Per-source sampling cost: `cargo run --release -p nox-metrics --example profile`.
fn main() {
    for (name, d) in nox_metrics::profile::parts(30) {
        println!("{name:32} {:8.1} µs", d.as_secs_f64() * 1e6);
    }
}
