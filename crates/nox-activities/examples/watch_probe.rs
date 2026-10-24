//! Checks Screen Recording (preflight, never prompts) and, only if already
//! granted, captures a small top-left region twice and prints the thumbnail
//! difference. No image is written anywhere.

use nox_activities::watch::{capture, tracker};
use nox_core::model::watch::Rect;
use std::time::{Duration, Instant};

fn main() {
    let granted = capture::has_access();
    println!("screen recording preflight: {granted}");
    if !granted {
        println!("not granted: no capture attempted");
        return;
    }
    let r = Rect {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 200.0,
    };
    let t = Instant::now();
    let a = capture::capture(r, Duration::from_secs(3));
    println!(
        "capture 1: {:?} in {:.0} ms",
        a.as_ref().map(|v| v.len()),
        t.elapsed().as_secs_f64() * 1e3
    );
    std::thread::sleep(Duration::from_millis(500));
    let b = capture::capture(r, Duration::from_secs(3));
    if let (Ok(a), Ok(b)) = (a, b) {
        println!(
            "thumbnail diff: {:.4}; mean grey {:.1}",
            tracker::diff(&a, &b),
            a.iter().map(|&x| f64::from(x)).sum::<f64>() / a.len() as f64
        );
    }
}
