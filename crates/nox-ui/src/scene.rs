//! Display list produced by the (pure) layout code and executed by the
//! AppKit painter. Hit regions map clicks to [`Action`]s.

use crate::geom::{Color, Point, Rect};
use nox_core::Command;
use nox_core::Section;
use nox_core::model::watch::WatchTrigger;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size: f64,
    pub weight: Weight,
    pub color: Color,
    pub align: Align,
    /// Monospaced digits (timers, percentages).
    pub mono: bool,
    /// Maximum lines; the last one is truncated with an ellipsis.
    pub lines: u8,
}

impl TextStyle {
    pub fn new(size: f64) -> Self {
        Self {
            size,
            weight: Weight::Regular,
            color: Color::TEXT,
            align: Align::Left,
            mono: false,
            lines: 1,
        }
    }
    pub fn weight(mut self, w: Weight) -> Self {
        self.weight = w;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    pub fn lines(mut self, n: u8) -> Self {
        self.lines = n.max(1);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Fill {
        rect: Rect,
        radius: f64,
        color: Color,
    },
    Stroke {
        rect: Rect,
        radius: f64,
        color: Color,
        width: f64,
    },
    Ellipse {
        rect: Rect,
        color: Color,
    },
    Text {
        rect: Rect,
        text: String,
        style: TextStyle,
    },
    /// SF Symbol drawn centred in `rect` at `size` points.
    Symbol {
        rect: Rect,
        name: String,
        size: f64,
        color: Color,
        weight: Weight,
    },
    /// Decoded album artwork, cached by id.
    Artwork {
        rect: Rect,
        id: u64,
        radius: f64,
    },
    /// Connected polyline (a series with gaps is split into several).
    Line {
        points: Vec<Point>,
        color: Color,
        width: f64,
    },
    /// Closed polygon filled (area under a sparkline).
    Area {
        points: Vec<Point>,
        color: Color,
    },
    /// Progress ring: `fraction` of the circle from 12 o'clock.
    Ring {
        center: Point,
        radius: f64,
        width: f64,
        fraction: f64,
        color: Color,
        track: Color,
    },
    PushClip {
        rect: Rect,
        radius: f64,
    },
    PopClip,
    /// Global opacity for the following operations (cross-fades).
    Alpha(f64),
    /// Where the camera preview layer is placed.
    CameraHost {
        rect: Rect,
        radius: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Send(Command),
    /// Several commands in order.
    SendAll(Vec<Command>),
    SelectSection(Section),
    /// Click on a timeline: position = (click.x - x) / w * duration.
    Seek {
        x: f64,
        w: f64,
        duration: f64,
    },
    MusicPanel(MusicPanel),
    OpenSettings,
    OpenMonitor,
    /// Explain what the closed-lid helper does, then ask for consent.
    InstallHelper,
    BeginWatchSelection(WatchTrigger),
    OpenUrl(String),
    /// Choose files with an open panel and add them to the shelf.
    PickFiles,
    Collapse,
    /// Open the island on a section (from the compact strip or an alert).
    Open(Option<Section>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MusicPanel {
    #[default]
    Lyrics,
    Queue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub rect: Rect,
    pub action: Action,
    /// Tooltip / accessibility label.
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub ops: Vec<Op>,
    pub hits: Vec<Hit>,
    /// Redraw again after this many seconds (progress, countdowns), if any.
    pub refresh: Option<f64>,
    /// The page shows live metrics (foreground sampling wanted).
    pub shows_metrics: bool,
    /// The camera page is on screen.
    pub shows_camera: bool,
    /// Vertical extent of scrollable content beyond its viewport.
    pub scroll_extent: f64,
}

impl Scene {
    pub fn fill(&mut self, rect: Rect, radius: f64, color: Color) {
        self.ops.push(Op::Fill {
            rect,
            radius,
            color,
        });
    }
    pub fn stroke(&mut self, rect: Rect, radius: f64, color: Color, width: f64) {
        self.ops.push(Op::Stroke {
            rect,
            radius,
            color,
            width,
        });
    }
    pub fn ellipse(&mut self, rect: Rect, color: Color) {
        self.ops.push(Op::Ellipse { rect, color });
    }
    pub fn text(&mut self, rect: Rect, text: impl Into<String>, style: TextStyle) {
        let text = text.into();
        if !text.is_empty() {
            self.ops.push(Op::Text { rect, text, style });
        }
    }
    pub fn symbol(&mut self, rect: Rect, name: &str, size: f64, color: Color) {
        self.ops.push(Op::Symbol {
            rect,
            name: name.into(),
            size,
            color,
            weight: Weight::Medium,
        });
    }
    pub fn hit(&mut self, rect: Rect, action: Action, label: impl Into<String>) {
        self.hits.push(Hit {
            rect,
            action,
            label: label.into(),
        });
    }
    pub fn refresh_in(&mut self, secs: f64) {
        if secs.is_finite() && secs > 0.0 {
            self.refresh = Some(self.refresh.map_or(secs, |r| r.min(secs)));
        }
    }
    /// The last hit region containing `p` (later = drawn on top).
    pub fn hit_at(&self, p: Point) -> Option<&Hit> {
        self.hits.iter().rev().find(|h| h.rect.contains(p))
    }
    /// All drawn text, for tests and accessibility.
    pub fn texts(&self) -> Vec<&str> {
        self.ops
            .iter()
            .filter_map(|o| match o {
                Op::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
    pub fn has_text(&self, needle: &str) -> bool {
        self.texts().iter().any(|t| t.contains(needle))
    }
    pub fn actions(&self) -> Vec<&Action> {
        self.hits.iter().map(|h| &h.action).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_prefers_topmost_and_refresh_keeps_minimum() {
        let mut s = Scene::default();
        s.hit(Rect::new(0.0, 0.0, 100.0, 100.0), Action::Collapse, "a");
        s.hit(Rect::new(10.0, 10.0, 10.0, 10.0), Action::OpenSettings, "b");
        assert_eq!(
            s.hit_at(Point::new(15.0, 15.0)).map(|h| &h.action),
            Some(&Action::OpenSettings)
        );
        assert_eq!(
            s.hit_at(Point::new(50.0, 50.0)).map(|h| &h.action),
            Some(&Action::Collapse)
        );
        assert!(s.hit_at(Point::new(150.0, 50.0)).is_none());
        s.refresh_in(1.0);
        s.refresh_in(0.5);
        s.refresh_in(f64::NAN);
        assert_eq!(s.refresh, Some(0.5));
        s.text(Rect::default(), "", TextStyle::new(10.0));
        assert!(s.ops.is_empty(), "empty strings are skipped");
    }
}
