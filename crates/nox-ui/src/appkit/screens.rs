// Fullscreen coverage logic adapted from vorssaint-utils (GPL-3.0-or-later),
// Sources/Vorssaint/Services/Notch/NotchAppFullscreen.swift, commit e916961.
//! Screen selection, raw notch parameters, and fullscreen detection.

use super::util::rect;
use crate::geom::Rect;
use crate::geometry::ScreenParams;
use nox_core::settings::DisplayChoice;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSScreen, NSStatusBar, NSWorkspace};
use objc2_core_foundation::CFArray;
use objc2_core_graphics::{CGDisplayIsBuiltin, CGWindowListCopyWindowInfo, CGWindowListOption};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

pub fn display_id(screen: &NSScreen) -> Option<u32> {
    let desc = screen.deviceDescription();
    let v = desc.objectForKey(&NSString::from_str("NSScreenNumber"))?;
    v.downcast_ref::<NSNumber>().map(|n| n.unsignedIntValue())
}

pub fn is_builtin(screen: &NSScreen) -> bool {
    display_id(screen).is_some_and(|d| CGDisplayIsBuiltin(d))
}

/// The screen hosting the island for a display choice.
pub fn choose(mtm: MainThreadMarker, choice: DisplayChoice) -> Option<Retained<NSScreen>> {
    let screens = NSScreen::screens(mtm);
    let builtin = screens.iter().find(|s| is_builtin(s));
    // The main display is the one with the menu bar: screens[0].
    let main = screens.firstObject();
    match choice {
        DisplayChoice::BuiltIn => builtin.or(main),
        DisplayChoice::Main => main,
        DisplayChoice::Automatic => builtin.or(main),
    }
}

pub fn params(screen: &NSScreen) -> ScreenParams {
    let frame = rect(screen.frame());
    let inset = screen.safeAreaInsets();
    let aux = |r: objc2_foundation::NSRect| {
        let r = rect(r);
        (r.w > 0.0 && r.h > 0.0).then_some(r)
    };
    let (left, right) = if inset.top > 0.0 {
        (
            aux(screen.auxiliaryTopLeftArea()),
            aux(screen.auxiliaryTopRightArea()),
        )
    } else {
        (None, None)
    };
    ScreenParams {
        frame,
        visible_frame: rect(screen.visibleFrame()),
        safe_area_top: inset.top,
        aux_left: left.map(|l| local(l, frame)),
        aux_right: right.map(|r| local(r, frame)),
        status_bar_thickness: NSStatusBar::systemStatusBar().thickness(),
    }
}

/// Auxiliary areas are documented in screen coordinates; normalise to
/// screen-local x so the geometry can centre on the housing.
fn local(r: Rect, frame: Rect) -> Rect {
    if r.x >= frame.x - 0.5 && r.max_x() <= frame.max_x() + 0.5 && frame.x != 0.0 {
        Rect::new(r.x - frame.x, r.y, r.w, r.h)
    } else {
        r
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowInfo {
    pub owner_pid: i32,
    pub layer: i64,
    pub alpha: f64,
    /// CG global space (top-left origin).
    pub bounds: Rect,
}

/// Whether the frontmost app covers `display` (CG space) with a window:
/// a native fullscreen Space (level 0, whole display incl. menu bar) or an
/// app-drawn fullscreen window raised above normal windows.
pub fn covers(windows: &[WindowInfo], frontmost: Option<i32>, own: i32, display: Rect) -> bool {
    let Some(front) = frontmost.filter(|p| *p != own) else {
        return false;
    };
    if display.w <= 0.0 {
        return false;
    }
    windows.iter().any(|w| {
        w.owner_pid == front
            && w.alpha > 0.0
            && w.layer >= 0
            && w.bounds.x <= display.x + 1.0
            && w.bounds.y <= display.y + 1.0
            && w.bounds.max_x() >= display.max_x() - 1.0
            && w.bounds.max_y() >= display.max_y() - 1.0
    })
}

fn num(d: &NSDictionary<NSString, AnyObject>, k: &str) -> Option<f64> {
    d.objectForKey(&NSString::from_str(k))?
        .downcast_ref::<NSNumber>()
        .map(|n| n.doubleValue())
}

/// Window list of the frontmost app (no titles are read: no permission needed).
pub fn frontmost_covers(display_id: u32, own_pid: i32) -> bool {
    let front = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|a| a.processIdentifier());
    if front.is_none() || front == Some(own_pid) {
        return false;
    }
    let Some(list) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    ) else {
        return false;
    };
    let cf: &CFArray = &list;
    // SAFETY: CFArray is toll-free bridged with NSArray; the window list is an
    // array of CFDictionary (bridged NSDictionary<NSString, id>), kept alive
    // by `list` for the duration of this function.
    let arr: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*(cf as *const CFArray).cast() };
    let mut windows = vec![];
    for d in arr.iter() {
        let Some(pid) = num(&d, "kCGWindowOwnerPID") else {
            continue;
        };
        if Some(pid as i32) != front {
            continue;
        }
        let Some(b) = d
            .objectForKey(&NSString::from_str("kCGWindowBounds"))
            .and_then(|b| b.downcast::<NSDictionary>().ok())
        else {
            continue;
        };
        // SAFETY: the bounds dictionary has NSString keys and NSNumber values.
        let b: &NSDictionary<NSString, AnyObject> =
            unsafe { &*(&*b as *const NSDictionary).cast() };
        windows.push(WindowInfo {
            owner_pid: pid as i32,
            layer: num(&d, "kCGWindowLayer").unwrap_or(0.0) as i64,
            alpha: num(&d, "kCGWindowAlpha").unwrap_or(1.0),
            bounds: Rect::new(
                num(b, "X").unwrap_or(0.0),
                num(b, "Y").unwrap_or(0.0),
                num(b, "Width").unwrap_or(0.0),
                num(b, "Height").unwrap_or(0.0),
            ),
        });
    }
    let r = objc2_core_graphics::CGDisplayBounds(display_id);
    let display = Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height);
    covers(&windows, front, own_pid, display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_rules() {
        let d = Rect::new(0.0, 0.0, 1470.0, 956.0);
        let full = WindowInfo {
            owner_pid: 7,
            layer: 0,
            alpha: 1.0,
            bounds: d,
        };
        assert!(covers(&[full], Some(7), 1, d));
        assert!(!covers(&[full], Some(1), 1, d), "own app never counts");
        assert!(!covers(&[full], Some(8), 1, d), "other app's window");
        let windowed = WindowInfo {
            bounds: Rect::new(0.0, 37.0, 1470.0, 919.0),
            ..full
        };
        assert!(
            !covers(&[windowed], Some(7), 1, d),
            "zoomed window keeps island"
        );
        let hidden = WindowInfo { alpha: 0.0, ..full };
        assert!(!covers(&[hidden], Some(7), 1, d));
    }
}
