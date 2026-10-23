//! Honest values: every value shown to the user says whether it was measured,
//! estimated, or is unavailable. Never substitute an error with `0`.

use serde::{Deserialize, Serialize};

/// Why a value cannot be shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unavailable {
    /// Not exposed by this hardware / macOS version.
    NotSupported,
    /// A permission is missing (the string names it, e.g. "Calendars").
    PermissionDenied(String),
    /// The source app/service is not running or not installed.
    SourceMissing(String),
    /// Collection is switched off in settings.
    Disabled,
    /// First sample not yet taken.
    Pending,
    /// The source returned an error.
    Error(String),
}

impl Unavailable {
    pub fn label_fr(&self) -> String {
        match self {
            Unavailable::NotSupported => "Indisponible".into(),
            Unavailable::PermissionDenied(p) => format!("Autorisation requise : {p}"),
            Unavailable::SourceMissing(s) => format!("{s} absent"),
            Unavailable::Disabled => "Désactivé".into(),
            Unavailable::Pending => "…".into(),
            Unavailable::Error(_) => "Indisponible".into(),
        }
    }
}

/// A value with its provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reading<T> {
    /// Read directly from a system or app source.
    Measured(T),
    /// Derived / modelled (e.g. cost from token counts, time remaining).
    Estimated(T),
    Unavailable(Unavailable),
}

impl<T> Reading<T> {
    pub fn pending() -> Self {
        Reading::Unavailable(Unavailable::Pending)
    }
    pub fn value(&self) -> Option<&T> {
        match self {
            Reading::Measured(v) | Reading::Estimated(v) => Some(v),
            Reading::Unavailable(_) => None,
        }
    }
    pub fn is_estimated(&self) -> bool {
        matches!(self, Reading::Estimated(_))
    }
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Reading<U> {
        match self {
            Reading::Measured(v) => Reading::Measured(f(v)),
            Reading::Estimated(v) => Reading::Estimated(f(v)),
            Reading::Unavailable(u) => Reading::Unavailable(u),
        }
    }
    pub fn as_ref(&self) -> Reading<&T> {
        match self {
            Reading::Measured(v) => Reading::Measured(v),
            Reading::Estimated(v) => Reading::Estimated(v),
            Reading::Unavailable(u) => Reading::Unavailable(u.clone()),
        }
    }
    /// Converts a fallible source into a measured reading.
    pub fn from_result<E: std::fmt::Display>(r: Result<T, E>) -> Self {
        match r {
            Ok(v) => Reading::Measured(v),
            Err(e) => Reading::Unavailable(Unavailable::Error(e.to_string())),
        }
    }
}

impl<T> Default for Reading<T> {
    fn default() -> Self {
        Reading::pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_is_not_zero() {
        let r: Reading<f64> = Reading::from_result(Err::<f64, _>("boom"));
        assert_eq!(r.value(), None);
    }

    #[test]
    fn map_keeps_provenance() {
        assert!(Reading::Estimated(2).map(|v| v * 2).is_estimated());
    }
}
