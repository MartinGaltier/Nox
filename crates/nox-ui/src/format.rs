//! French formatting of durations, sizes, rates and readings.

use nox_core::{Reading, Unavailable};

/// Narrow no-break space used before units in French.
pub const NNBSP: char = '\u{202F}';

/// `m:ss` or `h:mm:ss`.
pub fn clock(secs: f64) -> String {
    let s = if secs.is_finite() {
        secs.max(0.0).floor() as u64
    } else {
        0
    };
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Human duration: "45 s", "12 min", "1 h 05".
pub fn duration(secs: f64) -> String {
    let s = if secs.is_finite() {
        secs.max(0.0).round() as u64
    } else {
        0
    };
    if s < 60 {
        format!("{s}{NNBSP}s")
    } else if s < 3600 {
        format!("{}{NNBSP}min", s.div_ceil(60).min(59).max(s / 60))
    } else {
        format!("{}{NNBSP}h{NNBSP}{:02}", s / 3600, (s % 3600) / 60)
    }
}

/// Decimal with a French comma.
pub fn decimal(v: f64, digits: usize) -> String {
    format!("{v:.digits$}").replace('.', ",")
}

/// Bytes in French units (o, Ko, Mo, Go, To; powers of 1000 like Finder).
pub fn bytes(b: f64) -> String {
    if !b.is_finite() || b < 0.0 {
        return "—".into();
    }
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    let mut v = b;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    let digits = if i == 0 || v >= 100.0 {
        0
    } else if v >= 10.0 {
        1
    } else {
        2.min(1 + (i > 1) as usize)
    };
    let mut n = decimal(v, digits);
    if n.contains(',') {
        n = n.trim_end_matches('0').trim_end_matches(',').to_string();
    }
    format!("{n}{NNBSP}{}", UNITS[i])
}

pub fn rate(bps: f64) -> String {
    format!("{}/s", bytes(bps))
}

pub fn percent(v: f64) -> String {
    if v.is_finite() {
        format!("{}{NNBSP}%", v.round() as i64)
    } else {
        "—".into()
    }
}

pub fn celsius(v: f64) -> String {
    format!("{}{NNBSP}°C", v.round() as i64)
}

pub fn watts(v: f64) -> String {
    format!("{}{NNBSP}W", decimal(v, 1))
}

/// Text of a reading: value formatted by `f`, " (estimé)" when estimated,
/// the reason when unavailable. Never shows 0 for an error.
pub fn reading<T>(r: &Reading<T>, f: impl FnOnce(&T) -> String) -> String {
    match r {
        Reading::Measured(v) => f(v),
        Reading::Estimated(v) => format!("{} (estimé)", f(v)),
        Reading::Unavailable(u) => unavailable(u),
    }
}

pub fn unavailable(u: &Unavailable) -> String {
    match u {
        Unavailable::Error(e) if !e.is_empty() => {
            let short: String = e.chars().take(40).collect();
            format!("Indisponible ({short})")
        }
        Unavailable::Pending => "En attente de mesure".into(),
        other => other.label_fr(),
    }
}

/// Like [`unavailable`] but keeps the whole error message (shown on two lines).
pub fn unavailable_full(u: &Unavailable) -> String {
    match u {
        Unavailable::Error(e) if !e.is_empty() => e.clone(),
        other => unavailable(other),
    }
}

/// "dans 5 min", "dans 2 h 10", "maintenant", "il y a 3 min".
pub fn relative(target: f64, now: f64) -> String {
    let d = target - now;
    if d.abs() < 45.0 {
        "maintenant".into()
    } else if d > 0.0 {
        format!("dans {}", duration(d))
    } else {
        format!("il y a {}", duration(-d))
    }
}

/// Local wall-clock "HH:MM" of a Unix timestamp.
pub fn hour_minute(ts: f64) -> String {
    let (h, m, _) = local_hms(ts);
    format!("{h:02}:{m:02}")
}

/// Local time of day via libc `localtime_r` (respects the user's time zone).
pub fn local_hms(ts: f64) -> (u32, u32, u32) {
    let t = local_tm(ts);
    (t.0, t.1, t.2)
}

/// (hour, minute, second, day-of-year, year) in local time.
fn local_tm(ts: f64) -> (u32, u32, u32, i32, i32) {
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        gmtoff: i64,
        zone: *const i8,
    }
    unsafe extern "C" {
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let secs = if ts.is_finite() { ts.floor() as i64 } else { 0 };
    let mut tm = Tm {
        sec: 0,
        min: 0,
        hour: 0,
        mday: 0,
        mon: 0,
        year: 0,
        wday: 0,
        yday: 0,
        isdst: 0,
        gmtoff: 0,
        zone: std::ptr::null(),
    };
    // SAFETY: `secs` and `tm` are valid for the call; localtime_r is the
    // re-entrant variant and writes only into `tm`.
    let ok = unsafe { !localtime_r(&secs, &mut tm).is_null() };
    if !ok {
        return (0, 0, 0, 0, 1970);
    }
    (
        tm.hour as u32,
        tm.min as u32,
        tm.sec as u32,
        tm.yday,
        tm.year + 1900,
    )
}

/// Same local calendar day.
pub fn same_day(a: f64, b: f64) -> bool {
    let (x, y) = (local_tm(a), local_tm(b));
    x.3 == y.3 && x.4 == y.4
}

/// "Aujourd’hui", "Demain" or a short weekday.
pub fn day_label(ts: f64, now: f64) -> String {
    if same_day(ts, now) {
        "Aujourd’hui".into()
    } else if same_day(ts, now + 86_400.0) {
        "Demain".into()
    } else {
        let days = ((ts - now) / 86_400.0).ceil() as i64;
        format!("Dans {days}{NNBSP}j")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(65.9), "1:05");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(f64::NAN), "0:00");
        assert_eq!(clock(-4.0), "0:00");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(42.0), format!("42{NNBSP}s"));
        assert_eq!(duration(600.0), format!("10{NNBSP}min"));
        assert_eq!(duration(3900.0), format!("1{NNBSP}h{NNBSP}05"));
    }

    #[test]
    fn byte_units() {
        assert_eq!(bytes(512.0), format!("512{NNBSP}o"));
        assert_eq!(bytes(1_500_000.0), format!("1,5{NNBSP}Mo"));
        assert_eq!(bytes(12_340_000_000.0), format!("12,3{NNBSP}Go"));
        assert_eq!(bytes(250_000_000_000.0), format!("250{NNBSP}Go"));
        assert_eq!(bytes(-1.0), "—");
    }

    #[test]
    fn readings_are_honest() {
        let m: Reading<f64> = Reading::Measured(41.6);
        let e: Reading<f64> = Reading::Estimated(2.5);
        let u: Reading<f64> = Reading::Unavailable(Unavailable::NotSupported);
        let err: Reading<f64> = Reading::Unavailable(Unavailable::Error("SMC".into()));
        assert_eq!(reading(&m, |v| percent(*v)), format!("42{NNBSP}%"));
        assert_eq!(reading(&e, |v| decimal(*v, 2)), "2,50 (estimé)");
        assert_eq!(reading(&u, |v| percent(*v)), "Indisponible");
        assert_eq!(reading(&err, |v| percent(*v)), "Indisponible (SMC)");
        let p: Reading<f64> = Reading::pending();
        assert_ne!(reading(&p, |v| percent(*v)), format!("0{NNBSP}%"));
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative(100.0, 90.0), "maintenant");
        assert_eq!(relative(400.0, 100.0), format!("dans 5{NNBSP}min"));
        assert_eq!(relative(0.0, 300.0), format!("il y a 5{NNBSP}min"));
        assert!(same_day(1_700_000_000.0, 1_700_000_001.0));
    }
}
