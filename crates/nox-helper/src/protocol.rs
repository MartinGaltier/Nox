//! Wire protocol: one ASCII request line, one ASCII response line.
//!
//! Requests (strict whitelist, anything else is `err bad-request`):
//!
//! ```text
//! version
//! status
//! acquire <lease_secs 5..=600> <min_battery_percent 0..=100>
//! heartbeat
//! release
//! ```
//!
//! Responses:
//!
//! ```text
//! ok version <protocol> <crate version>
//! ok status sleep_disabled=<0|1|?> lease=<none|held|orphaned> applied=<0|1> revoked=<reason|none>
//! ok acquired applied=<0|1>
//! ok heartbeat
//! ok released <restored|untouched|nothing|failed>
//! err <code> [detail…]
//! ```
//!
//! A lease is the client's promise to send `heartbeat` at least every
//! `lease_secs`; if it stops (or its connection closes and it does not come
//! back within `lease_secs`), the helper restores the setting itself.

use std::fmt;

/// Longest accepted request line (bytes, without the newline).
pub const MAX_LINE: usize = 128;
pub const MIN_LEASE_SECS: u32 = 5;
pub const MAX_LEASE_SECS: u32 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Version,
    Status,
    Acquire { lease_secs: u32, min_battery: u8 },
    Heartbeat,
    Release,
}

impl Request {
    pub fn parse(line: &str) -> Result<Self, String> {
        if line.len() > MAX_LINE || !line.is_ascii() {
            return Err("line too long or not ASCII".into());
        }
        let mut it = line.split_ascii_whitespace();
        let verb = it.next().ok_or("empty request")?;
        let req = match verb {
            "version" => Request::Version,
            "status" => Request::Status,
            "heartbeat" => Request::Heartbeat,
            "release" => Request::Release,
            "acquire" => {
                let lease_secs: u32 = parse_num(it.next())?;
                let min_battery: u8 = parse_num(it.next())?;
                if !(MIN_LEASE_SECS..=MAX_LEASE_SECS).contains(&lease_secs) {
                    return Err(format!(
                        "lease must be {MIN_LEASE_SECS}..={MAX_LEASE_SECS} s"
                    ));
                }
                if min_battery > 100 {
                    return Err("battery percent must be 0..=100".into());
                }
                Request::Acquire {
                    lease_secs,
                    min_battery,
                }
            }
            _ => return Err("unknown request".into()),
        };
        if it.next().is_some() {
            return Err("unexpected argument".into());
        }
        Ok(req)
    }

    pub fn encode(&self) -> String {
        match self {
            Request::Version => "version".into(),
            Request::Status => "status".into(),
            Request::Heartbeat => "heartbeat".into(),
            Request::Release => "release".into(),
            Request::Acquire {
                lease_secs,
                min_battery,
            } => format!("acquire {lease_secs} {min_battery}"),
        }
    }
}

fn parse_num<T: std::str::FromStr>(s: Option<&str>) -> Result<T, String> {
    let s = s.ok_or("missing argument")?;
    // Digits only: no sign, no hex, no whitespace tricks.
    if s.is_empty() || s.len() > 3 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid number".into());
    }
    s.parse().map_err(|_| "invalid number".into())
}

/// Why the helper ended a lease on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeReason {
    /// No heartbeat within the lease.
    LeaseExpired,
    /// The client connection closed and did not come back within the lease.
    ClientGone,
    /// On battery at or below the client's threshold.
    Battery,
    /// Thermal pressure heavy or worse.
    Thermal,
    /// The setting was changed outside Nox (e.g. `sudo pmset disablesleep 0`).
    External,
    /// The helper is stopping (uninstall, upgrade, shutdown).
    Shutdown,
}

impl RevokeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LeaseExpired => "lease-expired",
            Self::ClientGone => "client-gone",
            Self::Battery => "battery",
            Self::Thermal => "thermal",
            Self::External => "external",
            Self::Shutdown => "shutdown",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "lease-expired" => Self::LeaseExpired,
            "client-gone" => Self::ClientGone,
            "battery" => Self::Battery,
            "thermal" => Self::Thermal,
            "external" => Self::External,
            "shutdown" => Self::Shutdown,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    None,
    /// Held by a connected client.
    Held,
    /// The owner disconnected; the helper restores when the grace runs out.
    Orphaned,
}

/// Result of a restore attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// Nox had set `SleepDisabled`; it is now back to 0.
    Restored,
    /// Nothing to undo: the setting was already 1 before Nox, or someone else
    /// changed it since — left exactly as found.
    Untouched,
    /// No lease / journal.
    Nothing,
    /// `pmset` failed; the journal is kept and the helper retries.
    Failed,
}

impl RestoreOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restored => "restored",
            Self::Untouched => "untouched",
            Self::Nothing => "nothing",
            Self::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "restored" => Self::Restored,
            "untouched" => Self::Untouched,
            "nothing" => Self::Nothing,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// `None` when the state could not be read.
    pub sleep_disabled: Option<bool>,
    pub lease: LeaseState,
    /// Whether the current lease changed the setting (false when it was
    /// already disabled before Nox).
    pub applied: bool,
    /// Why the last lease ended, if the helper ended it.
    pub revoked: Option<RevokeReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest,
    /// Peer verification failed.
    Forbidden,
    NoLease,
    /// The lease was ended by the helper.
    Revoked(RevokeReason),
    /// Another user's client holds the lease.
    Busy,
    /// Refused: on battery at or below the threshold.
    Battery,
    /// Refused: thermal pressure heavy or worse.
    Thermal,
    /// Reading or changing the setting failed.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Version { protocol: u32, version: String },
    Status(Status),
    Acquired { applied: bool },
    Heartbeat,
    Released(RestoreOutcome),
    Err { code: ErrorCode, detail: String },
}

impl Response {
    pub fn err(code: ErrorCode, detail: impl Into<String>) -> Self {
        Response::Err {
            code,
            detail: sanitize(&detail.into()),
        }
    }

    pub fn encode(&self) -> String {
        match self {
            Response::Version { protocol, version } => {
                format!("ok version {protocol} {}", sanitize(version))
            }
            Response::Status(s) => format!(
                "ok status sleep_disabled={} lease={} applied={} revoked={}",
                match s.sleep_disabled {
                    Some(true) => "1",
                    Some(false) => "0",
                    None => "?",
                },
                match s.lease {
                    LeaseState::None => "none",
                    LeaseState::Held => "held",
                    LeaseState::Orphaned => "orphaned",
                },
                u8::from(s.applied),
                s.revoked.map_or("none", RevokeReason::as_str),
            ),
            Response::Acquired { applied } => format!("ok acquired applied={}", u8::from(*applied)),
            Response::Heartbeat => "ok heartbeat".into(),
            Response::Released(o) => format!("ok released {}", o.as_str()),
            Response::Err { code, detail } => {
                let code = match code {
                    ErrorCode::BadRequest => "bad-request".to_string(),
                    ErrorCode::Forbidden => "forbidden".to_string(),
                    ErrorCode::NoLease => "no-lease".to_string(),
                    ErrorCode::Revoked(r) => format!("revoked {}", r.as_str()),
                    ErrorCode::Busy => "busy".to_string(),
                    ErrorCode::Battery => "battery".to_string(),
                    ErrorCode::Thermal => "thermal".to_string(),
                    ErrorCode::Failed => "failed".to_string(),
                };
                if detail.is_empty() {
                    format!("err {code}")
                } else {
                    format!("err {code} {detail}")
                }
            }
        }
    }

    pub fn parse(line: &str) -> Result<Self, String> {
        let bad = || format!("unexpected response: {line:?}");
        let mut it = line.split_ascii_whitespace();
        match (it.next(), it.next()) {
            (Some("ok"), Some("version")) => {
                let protocol = it.next().and_then(|p| p.parse().ok()).ok_or_else(bad)?;
                let version = it.next().ok_or_else(bad)?.to_string();
                Ok(Response::Version { protocol, version })
            }
            (Some("ok"), Some("status")) => {
                let mut s = Status {
                    sleep_disabled: None,
                    lease: LeaseState::None,
                    applied: false,
                    revoked: None,
                };
                for kv in it {
                    let (k, v) = kv.split_once('=').ok_or_else(bad)?;
                    match k {
                        "sleep_disabled" => {
                            s.sleep_disabled = match v {
                                "1" => Some(true),
                                "0" => Some(false),
                                _ => None,
                            }
                        }
                        "lease" => {
                            s.lease = match v {
                                "held" => LeaseState::Held,
                                "orphaned" => LeaseState::Orphaned,
                                _ => LeaseState::None,
                            }
                        }
                        "applied" => s.applied = v == "1",
                        "revoked" => s.revoked = RevokeReason::parse(v),
                        _ => {} // forward compatible
                    }
                }
                Ok(Response::Status(s))
            }
            (Some("ok"), Some("acquired")) => Ok(Response::Acquired {
                applied: it.next() == Some("applied=1"),
            }),
            (Some("ok"), Some("heartbeat")) => Ok(Response::Heartbeat),
            (Some("ok"), Some("released")) => Ok(Response::Released(
                it.next().and_then(RestoreOutcome::parse).ok_or_else(bad)?,
            )),
            (Some("err"), Some(code)) => {
                let code = match code {
                    "bad-request" => ErrorCode::BadRequest,
                    "forbidden" => ErrorCode::Forbidden,
                    "no-lease" => ErrorCode::NoLease,
                    "revoked" => {
                        ErrorCode::Revoked(it.next().and_then(RevokeReason::parse).ok_or_else(bad)?)
                    }
                    "busy" => ErrorCode::Busy,
                    "battery" => ErrorCode::Battery,
                    "thermal" => ErrorCode::Thermal,
                    _ => ErrorCode::Failed,
                };
                Ok(Response::Err {
                    code,
                    detail: it.collect::<Vec<_>>().join(" "),
                })
            }
            _ => Err(bad()),
        }
    }
}

/// Keeps details single-line printable ASCII.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .take(200)
        .collect()
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_only() {
        assert_eq!(Request::parse("status"), Ok(Request::Status));
        assert_eq!(
            Request::parse("acquire 60 20"),
            Ok(Request::Acquire {
                lease_secs: 60,
                min_battery: 20
            })
        );
        for bad in [
            "",
            "STATUS",
            "status now",
            "acquire",
            "acquire 60",
            "acquire 4 0",
            "acquire 601 0",
            "acquire 60 101",
            "acquire -1 0",
            "acquire 0x10 0",
            "acquire 60 20 extra",
            "pmset disablesleep 1",
            "release; rm -rf /",
            "statu\u{e9}",
        ] {
            assert!(Request::parse(bad).is_err(), "{bad:?} must be rejected");
        }
        let long = "a".repeat(MAX_LINE + 1);
        assert!(Request::parse(&long).is_err());
    }

    #[test]
    fn request_roundtrip() {
        for r in [
            Request::Version,
            Request::Status,
            Request::Heartbeat,
            Request::Release,
            Request::Acquire {
                lease_secs: 600,
                min_battery: 0,
            },
        ] {
            assert_eq!(Request::parse(&r.encode()), Ok(r));
        }
    }

    #[test]
    fn response_roundtrip() {
        let all = [
            Response::Version {
                protocol: 1,
                version: "0.1.0".into(),
            },
            Response::Status(Status {
                sleep_disabled: Some(true),
                lease: LeaseState::Orphaned,
                applied: true,
                revoked: None,
            }),
            Response::Status(Status {
                sleep_disabled: None,
                lease: LeaseState::None,
                applied: false,
                revoked: Some(RevokeReason::Battery),
            }),
            Response::Acquired { applied: false },
            Response::Heartbeat,
            Response::Released(RestoreOutcome::Untouched),
            Response::err(ErrorCode::Revoked(RevokeReason::ClientGone), ""),
            Response::err(ErrorCode::Failed, "pmset exited 1"),
        ];
        for r in all {
            assert_eq!(Response::parse(&r.encode()).as_ref(), Ok(&r), "{r}");
        }
    }

    #[test]
    fn details_are_single_line() {
        let r = Response::err(ErrorCode::Failed, "a\nb\u{0}c");
        assert_eq!(r.encode(), "err failed a_b_c");
    }
}
