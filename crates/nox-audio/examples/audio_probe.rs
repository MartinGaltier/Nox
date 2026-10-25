//! Lists audio clients and the default output. Creates nothing, needs no
//! permission: `cargo run -p nox-audio --example audio_probe`.
fn main() {
    println!("supported (macOS ≥ 14.4): {}", nox_audio::supported());
    match nox_audio::default_output() {
        Some((name, uid, rate, out_ch, in_ch)) => {
            println!("default output: {name} ({uid}) {rate} Hz, {out_ch} out / {in_ch} in")
        }
        None => println!("default output: none"),
    }
    for c in nox_audio::audio_clients() {
        println!(
            "obj {:>4} pid {:>6} playing={:<5} bundle={:<40} owner={:?} {:?}",
            c.object,
            c.pid,
            c.running_output,
            c.bundle_id.as_deref().unwrap_or("-"),
            c.owner_pid,
            c.owner_name.as_deref().unwrap_or("-")
        );
    }
}
