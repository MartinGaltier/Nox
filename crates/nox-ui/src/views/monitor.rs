//! Detailed system monitor window content (cores P/E, sensors, memory,
//! battery, power, network, disks, bounded histories with gaps).

use super::{bar, label, secondary, sparkline, tertiary};
use crate::format;
use crate::geom::{Color, Rect};
use crate::scene::{Align, Scene, Weight};
use nox_core::model::metrics::{CoreKind, MemoryPressure, MetricsState, Series};
use nox_core::{Reading, Settings};

fn panel(s: &mut Scene, r: Rect, title: &str) -> Rect {
    s.fill(r, 12.0, Color::gray(1.0, 0.06));
    s.text(
        Rect::new(r.x + 14.0, r.y + 10.0, r.w - 28.0, 15.0),
        title,
        label(12.0).weight(Weight::Semibold),
    );
    Rect::new(r.x + 14.0, r.y + 32.0, r.w - 28.0, r.h - 44.0)
}

/// Key/value row; returns the next y.
fn kv(s: &mut Scene, r: Rect, y: f64, k: &str, v: String) -> f64 {
    s.text(Rect::new(r.x, y, r.w * 0.5, 14.0), k, secondary(11.0));
    s.text(
        Rect::new(r.x + r.w * 0.4, y, r.w * 0.6, 14.0),
        v,
        label(11.0).mono().align(Align::Right),
    );
    y + 17.0
}

fn graph(s: &mut Scene, r: Rect, m: &MetricsState, series: Series, max: Option<f64>, c: Color) {
    s.fill(r, 6.0, Color::gray(1.0, 0.04));
    sparkline(s, r.inset(2.0, 2.0), &m.history, series, max, c);
}

pub fn render(s: &mut Scene, r: Rect, metrics: Option<&MetricsState>, settings: &Settings) {
    s.shows_metrics = true;
    let Some(m) = metrics else {
        super::system::metrics_unavailable(s, r);
        return;
    };
    let snap = &m.snapshot;
    let gap = 12.0;
    let cw = (r.w - 2.0 * gap) / 3.0;
    let rh = (r.h - 2.0 * gap) / 3.0;
    let cell = |c: usize, row: usize, span: usize| {
        Rect::new(
            r.x + c as f64 * (cw + gap),
            r.y + row as f64 * (rh + gap),
            cw * span as f64 + gap * (span as f64 - 1.0),
            rh,
        )
    };

    // CPU (two columns wide).
    let cpu = panel(s, cell(0, 0, 2), "Processeur");
    s.text(
        Rect::new(cpu.x, cpu.y, 140.0, 22.0),
        format::reading(&snap.cpu_total, |v| format::percent(*v)),
        label(20.0).weight(Weight::Semibold).mono(),
    );
    s.text(
        Rect::new(cpu.x, cpu.y + 24.0, 160.0, 14.0),
        format!(
            "Température {}",
            format::reading(&snap.cpu_temp, |v| format::celsius(*v))
        ),
        secondary(10.5),
    );
    graph(
        s,
        Rect::new(cpu.x, cpu.y + 44.0, 150.0, cpu.h - 44.0),
        m,
        Series::Cpu,
        Some(100.0),
        Color::ACCENT,
    );
    let cores_r = Rect::new(cpu.x + 166.0, cpu.y, cpu.w - 166.0, cpu.h);
    match &snap.cpu_cores {
        Reading::Measured(cores) | Reading::Estimated(cores) if !cores.is_empty() => {
            let n = cores.len() as f64;
            let bw = (cores_r.w / n - 4.0).max(3.0);
            for (i, c) in cores.iter().enumerate() {
                let x = cores_r.x + i as f64 * (bw + 4.0);
                let col = if c.kind == CoreKind::Performance {
                    Color::ACCENT
                } else {
                    Color::GREEN
                };
                let full = cores_r.h - 16.0;
                s.fill(Rect::new(x, cores_r.y, bw, full), 3.0, Color::TRACK);
                let h = (c.usage / 100.0).clamp(0.0, 1.0) * full;
                s.fill(Rect::new(x, cores_r.y + full - h, bw, h), 3.0, col);
                s.text(
                    Rect::new(x - 2.0, cores_r.max_y() - 13.0, bw + 4.0, 12.0),
                    if c.kind == CoreKind::Performance {
                        "P"
                    } else {
                        "E"
                    },
                    tertiary(9.0).align(Align::Center),
                );
            }
        }
        other => s.text(
            cores_r,
            format!("Cœurs : {}", format::reading(other, |_| String::new())),
            tertiary(11.0),
        ),
    }

    // GPU.
    let gpu = panel(s, cell(2, 0, 1), "Graphisme");
    s.text(
        Rect::new(gpu.x, gpu.y, gpu.w, 22.0),
        format::reading(&snap.gpu_usage, |v| format::percent(*v)),
        label(20.0).weight(Weight::Semibold).mono(),
    );
    s.text(
        Rect::new(gpu.x, gpu.y + 24.0, gpu.w, 14.0),
        format!(
            "Température {}",
            format::reading(&snap.gpu_temp, |v| format::celsius(*v))
        ),
        secondary(10.5),
    );
    graph(
        s,
        Rect::new(gpu.x, gpu.y + 44.0, gpu.w, gpu.h - 44.0),
        m,
        Series::Gpu,
        Some(100.0),
        Color::PURPLE,
    );

    // Memory.
    let mem = panel(s, cell(0, 1, 1), "Mémoire");
    match &snap.memory {
        Reading::Measured(v) | Reading::Estimated(v) => {
            bar(
                s,
                Rect::new(mem.x, mem.y, mem.w, 6.0),
                (v.total > 0).then(|| v.used as f64 / v.total as f64),
                Color::GREEN,
            );
            let mut y = mem.y + 12.0;
            y = kv(
                s,
                mem,
                y,
                "Utilisée",
                format!(
                    "{} / {}",
                    format::bytes(v.used as f64),
                    format::bytes(v.total as f64)
                ),
            );
            y = kv(s, mem, y, "Apps", format::bytes(v.app as f64));
            y = kv(s, mem, y, "Résidente", format::bytes(v.wired as f64));
            y = kv(s, mem, y, "Compressée", format::bytes(v.compressed as f64));
            y = kv(
                s,
                mem,
                y,
                "Pression",
                format::reading(&v.pressure, |p| {
                    match p {
                        MemoryPressure::Normal => "normale",
                        MemoryPressure::Warning => "élevée",
                        MemoryPressure::Critical => "critique",
                    }
                    .into()
                }),
            );
            kv(
                s,
                mem,
                y,
                "Swap",
                format!(
                    "{} / {}",
                    format::bytes(v.swap_used as f64),
                    format::bytes(v.swap_total as f64)
                ),
            );
        }
        Reading::Unavailable(u) => s.text(mem, format::unavailable(u), tertiary(11.0)),
    }

    // Battery.
    let bat = panel(s, cell(1, 1, 1), "Batterie");
    match &snap.battery {
        Reading::Measured(b) | Reading::Estimated(b) => {
            let mut y = bat.y;
            let state = if b.charging {
                "en charge"
            } else if b.on_ac {
                "sur secteur"
            } else {
                "sur batterie"
            };
            y = kv(
                s,
                bat,
                y,
                "Charge",
                format!("{} · {state}", format::percent(b.percent)),
            );
            y = kv(
                s,
                bat,
                y,
                "Santé",
                format::reading(&b.health_percent, |v| format::percent(*v)),
            );
            y = kv(
                s,
                bat,
                y,
                "Cycles",
                format::reading(&b.cycle_count, |v| v.to_string()),
            );
            y = kv(
                s,
                bat,
                y,
                "État",
                format::reading(&b.condition, |v| v.clone()),
            );
            y = kv(
                s,
                bat,
                y,
                if b.charging {
                    "Plein dans"
                } else {
                    "Autonomie"
                },
                format::reading(&b.time_remaining_min, |v| {
                    format::duration(*v as f64 * 60.0)
                }),
            );
            y = kv(
                s,
                bat,
                y,
                "Flux batterie",
                format::reading(&b.battery_watts, |v| format::watts(*v)),
            );
            kv(
                s,
                bat,
                y,
                "Adaptateur",
                format::reading(&b.adapter_watts, |v| format::watts(*v)),
            );
        }
        Reading::Unavailable(u) => s.text(bat, format::unavailable(u), tertiary(11.0)),
    }

    // Power.
    let pw = panel(s, cell(2, 1, 1), "Puissance");
    match &snap.power {
        Reading::Measured(p) | Reading::Estimated(p) => {
            let mut y = pw.y;
            for (k, v) in [
                ("Système", &p.system_watts),
                ("CPU", &p.cpu_watts),
                ("GPU", &p.gpu_watts),
                ("Neural Engine", &p.ane_watts),
            ] {
                y = kv(s, pw, y, k, format::reading(v, |w| format::watts(*w)));
            }
            graph(
                s,
                Rect::new(pw.x, y + 2.0, pw.w, (pw.max_y() - y - 2.0).max(10.0)),
                m,
                Series::SystemPower,
                None,
                Color::RED,
            );
        }
        Reading::Unavailable(u) => s.text(pw, format::unavailable(u), tertiary(11.0)),
    }

    // Network.
    let net = panel(s, cell(0, 2, 1), "Réseau");
    match &snap.network {
        Reading::Measured(n) | Reading::Estimated(n) => {
            let mut y = net.y;
            y = kv(s, net, y, "Réception", format::rate(n.rx_bytes_per_sec));
            y = kv(s, net, y, "Envoi", format::rate(n.tx_bytes_per_sec));
            y = kv(
                s,
                net,
                y,
                "Interface",
                n.primary_interface.clone().unwrap_or_else(|| "—".into()),
            );
            let gr = Rect::new(net.x, y + 2.0, net.w, (net.max_y() - y - 2.0).max(10.0));
            graph(s, gr, m, Series::NetRx, None, Color::ORANGE);
            sparkline(
                s,
                gr.inset(2.0, 2.0),
                &m.history,
                Series::NetTx,
                None,
                Color::ACCENT,
            );
        }
        Reading::Unavailable(u) => s.text(net, format::unavailable(u), tertiary(11.0)),
    }

    // Sensors.
    let sen = panel(s, cell(1, 2, 1), "Capteurs");
    match &snap.sensors {
        Reading::Measured(list) | Reading::Estimated(list) if !list.is_empty() => {
            let mut y = sen.y;
            let rows = ((sen.h) / 17.0).floor() as usize;
            for x in list.iter().take(rows) {
                y = kv(
                    s,
                    sen,
                    y,
                    &format!("{} · {}", x.group, x.label),
                    format::celsius(x.celsius),
                );
            }
        }
        Reading::Unavailable(u) => s.text(sen, format::unavailable(u), tertiary(11.0)),
        _ => s.text(sen, "Aucun capteur", tertiary(11.0)),
    }
    let fans = format::reading(&snap.fans_rpm, |f| {
        if f.is_empty() {
            "aucun".into()
        } else {
            f.iter()
                .map(|r| format!("{} tr/min", r.round()))
                .collect::<Vec<_>>()
                .join(", ")
        }
    });
    s.text(
        Rect::new(sen.x, sen.max_y() - 2.0, sen.w, 13.0),
        format!("Ventilateurs : {fans}"),
        tertiary(9.5),
    );

    // Disks.
    let dk = panel(s, cell(2, 2, 1), "Disques");
    match &snap.disks {
        Reading::Measured(d) | Reading::Estimated(d) if !d.is_empty() => {
            let mut y = dk.y;
            for disk in d.iter().take(3) {
                s.text(
                    Rect::new(dk.x, y, dk.w, 14.0),
                    &disk.name,
                    label(11.0).weight(Weight::Medium),
                );
                let used = disk.total.saturating_sub(disk.available);
                bar(
                    s,
                    Rect::new(dk.x, y + 17.0, dk.w, 5.0),
                    (disk.total > 0).then(|| used as f64 / disk.total as f64),
                    Color::ACCENT,
                );
                s.text(
                    Rect::new(dk.x, y + 24.0, dk.w, 13.0),
                    format!(
                        "{} libres sur {}",
                        format::bytes(disk.available as f64),
                        format::bytes(disk.total as f64)
                    ),
                    tertiary(10.0),
                );
                y += 42.0;
            }
        }
        Reading::Unavailable(u) => s.text(dk, format::unavailable(u), tertiary(11.0)),
        _ => s.text(dk, "Aucun volume", tertiary(11.0)),
    }

    let _ = settings;
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_core::Unavailable;
    use nox_core::model::metrics::{CoreUsage, MetricsState};

    #[test]
    fn monitor_labels_cores_and_reasons() {
        let mut m = MetricsState::default();
        m.snapshot.cpu_cores = Reading::Measured(vec![
            CoreUsage {
                index: 0,
                kind: CoreKind::Efficiency,
                usage: 10.0,
            },
            CoreUsage {
                index: 1,
                kind: CoreKind::Performance,
                usage: 90.0,
            },
        ]);
        m.snapshot.fans_rpm = Reading::Unavailable(Unavailable::NotSupported);
        let mut s = Scene::default();
        render(
            &mut s,
            Rect::new(0.0, 0.0, 900.0, 600.0),
            Some(&m),
            &Settings::default(),
        );
        assert!(s.has_text("P") && s.has_text("E"));
        assert!(s.has_text("Ventilateurs : Indisponible"));
        let mut s = Scene::default();
        render(
            &mut s,
            Rect::new(0.0, 0.0, 900.0, 600.0),
            None,
            &Settings::default(),
        );
        assert!(s.has_text("Mesures indisponibles"));
    }
}
