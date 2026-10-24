//! Lists IOReport "Energy Model" channels over 1 s.
fn main() {
    match nox_metrics::diag::energy(
        std::env::args()
            .nth(1)
            .and_then(|a| a.parse().ok())
            .unwrap_or(1000),
    ) {
        Ok(v) => {
            for (g, c, u, x) in v {
                println!("{g} | {c} | {u} | {x}");
            }
        }
        Err(e) => eprintln!("{e}"),
    }
}
