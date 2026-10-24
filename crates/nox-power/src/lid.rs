//! Closed-lid control through the privileged helper (client side).
//!
//! The app holds one persistent connection while a closed-lid session runs
//! and heartbeats on it. If the app dies, the kernel closes the socket and the
//! helper restores lid sleep after the lease — nothing here needs to run.

use nox_core::Reading;
use nox_core::model::power::HelperStatus;
use nox_core::reading::Unavailable;
use nox_helper::protocol::{ErrorCode, Request, Response, RestoreOutcome, RevokeReason};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// Grace the helper gives us without heartbeat / after a disconnect.
pub const LEASE_SECS: u32 = 60;
/// Heartbeat period (a quarter of the lease).
pub const HEARTBEAT_SECS: f64 = 15.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LidError {
    NotInstalled,
    Unreachable(String),
    Forbidden(String),
    Revoked(RevokeReason),
    Refused { code: ErrorCode, detail: String },
    Protocol(String),
}

impl LidError {
    pub fn message_fr(&self) -> String {
        match self {
            Self::NotInstalled => {
                "Le mode capot fermé nécessite l’assistant : installez-le depuis Maintien éveillé."
                    .into()
            }
            Self::Unreachable(e) => format!("Assistant capot fermé injoignable ({e})."),
            Self::Forbidden(_) => {
                "L’assistant a été installé pour une autre copie de Nox : réinstallez-le.".into()
            }
            Self::Revoked(r) => match r {
                RevokeReason::Battery => {
                    "Batterie sous le seuil : veille capot fermé rétablie.".into()
                }
                RevokeReason::Thermal => {
                    "Température élevée : veille capot fermé rétablie par sécurité.".into()
                }
                RevokeReason::External => {
                    "Le réglage de veille a été modifié en dehors de Nox.".into()
                }
                _ => "L’assistant a rétabli la veille capot fermé.".into(),
            },
            Self::Refused { code, detail } => match code {
                ErrorCode::Battery => format!("Refusé : batterie sous le seuil ({detail})."),
                ErrorCode::Thermal => "Refusé : température élevée.".into(),
                ErrorCode::Busy => "Assistant occupé par une autre session.".into(),
                _ => format!("L’assistant a échoué : {detail}"),
            },
            Self::Protocol(e) => format!("Réponse inattendue de l’assistant ({e})."),
        }
    }
}

pub trait LidControl: Send {
    /// Detected without privileges (files + a `version` round trip).
    fn status(&mut self) -> HelperStatus;
    /// `SleepDisabled` read directly from the IORegistry.
    fn sleep_disabled(&mut self) -> Reading<bool>;
    /// Disables lid-close sleep for this session. `Ok(applied)`: `false` when
    /// it was already disabled outside Nox (left untouched).
    fn acquire(&mut self, min_battery: u8) -> Result<bool, LidError>;
    fn heartbeat(&mut self) -> Result<(), LidError>;
    fn release(&mut self) -> Result<RestoreOutcome, LidError>;
}

/// App-side record of a held lease, so the next launch knows a previous run
/// ended without releasing (crash, kill -9, force quit).
#[derive(Debug, Clone)]
pub struct Marker {
    path: PathBuf,
}

impl Marker {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn default_location() -> Self {
        Self::new(
            nox_core::paths::app_support_dir()
                .join("power")
                .join("closed-lid-lease.json"),
        )
    }
    pub fn set(&self) {
        let body = serde_json::json!({
            "pid": std::process::id(),
            "since": nox_core::model::now(),
        });
        if let Err(e) = nox_core::paths::write_atomic(&self.path, body.to_string().as_bytes()) {
            log::warn!("closed-lid marker not written: {e}");
        }
    }
    pub fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
    pub fn exists(&self) -> bool {
        self.path.exists()
    }
}

pub struct HelperClient {
    socket: PathBuf,
    installed: Box<dyn Fn() -> bool + Send>,
    conn: Option<BufReader<UnixStream>>,
    holds_lease: bool,
    marker: Option<Marker>,
}

const IO_TIMEOUT: Duration = Duration::from_secs(25);

impl HelperClient {
    /// The installed helper.
    pub fn system() -> Self {
        Self {
            socket: PathBuf::from(nox_helper::SOCKET_PATH),
            installed: Box::new(crate::install::helper_files_present),
            conn: None,
            holds_lease: false,
            marker: Some(Marker::default_location()),
        }
    }

    /// A helper served on `socket` (tests: unprivileged server + fake backend).
    pub fn with_socket(socket: PathBuf, marker: Option<Marker>) -> Self {
        Self {
            socket,
            installed: Box::new(|| true),
            conn: None,
            holds_lease: false,
            marker,
        }
    }

    fn connect(&mut self) -> Result<&mut BufReader<UnixStream>, LidError> {
        if self.conn.is_none() {
            if !(self.installed)() {
                return Err(LidError::NotInstalled);
            }
            let s = UnixStream::connect(&self.socket)
                .map_err(|e| LidError::Unreachable(e.to_string()))?;
            s.set_read_timeout(Some(IO_TIMEOUT))
                .and_then(|_| s.set_write_timeout(Some(IO_TIMEOUT)))
                .map_err(|e| LidError::Unreachable(e.to_string()))?;
            self.conn = Some(BufReader::new(s));
        }
        Ok(self.conn.as_mut().expect("just connected"))
    }

    fn request(&mut self, req: Request) -> Result<Response, LidError> {
        let r = self.round_trip(req);
        if matches!(r, Err(LidError::Unreachable(_))) {
            self.conn = None;
        }
        let resp = r?;
        match resp {
            Response::Err {
                code: ErrorCode::Forbidden,
                detail,
            } => {
                self.conn = None;
                Err(LidError::Forbidden(detail))
            }
            Response::Err {
                code: ErrorCode::Revoked(r),
                ..
            } => Err(LidError::Revoked(r)),
            Response::Err { code, detail } => Err(LidError::Refused { code, detail }),
            ok => Ok(ok),
        }
    }

    fn round_trip(&mut self, req: Request) -> Result<Response, LidError> {
        let conn = self.connect()?;
        conn.get_mut()
            .write_all(format!("{}\n", req.encode()).as_bytes())
            .map_err(|e| LidError::Unreachable(e.to_string()))?;
        let mut line = String::new();
        match conn.read_line(&mut line) {
            Ok(0) => Err(LidError::Unreachable("connection closed".into())),
            Ok(_) => Response::parse(line.trim_end()).map_err(LidError::Protocol),
            Err(e) => Err(LidError::Unreachable(e.to_string())),
        }
    }

    fn drop_idle_connection(&mut self) {
        if !self.holds_lease {
            self.conn = None;
        }
    }
}

impl LidControl for HelperClient {
    fn status(&mut self) -> HelperStatus {
        if !(self.installed)() {
            return HelperStatus::NotInstalled;
        }
        let s = match self.request(Request::Version) {
            Ok(Response::Version { protocol, version }) => {
                if protocol == nox_helper::PROTOCOL_VERSION && version == nox_helper::VERSION {
                    HelperStatus::Installed
                } else {
                    HelperStatus::Outdated
                }
            }
            Err(LidError::Forbidden(_)) => HelperStatus::ClientRejected,
            Err(LidError::NotInstalled) => HelperStatus::NotInstalled,
            _ => HelperStatus::Unreachable,
        };
        self.drop_idle_connection();
        s
    }

    fn sleep_disabled(&mut self) -> Reading<bool> {
        match nox_helper::sys::sleep_disabled() {
            Ok(v) => Reading::Measured(v),
            Err(e) => Reading::Unavailable(Unavailable::Error(e)),
        }
    }

    fn acquire(&mut self, min_battery: u8) -> Result<bool, LidError> {
        let r = self.request(Request::Acquire {
            lease_secs: LEASE_SECS,
            min_battery,
        });
        match r {
            Ok(Response::Acquired { applied }) => {
                self.holds_lease = true;
                if let Some(m) = &self.marker {
                    m.set();
                }
                Ok(applied)
            }
            Ok(other) => {
                self.drop_idle_connection();
                Err(LidError::Protocol(other.encode()))
            }
            Err(e) => {
                self.drop_idle_connection();
                Err(e)
            }
        }
    }

    fn heartbeat(&mut self) -> Result<(), LidError> {
        if self.conn.is_none() {
            return Err(LidError::Unreachable("not connected".into()));
        }
        match self.request(Request::Heartbeat)? {
            Response::Heartbeat => Ok(()),
            other => Err(LidError::Protocol(other.encode())),
        }
    }

    fn release(&mut self) -> Result<RestoreOutcome, LidError> {
        let r = match self.request(Request::Release) {
            Ok(Response::Released(o)) => Ok(o),
            Ok(other) => Err(LidError::Protocol(other.encode())),
            Err(e) => Err(e),
        };
        // Whatever happened, our side of the lease is over: closing the
        // connection lets the helper restore after the grace if the release
        // itself did not get through.
        self.holds_lease = false;
        self.conn = None;
        // The marker stays when the outcome is unknown or failed, so the next
        // launch reconciles with the helper.
        if let Some(m) = &self.marker
            && matches!(r, Ok(o) if o != RestoreOutcome::Failed)
        {
            m.clear();
        }
        r
    }
}
