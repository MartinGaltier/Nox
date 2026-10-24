//! Lists SMC keys: `cargo run -p nox-metrics --example smc_keys [PREFIX…]`.
fn main() {
    let prefixes: Vec<String> = std::env::args().skip(1).collect();
    match nox_metrics::diag::smc_keys() {
        Ok(keys) => {
            println!("{} keys", keys.len());
            for (k, t, v) in keys {
                if prefixes.is_empty() || prefixes.iter().any(|p| k.starts_with(p.as_str())) {
                    println!("{k} [{t}] {v:?}");
                }
            }
        }
        Err(e) => eprintln!("SMC: {e}"),
    }
}
