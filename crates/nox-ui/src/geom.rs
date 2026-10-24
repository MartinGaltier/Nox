//! Plain geometry and colour types shared by layout and drawing. No AppKit.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

impl Size {
    pub const fn new(w: f64, h: f64) -> Self {
        Self { w, h }
    }
}

/// A rectangle. In view space (flipped) `y` grows downwards; in AppKit
/// global space it grows upwards. Functions say which one they expect.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.w
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.h
    }
    pub fn mid_x(&self) -> f64 {
        self.x + self.w / 2.0
    }
    pub fn mid_y(&self) -> f64 {
        self.y + self.h / 2.0
    }
    pub fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.max_x() && p.y >= self.y && p.y < self.max_y()
    }
    pub fn inset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(
            self.x + dx,
            self.y + dy,
            (self.w - 2.0 * dx).max(0.0),
            (self.h - 2.0 * dy).max(0.0),
        )
    }
    pub fn offset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    /// Splits off a column of `w` from the left; returns (left, rest).
    pub fn split_left(&self, w: f64, gap: f64) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w);
        (
            Rect::new(self.x, self.y, w, self.h),
            Rect::new(
                self.x + w + gap,
                self.y,
                (self.w - w - gap).max(0.0),
                self.h,
            ),
        )
    }
    /// Splits off a column of `w` from the right; returns (rest, right).
    pub fn split_right(&self, w: f64, gap: f64) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w);
        (
            Rect::new(self.x, self.y, (self.w - w - gap).max(0.0), self.h),
            Rect::new(self.max_x() - w, self.y, w, self.h),
        )
    }
    /// Splits off a row of `h` from the top (flipped space); returns (top, rest).
    pub fn split_top(&self, h: f64, gap: f64) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.h);
        (
            Rect::new(self.x, self.y, self.w, h),
            Rect::new(
                self.x,
                self.y + h + gap,
                self.w,
                (self.h - h - gap).max(0.0),
            ),
        )
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.max_x() && o.x < self.max_x() && self.y < o.max_y() && o.y < self.max_y()
    }
    /// A centred sub-rectangle of the given size.
    pub fn centered(&self, w: f64, h: f64) -> Rect {
        Rect::new(self.mid_x() - w / 2.0, self.mid_y() - h / 2.0, w, h)
    }
}

/// sRGB colour, components 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub const fn rgba(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }
    pub const fn gray(v: f64, a: f64) -> Self {
        Self::rgba(v, v, v, a)
    }
    pub fn with_alpha(self, a: f64) -> Self {
        Self { a, ..self }
    }
    pub fn scaled_alpha(self, k: f64) -> Self {
        Self {
            a: self.a * k,
            ..self
        }
    }
    pub const BLACK: Color = Color::gray(0.0, 1.0);
    pub const WHITE: Color = Color::gray(1.0, 1.0);
    pub const TEXT: Color = Color::gray(1.0, 0.95);
    pub const SECONDARY: Color = Color::gray(1.0, 0.6);
    pub const TERTIARY: Color = Color::gray(1.0, 0.38);
    pub const CARD: Color = Color::gray(1.0, 0.08);
    pub const CARD_HOVER: Color = Color::gray(1.0, 0.14);
    pub const TRACK: Color = Color::gray(1.0, 0.16);
    pub const ACCENT: Color = Color::rgba(0.39, 0.62, 1.0, 1.0);
    pub const GREEN: Color = Color::rgba(0.30, 0.85, 0.45, 1.0);
    pub const ORANGE: Color = Color::rgba(1.0, 0.62, 0.20, 1.0);
    pub const RED: Color = Color::rgba(1.0, 0.33, 0.30, 1.0);
    pub const PURPLE: Color = Color::rgba(0.72, 0.52, 1.0, 1.0);
    pub const YELLOW: Color = Color::rgba(1.0, 0.84, 0.25, 1.0);
}
