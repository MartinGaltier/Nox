//! NSAppleScript (main thread) and the Automation permission check.

use nox_core::permissions::PermissionState;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSAppleScript, NSDictionary, NSNumber, NSString};

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptError {
    /// errAEEventNotPermitted (-1743): Automation denied for this target.
    NotAuthorized,
    /// procNotFound (-600) / app not running.
    AppNotRunning,
    /// errAETimeout (-1712).
    Timeout,
    Other(i64, String),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAuthorized => write!(f, "Automation not authorised"),
            Self::AppNotRunning => write!(f, "application not running"),
            Self::Timeout => write!(f, "Apple Event timed out"),
            Self::Other(c, m) => write!(f, "AppleScript error {c}: {m}"),
        }
    }
}

fn classify(code: i64, message: String) -> ScriptError {
    match code {
        -1743 | -1744 => ScriptError::NotAuthorized,
        -600 | -609 | -10810 => ScriptError::AppNotRunning,
        -1712 => ScriptError::Timeout,
        _ => ScriptError::Other(code, message),
    }
}

/// Runs `source` on the main thread and returns the result's string value.
/// Scripts must wrap their Apple Events in `with timeout of N seconds` so a
/// hung target cannot block the main thread for long.
pub fn run(source: &str) -> Result<Option<String>, ScriptError> {
    let source = source.to_owned();
    nox_core::main_thread::run_sync(move || run_here(&source))
}

fn run_here(source: &str) -> Result<Option<String>, ScriptError> {
    let src = NSString::from_str(source);
    let Some(script) = NSAppleScript::initWithSource(NSAppleScript::alloc(), &src) else {
        return Err(ScriptError::Other(0, "invalid script".into()));
    };
    let mut info: Option<Retained<NSDictionary<NSString, AnyObject>>> = None;
    // SAFETY: called on the main thread (via `run`); `info` is a valid
    // out-parameter for the error dictionary.
    let result = unsafe { script.executeAndReturnError(Some(&mut info)) };
    if let Some(info) = info {
        let code = info
            .objectForKey(&NSString::from_str("NSAppleScriptErrorNumber"))
            .and_then(|v| v.downcast_ref::<NSNumber>().map(|n| n.longLongValue()))
            .unwrap_or(0);
        let message = info
            .objectForKey(&NSString::from_str("NSAppleScriptErrorMessage"))
            .and_then(|v| v.downcast_ref::<NSString>().map(|s| s.to_string()))
            .unwrap_or_default();
        return Err(classify(code, message));
    }
    Ok(result.stringValue().map(|s| s.to_string()))
}

#[repr(C)]
struct AEDesc {
    descriptor_type: u32,
    data_handle: *mut std::ffi::c_void,
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn AECreateDesc(
        type_code: u32,
        data: *const std::ffi::c_void,
        size: isize,
        result: *mut AEDesc,
    ) -> i16;
    fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
    fn AEDeterminePermissionToAutomateTarget(
        target: *const AEDesc,
        event_class: u32,
        event_id: u32,
        ask_user_if_needed: u8,
    ) -> i32;
}

const TYPE_APPLICATION_BUNDLE_ID: u32 = u32::from_be_bytes(*b"bund");
const TYPE_WILDCARD: u32 = u32::from_be_bytes(*b"****");

/// Automation permission towards `bundle_id`. With `ask == false` this never
/// shows a prompt. With `ask == true` it may block until the user answers, so
/// never call it on the main thread.
pub fn automation_permission(bundle_id: &str, ask: bool) -> PermissionState {
    let bytes = bundle_id.as_bytes();
    let mut desc = AEDesc {
        descriptor_type: 0,
        data_handle: std::ptr::null_mut(),
    };
    // SAFETY: `bytes` outlives the call; AECreateDesc copies the data.
    let err = unsafe {
        AECreateDesc(
            TYPE_APPLICATION_BUNDLE_ID,
            bytes.as_ptr().cast(),
            bytes.len() as isize,
            &mut desc,
        )
    };
    if err != 0 {
        return PermissionState::NotDetermined;
    }
    // SAFETY: `desc` was initialised above.
    let status = unsafe {
        AEDeterminePermissionToAutomateTarget(&desc, TYPE_WILDCARD, TYPE_WILDCARD, u8::from(ask))
    };
    // SAFETY: disposes the descriptor created above exactly once.
    unsafe { AEDisposeDesc(&mut desc) };
    match status {
        0 => PermissionState::Granted,
        -1743 => PermissionState::Denied,
        -1744 => PermissionState::NotDetermined,
        // procNotFound: cannot tell until the app runs.
        _ => PermissionState::NotDetermined,
    }
}

/// Escapes a Rust string for inclusion in an AppleScript string literal.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_are_escaped() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(quote("x\ny"), "\"x y\"");
    }

    #[test]
    fn error_codes_are_classified() {
        assert_eq!(classify(-1743, String::new()), ScriptError::NotAuthorized);
        assert_eq!(classify(-600, String::new()), ScriptError::AppNotRunning);
        assert!(matches!(
            classify(-2753, "x".into()),
            ScriptError::Other(-2753, _)
        ));
    }
}
