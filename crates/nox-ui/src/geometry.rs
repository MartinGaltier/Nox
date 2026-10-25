// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchSupport.swift
// (NotchLayout, NotchGeometry) and Sources/Vorssaint/UI/Notch/NotchView.swift (NotchShape), commit e916961.
//! Notch geometry, island sizes, silhouette path and hit-testing. Pure Rust:
//! AppKit only supplies the raw screen parameters.

use crate::geom::{Point, Rect, Size};
use nox_core::settings::{IslandSettings, IslandSilhouette, IslandSize};

/// Raw parameters read from an `NSScreen` (AppKit global space, y up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenParams {
    pub frame: Rect,
    pub visible_frame: Rect,
    /// `safeAreaInsets.top` (0 without a camera housing).
    pub safe_area_top: f64,
    /// `auxiliaryTopLeftArea` / `auxiliaryTopRightArea` (screen-local).
    pub aux_left: Option<Rect>,
    pub aux_right: Option<Rect>,
    /// `NSStatusBar.systemStatusBar.thickness`.
    pub status_bar_thickness: f64,
}

/// Where and how big the camera region is on one display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotchGeometry {
    pub screen: Rect,
    /// A physical camera housing exists.
    pub notched: bool,
    /// Global x of the camera centre.
    pub center_x: f64,
    pub camera_width: f64,
    pub camera_height: f64,
    pub bar_height: f64,
    /// Capsule mode: distance kept from the top/bottom of the menu bar.
    pub floating_gap: Option<f64>,
    /// Something is drawn at rest (false: no notch and simulation is off).
    pub rest_visible: bool,
}

/// Shoulder (top flare) of a surface of height `h`.
pub fn shoulder(h: f64) -> f64 {
    (h * 0.19).clamp(0.0, 14.0)
}

/// Bottom corner radius of a surface of height `h`.
pub fn surface_radius(h: f64) -> f64 {
    (h * 0.34).clamp(0.0, 28.0)
}

pub const HORIZONTAL_INSET: f64 = 22.0;
pub const TABS_HEIGHT: f64 = 26.0;
pub const BOTTOM_INSET: f64 = 14.0;

fn finite_or(v: f64, d: f64) -> f64 {
    if v.is_finite() { v } else { d }
}

impl NotchGeometry {
    pub fn new(p: &ScreenParams, simulate: bool, silhouette: IslandSilhouette) -> Self {
        let screen = p.frame;
        let top_gap = screen.max_y() - p.visible_frame.max_y();
        let bar = if top_gap > 1.0 {
            top_gap
        } else {
            finite_or(p.status_bar_thickness, 24.0)
        }
        .clamp(16.0, 64.0);
        let camera = match (p.aux_left, p.aux_right) {
            (Some(l), Some(r)) => (r.x - l.max_x()).max(0.0),
            _ => 0.0,
        };
        let notched = p.safe_area_top.is_finite() && p.safe_area_top > 0.0 && camera > 0.0;
        if notched {
            // The housing is centred on built-in panels; derive the centre from the
            // auxiliary areas when they are given in screen-local coordinates.
            let center_x = match (p.aux_left, p.aux_right) {
                (Some(l), Some(r)) if l.x.abs() < 1.0 => screen.x + (l.max_x() + r.x) / 2.0,
                _ => screen.mid_x(),
            };
            return Self {
                screen,
                notched,
                center_x,
                camera_width: camera.min(screen.w * 0.7),
                camera_height: p.safe_area_top.min(64.0),
                bar_height: bar.max(p.safe_area_top),
                floating_gap: None,
                rest_visible: true,
            };
        }
        let floating_gap = (simulate && silhouette == IslandSilhouette::Capsule)
            .then(|| ((bar - 20.0) / 2.0).floor().clamp(0.0, 2.0));
        Self {
            screen,
            notched: false,
            center_x: screen.mid_x(),
            camera_width: (180.0 * bar / 32.0).round(),
            camera_height: bar,
            bar_height: bar,
            floating_gap,
            rest_visible: simulate,
        }
    }

    pub fn floats(&self) -> bool {
        self.floating_gap.is_some()
    }

    /// Closed island: the camera region, plus a wing on each side for an activity.
    pub fn compact_size(&self, wing: f64) -> Size {
        let w = (self.camera_width + 2.0 * wing.max(0.0)).min(self.screen.w - 24.0);
        Size::new(w, self.camera_height)
    }

    pub fn expanded_width(&self, s: &IslandSettings) -> f64 {
        let preferred = match s.size {
            IslandSize::Compact => 440.0,
            IslandSize::Spacious => 520.0,
            IslandSize::Custom => finite_or(s.width, 520.0).clamp(360.0, 640.0),
        };
        preferred
            .max(self.camera_width + 2.0 * 120.0)
            .min(self.screen.w - 24.0)
    }

    pub fn content_height(s: &IslandSettings) -> f64 {
        match s.size {
            IslandSize::Compact => 150.0,
            IslandSize::Spacious | IslandSize::Custom => 190.0,
        }
    }

    pub fn expanded_size(&self, s: &IslandSettings) -> Size {
        let top = self.camera_height.max(28.0) + if self.floats() { 4.0 } else { 0.0 };
        Size::new(
            self.expanded_width(s),
            top + TABS_HEIGHT + Self::content_height(s) + BOTTOM_INSET,
        )
    }

    /// Fixed panel frame (AppKit global space) holding the largest island
    /// `max` for the whole session: hangs from the top edge and is centred
    /// on the camera with whole points, so it never moves while animating.
    pub fn window_frame(&self, max: Size) -> Rect {
        let x = (self.center_x - max.w / 2.0).round();
        let x = x.clamp(
            self.screen.x,
            (self.screen.max_x() - max.w).max(self.screen.x),
        );
        Rect::new(x, self.screen.max_y() - max.h, max.w, max.h)
    }

    /// Left edge of a shape `width` wide, centred on the camera and snapped to
    /// half points (a whole pixel on 2x displays), in AppKit global space.
    pub fn shape_x(&self, width: f64) -> f64 {
        ((self.center_x - width / 2.0) * 2.0).round() / 2.0
    }

    /// Rect of the camera housing in a view of width `view_w` (flipped).
    pub fn camera_rect_in(&self, view_w: f64) -> Rect {
        Rect::new(
            (view_w - self.camera_width) / 2.0,
            0.0,
            self.camera_width,
            self.camera_height,
        )
    }
}

/// Path element of the island outline (flipped view space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    Move(Point),
    Line(Point),
    Curve(Point, Point, Point),
    Close,
}

const TANGENT: f64 = 0.552_284_75;

fn rounded_rect(path: &mut Vec<Seg>, r: Rect, radius: f64) {
    let c = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let k = c * (1.0 - TANGENT);
    let (l, t, rt, b) = (r.x, r.y, r.max_x(), r.max_y());
    path.push(Seg::Move(Point::new(l + c, t)));
    path.push(Seg::Line(Point::new(rt - c, t)));
    path.push(Seg::Curve(
        Point::new(rt - k, t),
        Point::new(rt, t + k),
        Point::new(rt, t + c),
    ));
    path.push(Seg::Line(Point::new(rt, b - c)));
    path.push(Seg::Curve(
        Point::new(rt, b - k),
        Point::new(rt - k, b),
        Point::new(rt - c, b),
    ));
    path.push(Seg::Line(Point::new(l + c, b)));
    path.push(Seg::Curve(
        Point::new(l + k, b),
        Point::new(l, b - k),
        Point::new(l, b - c),
    ));
    path.push(Seg::Line(Point::new(l, t + c)));
    path.push(Seg::Curve(
        Point::new(l, t + k),
        Point::new(l + k, t),
        Point::new(l + c, t),
    ));
    path.push(Seg::Close);
}

/// The silhouette. `floating_gap` = capsule; `compact` rounds a capsule fully.
pub fn island_path(size: Size, floating_gap: Option<f64>, compact: bool) -> Vec<Seg> {
    let (w, h) = (size.w.max(0.0), size.h.max(0.0));
    let mut p = Vec::with_capacity(12);
    if let Some(gap) = floating_gap {
        let side = shoulder(h).min(w / 2.0);
        let body = Rect::new(side, gap, w - 2.0 * side, h - 2.0 * gap);
        let radius = if compact {
            body.h / 2.0
        } else {
            surface_radius(h)
        };
        rounded_rect(&mut p, body, radius);
        return p;
    }
    let s = shoulder(h).min(w / 2.0);
    let b = surface_radius(h)
        .min(h / 2.0)
        .min((w - s * 2.0) / 2.0)
        .max(0.0);
    p.push(Seg::Move(Point::new(0.0, 0.0)));
    p.push(Seg::Line(Point::new(w, 0.0)));
    p.push(Seg::Curve(
        Point::new(w - s * TANGENT, 0.0),
        Point::new(w - s, s * (1.0 - TANGENT)),
        Point::new(w - s, s),
    ));
    p.push(Seg::Line(Point::new(w - s, h - b)));
    p.push(Seg::Curve(
        Point::new(w - s, h - b * (1.0 - TANGENT)),
        Point::new(w - s - b * (1.0 - TANGENT), h),
        Point::new(w - s - b, h),
    ));
    p.push(Seg::Line(Point::new(s + b, h)));
    p.push(Seg::Curve(
        Point::new(s + b * (1.0 - TANGENT), h),
        Point::new(s, h - b * (1.0 - TANGENT)),
        Point::new(s, h - b),
    ));
    p.push(Seg::Line(Point::new(s, s)));
    p.push(Seg::Curve(
        Point::new(s, s * (1.0 - TANGENT)),
        Point::new(s * TANGENT, 0.0),
        Point::new(0.0, 0.0),
    ));
    p.push(Seg::Close);
    p
}

fn in_rounded_rect(r: Rect, radius: f64, p: Point) -> bool {
    if !r.contains(p) {
        return false;
    }
    let c = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let cx = p.x.clamp(r.x + c, r.max_x() - c);
    let cy = p.y.clamp(r.y + c, r.max_y() - c);
    let (dx, dy) = (p.x - cx, p.y - cy);
    dx * dx + dy * dy <= c * c + 1e-9
}

/// Whether `p` (flipped view space) lies inside the silhouette (circular
/// approximation of the Bézier corners, within a fraction of a point).
pub fn island_contains(size: Size, floating_gap: Option<f64>, compact: bool, p: Point) -> bool {
    let (w, h) = (size.w, size.h);
    if !(p.x >= 0.0 && p.x <= w && p.y >= 0.0 && p.y <= h) {
        return false;
    }
    if let Some(gap) = floating_gap {
        let side = shoulder(h).min(w / 2.0);
        let body = Rect::new(side, gap, w - 2.0 * side, h - 2.0 * gap);
        let radius = if compact {
            body.h / 2.0
        } else {
            surface_radius(h)
        };
        return in_rounded_rect(body, radius, p);
    }
    let s = shoulder(h).min(w / 2.0);
    // Shoulders: outside the quarter circle centred at (0, s) / (w, s).
    if p.y < s {
        if p.x < s {
            let (dx, dy) = (p.x, p.y - s);
            return dx * dx + dy * dy >= s * s - 1e-9;
        }
        if p.x > w - s {
            let (dx, dy) = (w - p.x, p.y - s);
            return dx * dx + dy * dy >= s * s - 1e-9;
        }
        return true;
    }
    let body = Rect::new(s, -h, w - 2.0 * s, 2.0 * h);
    let b = surface_radius(h).min(h / 2.0).min((w - s * 2.0) / 2.0);
    in_rounded_rect(body, b, p)
}

/// The rectangle the island's content uses when expanded (flipped space).
pub fn expanded_content_rect(size: Size, g: &NotchGeometry) -> Rect {
    let s = shoulder(size.h);
    let top = g.camera_height.max(28.0) + if g.floats() { 4.0 } else { 0.0 };
    Rect::new(
        s + HORIZONTAL_INSET - 6.0,
        top,
        (size.w - 2.0 * (s + HORIZONTAL_INSET - 6.0)).max(0.0),
        (size.h - top - BOTTOM_INSET + 4.0).max(0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MacBook Air M2, default scaling (1470×956 pt): camera housing 185×32.
    fn m2_air() -> ScreenParams {
        ScreenParams {
            frame: Rect::new(0.0, 0.0, 1470.0, 956.0),
            visible_frame: Rect::new(0.0, 0.0, 1470.0, 919.0),
            safe_area_top: 32.0,
            aux_left: Some(Rect::new(0.0, 924.0, 642.5, 32.0)),
            aux_right: Some(Rect::new(827.5, 924.0, 642.5, 32.0)),
            status_bar_thickness: 24.0,
        }
    }

    fn external() -> ScreenParams {
        ScreenParams {
            frame: Rect::new(1470.0, 0.0, 2560.0, 1440.0),
            visible_frame: Rect::new(1470.0, 0.0, 2560.0, 1415.0),
            safe_area_top: 0.0,
            aux_left: None,
            aux_right: None,
            status_bar_thickness: 24.0,
        }
    }

    #[test]
    fn notched_display_centres_on_camera() {
        let g = NotchGeometry::new(&m2_air(), true, IslandSilhouette::Capsule);
        assert!(g.notched);
        assert_eq!(g.camera_width, 185.0);
        assert_eq!(g.camera_height, 32.0);
        assert_eq!(g.center_x, 735.0);
        assert_eq!(g.floating_gap, None, "a real notch never floats");
        let f = g.window_frame(Size::new(185.0, 32.0));
        assert_eq!(f, Rect::new(643.0, 924.0, 185.0, 32.0));
        // Hangs flush with the top edge.
        assert_eq!(f.max_y(), 956.0);
        // Centred on 735: 642.5 is already a half point.
        assert_eq!(g.shape_x(185.0), 642.5);
        assert_eq!(g.shape_x(186.0), 642.0);
    }

    #[test]
    fn external_display_simulates_or_hides() {
        let g = NotchGeometry::new(&external(), true, IslandSilhouette::Capsule);
        assert!(!g.notched && g.rest_visible);
        assert_eq!(g.bar_height, 25.0);
        assert_eq!(g.floating_gap, Some(2.0));
        assert_eq!(g.center_x, 1470.0 + 1280.0);
        let hanging = NotchGeometry::new(&external(), true, IslandSilhouette::Notch);
        assert_eq!(hanging.floating_gap, None);
        let off = NotchGeometry::new(&external(), false, IslandSilhouette::Capsule);
        assert!(!off.rest_visible);
    }

    #[test]
    fn expanded_size_respects_presets_and_screen() {
        let g = NotchGeometry::new(&m2_air(), true, IslandSilhouette::Capsule);
        let mut s = IslandSettings::default();
        s.size = IslandSize::Compact;
        assert_eq!(g.expanded_width(&s), 440.0);
        s.size = IslandSize::Custom;
        s.width = 10_000.0;
        assert_eq!(g.expanded_width(&s), 640.0);
        s.width = f64::NAN;
        assert_eq!(g.expanded_width(&s), 520.0);
        let size = g.expanded_size(&s);
        assert!(size.h > g.camera_height + 100.0);
    }

    #[test]
    fn hit_test_follows_silhouette() {
        let size = Size::new(400.0, 200.0);
        let s = shoulder(200.0);
        assert_eq!(s, 14.0);
        // Centre and top edge are inside.
        assert!(island_contains(size, None, false, Point::new(200.0, 100.0)));
        assert!(island_contains(size, None, false, Point::new(10.0, 0.5)));
        // The shoulder's hollow lets clicks through.
        assert!(!island_contains(size, None, false, Point::new(2.0, 12.0)));
        assert!(!island_contains(size, None, false, Point::new(398.0, 12.0)));
        // Side columns below the shoulders are outside.
        assert!(!island_contains(size, None, false, Point::new(5.0, 100.0)));
        // Bottom corner rounding.
        assert!(!island_contains(size, None, false, Point::new(15.0, 199.0)));
        assert!(island_contains(size, None, false, Point::new(60.0, 199.0)));
        // Outside the bounds.
        assert!(!island_contains(size, None, false, Point::new(-1.0, 10.0)));
    }

    #[test]
    fn capsule_hit_test_keeps_gap() {
        let size = Size::new(200.0, 25.0);
        assert!(!island_contains(
            size,
            Some(2.0),
            true,
            Point::new(100.0, 1.0)
        ));
        assert!(island_contains(
            size,
            Some(2.0),
            true,
            Point::new(100.0, 12.0)
        ));
        assert!(!island_contains(
            size,
            Some(2.0),
            true,
            Point::new(6.0, 3.0)
        ));
    }

    #[test]
    fn path_is_closed_and_spans_width() {
        let p = island_path(Size::new(300.0, 120.0), None, false);
        assert!(matches!(p.first(), Some(Seg::Move(_))));
        assert_eq!(p.last(), Some(&Seg::Close));
        assert!(p.contains(&Seg::Line(Point::new(300.0, 0.0))));
        let c = island_path(Size::new(300.0, 24.0), Some(2.0), true);
        assert_eq!(c.last(), Some(&Seg::Close));
    }
}
