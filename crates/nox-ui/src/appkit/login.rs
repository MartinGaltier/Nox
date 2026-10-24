//! Launch at login through `SMAppService.mainAppService` (macOS 13+). The
//! framework is called dynamically so no extra crate is needed. An ad-hoc
//! signed build may be refused by macOS: the real error is returned and shown.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::NSError;

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginStatus {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
    Unsupported,
}

fn service() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"SMAppService")?;
    // SAFETY: +[SMAppService mainAppService] returns a non-null object.
    let s: Retained<AnyObject> = unsafe { msg_send![cls, mainAppService] };
    Some(s)
}

pub fn status() -> LoginStatus {
    let Some(s) = service() else {
        return LoginStatus::Unsupported;
    };
    // SAFETY: `status` returns SMAppServiceStatus (NSInteger).
    let st: isize = unsafe { msg_send![&*s, status] };
    match st {
        0 => LoginStatus::NotRegistered,
        1 => LoginStatus::Enabled,
        2 => LoginStatus::RequiresApproval,
        _ => LoginStatus::NotFound,
    }
}

pub fn set(enabled: bool) -> Result<LoginStatus, String> {
    let Some(s) = service() else {
        return Err("SMAppService indisponible sur cette version de macOS".into());
    };
    // SAFETY: (un)registerAndReturnError: returns BOOL and fills an NSError.
    let r: Result<(), Retained<NSError>> = unsafe {
        if enabled {
            msg_send![&*s, registerAndReturnError: _]
        } else {
            msg_send![&*s, unregisterAndReturnError: _]
        }
    };
    match r {
        Ok(()) => Ok(status()),
        Err(e) => Err(e.localizedDescription().to_string()),
    }
}
