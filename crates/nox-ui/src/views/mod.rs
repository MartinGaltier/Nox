//! Layout of every island surface into a [`Scene`]. Pure: reads
//! `IslandState`/`Settings`, never AppKit.

pub mod agents;
pub mod compact;
pub mod extra;
pub mod lists;
pub mod monitor;
pub mod music;
pub mod schedule;
pub mod system;
pub mod tools;

use crate::geom::{Color, Point, Rect};
use crate::geometry::{NotchGeometry, TABS_HEIGHT, shoulder};
use crate::scene::{Action, Align, MusicPanel, Scene, TextStyle, Weight};
use nox_core::model::metrics::{MetricsHistory, Series};
use nox_core::permissions::{Permission, PermissionState};
use nox_core::{Command, IslandState, Section, Settings};

/// Everything a view reads.
pub struct Ctx<'a> {
    pub state: &'a IslandState,
    pub settings: &'a Settings,
    /// Wall clock (Unix seconds).
    pub now: f64,
    /// Pointer in view space, for hover highlights.
    pub hover: Option<Point>,
    /// Vertical scroll offset of the current page.
    pub scroll: f64,
    pub music_panel: MusicPanel,
    /// The camera preview layer is attached (integrations provided it).
    pub camera_attached: bool,
}

impl Ctx<'_> {
    pub fn hovered(&self, r: Rect) -> bool {
        self.hover.is_some_and(|p| r.contains(p))
    }
}

// ---------- shared widgets ----------

pub fn label(size: f64) -> TextStyle {
    TextStyle::new(size)
}

pub fn secondary(size: f64) -> TextStyle {
    TextStyle::new(size).color(Color::SECONDARY)
}

pub fn tertiary(size: f64) -> TextStyle {
    TextStyle::new(size).color(Color::TERTIARY)
}

/// A round icon button. Registers its hit region.
pub fn icon_button(
    s: &mut Scene,
    cx: &Ctx,
    r: Rect,
    symbol: &str,
    size: f64,
    action: Action,
    tip: &str,
) {
    let bg = if cx.hovered(r) {
        Color::CARD_HOVER
    } else {
        Color::gray(1.0, 0.0)
    };
    s.fill(r, r.h.min(r.w) / 2.0, bg);
    s.symbol(r, symbol, size, Color::TEXT);
    s.hit(r, action, tip);
}

/// A pill button with a text label (`selected` highlights it).
pub fn pill(s: &mut Scene, cx: &Ctx, r: Rect, text: &str, selected: bool, action: Action) {
    let bg = if selected {
        Color::ACCENT.with_alpha(0.85)
    } else if cx.hovered(r) {
        Color::CARD_HOVER
    } else {
        Color::CARD
    };
    s.fill(r, r.h / 2.0, bg);
    s.text(
        r.inset(6.0, (r.h - 13.0).max(0.0) / 2.0),
        text,
        label(11.0)
            .weight(Weight::Medium)
            .align(Align::Center)
            .color(if selected { Color::WHITE } else { Color::TEXT }),
    );
    s.hit(r, action, text);
}

/// A row of equal pills; returns the row's rect.
pub fn pill_row(s: &mut Scene, cx: &Ctx, r: Rect, items: &[(String, bool, Action)], gap: f64) {
    if items.is_empty() {
        return;
    }
    let n = items.len() as f64;
    let w = ((r.w - gap * (n - 1.0)) / n).max(0.0);
    for (i, (t, sel, a)) in items.iter().enumerate() {
        let x = r.x + i as f64 * (w + gap);
        pill(s, cx, Rect::new(x, r.y, w, r.h), t, *sel, a.clone());
    }
}

pub fn card(s: &mut Scene, r: Rect) {
    s.fill(r, 12.0, Color::CARD);
}

/// Horizontal progress bar; `None` draws an indeterminate striped track.
pub fn bar(s: &mut Scene, r: Rect, fraction: Option<f64>, color: Color) {
    s.fill(r, r.h / 2.0, Color::TRACK);
    match fraction {
        Some(f) if f.is_finite() => {
            let f = f.clamp(0.0, 1.0);
            if f > 0.0 {
                s.fill(
                    Rect::new(r.x, r.y, (r.w * f).max(r.h), r.h),
                    r.h / 2.0,
                    color,
                );
            }
        }
        _ => {
            // Indeterminate: three short segments, no animation (no timer).
            for i in 0..3 {
                let x = r.x + r.w * (0.1 + 0.3 * i as f64);
                s.fill(
                    Rect::new(x, r.y, r.w * 0.12, r.h),
                    r.h / 2.0,
                    color.with_alpha(0.5),
                );
            }
        }
    }
}

/// Honest empty state: symbol, title and an explanation.
pub fn empty(s: &mut Scene, r: Rect, symbol: &str, title: &str, detail: &str) {
    let c = r.centered(r.w.min(360.0), 74.0);
    s.symbol(
        Rect::new(c.mid_x() - 14.0, c.y, 28.0, 26.0),
        symbol,
        20.0,
        Color::TERTIARY,
    );
    s.text(
        Rect::new(c.x, c.y + 30.0, c.w, 17.0),
        title,
        label(13.0).weight(Weight::Semibold).align(Align::Center),
    );
    s.text(
        Rect::new(c.x, c.y + 49.0, c.w, 28.0),
        detail,
        secondary(11.0).align(Align::Center).lines(2),
    );
}

/// Module switched off or not publishing yet.
pub fn module_missing(s: &mut Scene, r: Rect, section: Section, enabled: bool) {
    if enabled {
        empty(
            s,
            r,
            section.symbol(),
            section.label_fr(),
            "En attente des premières données du module.",
        );
    } else {
        empty(
            s,
            r,
            section.symbol(),
            section.label_fr(),
            "Module désactivé dans les réglages.",
        );
    }
}

/// Permission gate. Returns true when the content may be shown.
pub fn permission_gate(
    s: &mut Scene,
    cx: &Ctx,
    r: Rect,
    perm: Permission,
    state: PermissionState,
    why: &str,
    request: Option<Command>,
) -> bool {
    match state {
        PermissionState::Granted | PermissionState::NotRequired => true,
        PermissionState::NotDetermined if request.is_some() => {
            empty(
                s,
                r.split_top(r.h - 34.0, 0.0).0,
                "lock",
                perm.label_fr(),
                why,
            );
            let b = Rect::new(r.mid_x() - 70.0, r.max_y() - 30.0, 140.0, 24.0);
            pill(
                s,
                cx,
                b,
                "Autoriser…",
                true,
                Action::Send(request.expect("checked")),
            );
            false
        }
        _ => {
            let detail = format!("{why} Autorisez Nox dans Réglages Système › Confidentialité.");
            empty(
                s,
                r.split_top(r.h - 34.0, 0.0).0,
                "lock.slash",
                &format!("Autorisation requise : {}", perm.label_fr()),
                &detail,
            );
            let b = Rect::new(r.mid_x() - 90.0, r.max_y() - 30.0, 180.0, 24.0);
            pill(
                s,
                cx,
                b,
                "Ouvrir les Réglages Système",
                false,
                Action::OpenUrl(perm.settings_url().into()),
            );
            false
        }
    }
}

/// Sparkline of a bounded history; gaps (`None`) break the line.
pub fn sparkline(
    s: &mut Scene,
    r: Rect,
    history: &MetricsHistory,
    series: Series,
    max: Option<f64>,
    color: Color,
) {
    let Some(q) = history.get(series) else {
        return;
    };
    let cap = history.capacity.max(q.len()).max(2);
    let hi = max.unwrap_or_else(|| q.iter().flatten().copied().fold(0.0f64, f64::max).max(1e-9));
    let step = r.w / (cap - 1) as f64;
    let offset = cap - q.len();
    let mut run: Vec<Point> = vec![];
    let flush = |s: &mut Scene, run: &mut Vec<Point>| {
        if run.len() >= 2 {
            let mut area = run.clone();
            area.push(Point::new(run.last().expect("len>=2").x, r.max_y()));
            area.push(Point::new(run[0].x, r.max_y()));
            s.ops.push(crate::scene::Op::Area {
                points: area,
                color: color.with_alpha(0.18),
            });
            s.ops.push(crate::scene::Op::Line {
                points: std::mem::take(run),
                color,
                width: 1.4,
            });
        } else {
            run.clear();
        }
    };
    for (i, v) in q.iter().enumerate() {
        match v {
            Some(v) if v.is_finite() => {
                let x = r.x + (offset + i) as f64 * step;
                let y = r.max_y() - (v / hi).clamp(0.0, 1.0) * r.h;
                run.push(Point::new(x, y));
            }
            _ => flush(s, &mut run),
        }
    }
    flush(s, &mut run);
}

// ---------- island chrome ----------

/// Header (section title left of the camera, buttons right of it) and the
/// row of section tabs. Returns the content rectangle.
pub fn chrome(
    s: &mut Scene,
    cx: &Ctx,
    g: &NotchGeometry,
    w: f64,
    h: f64,
    sections: &[Section],
    current: Section,
) -> Rect {
    let sh = shoulder(h);
    let side = sh + 16.0;
    let top = g.camera_height.max(28.0) + if g.floats() { 4.0 } else { 0.0 };
    let cam = if g.notched {
        g.camera_rect_in(w)
    } else {
        Rect::new(w / 2.0, 0.0, 0.0, 0.0)
    };
    let row_y = if g.floats() { 6.0 } else { (top - 22.0) / 2.0 };
    // Left: current section title.
    let left = Rect::new(side, row_y, (cam.x - 8.0 - side).max(0.0), 22.0);
    if g.notched || left.w > 60.0 {
        s.symbol(
            Rect::new(left.x, left.y, 20.0, 22.0),
            current.symbol(),
            13.0,
            Color::SECONDARY,
        );
        s.text(
            Rect::new(left.x + 24.0, left.y + 3.0, (left.w - 24.0).max(0.0), 17.0),
            current.label_fr(),
            label(13.0).weight(Weight::Semibold),
        );
    }
    // Right: monitor + settings.
    let right_end = w - side;
    let b = 24.0;
    icon_button(
        s,
        cx,
        Rect::new(right_end - b, row_y - 1.0, b, b),
        "gearshape",
        12.0,
        Action::OpenSettings,
        "Réglages",
    );
    icon_button(
        s,
        cx,
        Rect::new(right_end - 2.0 * b - 4.0, row_y - 1.0, b, b),
        "chart.xyaxis.line",
        12.0,
        Action::OpenMonitor,
        "Moniteur système",
    );

    // Tabs.
    let tabs = Rect::new(side, top, w - 2.0 * side, TABS_HEIGHT);
    let n = sections.len().max(1) as f64;
    let tw = (tabs.w / n).min(32.0);
    let start = tabs.mid_x() - tw * n / 2.0;
    for (i, sec) in sections.iter().enumerate() {
        let r = Rect::new(start + i as f64 * tw, tabs.y + 1.0, tw, 22.0);
        let sel = *sec == current;
        if sel {
            s.fill(r.inset(2.0, 0.0), 8.0, Color::gray(1.0, 0.16));
        } else if cx.hovered(r) {
            s.fill(r.inset(2.0, 0.0), 8.0, Color::gray(1.0, 0.08));
        }
        s.symbol(
            r,
            sec.symbol(),
            12.0,
            if sel { Color::WHITE } else { Color::SECONDARY },
        );
        s.hit(r, Action::SelectSection(*sec), sec.label_fr());
    }
    Rect::new(
        side,
        top + TABS_HEIGHT + 4.0,
        w - 2.0 * side,
        (h - top - TABS_HEIGHT - 4.0 - 12.0).max(0.0),
    )
}

/// The page body for `section`.
pub fn page(s: &mut Scene, cx: &Ctx, r: Rect, section: Section) {
    let enabled = cx.settings.module_enabled(section.module());
    match section {
        Section::Music => music::page(s, cx, r, enabled),
        Section::Calendar => schedule::calendar(s, cx, r, enabled),
        Section::Timers => schedule::timers(s, cx, r, enabled),
        Section::Agents => agents::page(s, cx, r, enabled),
        Section::Notifications => lists::notifications(s, cx, r, enabled),
        Section::Downloads => lists::downloads(s, cx, r, enabled),
        Section::Files => lists::files(s, cx, r, enabled),
        Section::Accessories => lists::accessories(s, cx, r, enabled),
        Section::System => system::page(s, cx, r, enabled),
        Section::KeepAwake => system::keep_awake(s, cx, r, enabled),
        Section::Watch => tools::watch(s, cx, r, enabled),
        Section::Camera => tools::camera(s, cx, r, enabled),
        Section::Shortcuts => tools::shortcuts(s, cx, r, enabled),
        Section::Captures => extra::captures(s, cx, r, enabled),
        Section::Scratchpad => extra::scratchpad(s, cx, r, enabled),
        Section::Clipboard => extra::clipboard(s, cx, r, enabled),
        // Rendered by Agent Interface once nox-audio publishes MixerState.
        Section::Mixer if !enabled => module_missing(s, r, section, enabled),
        Section::Mixer => empty(
            s,
            r,
            section.symbol(),
            section.label_fr(),
            "En attente : affichage du mixeur pas encore disponible dans cette version.",
        ),
    }
}

/// Vertically scrolled list helper: clips `r`, offsets rows by the scroll and
/// records the overflow so the view can clamp scrolling.
pub struct ListLayout {
    pub viewport: Rect,
    pub y: f64,
    start: f64,
    first_hit: usize,
}

impl ListLayout {
    pub fn begin(s: &mut Scene, r: Rect, scroll: f64) -> Self {
        s.ops.push(crate::scene::Op::PushClip {
            rect: r,
            radius: 8.0,
        });
        Self {
            viewport: r,
            y: r.y - scroll.max(0.0),
            start: r.y - scroll.max(0.0),
            first_hit: s.hits.len(),
        }
    }
    /// Next row of height `h`; `None` if it is entirely outside the viewport
    /// (still advances, so the extent is right).
    pub fn row(&mut self, h: f64, gap: f64) -> Option<Rect> {
        let r = Rect::new(self.viewport.x, self.y, self.viewport.w, h);
        self.y += h + gap;
        r.intersects(&self.viewport).then_some(r)
    }
    pub fn end(self, s: &mut Scene) {
        s.ops.push(crate::scene::Op::PopClip);
        let content = self.y - self.start;
        s.scroll_extent = s.scroll_extent.max((content - self.viewport.h).max(0.0));
        // Rows scrolled out of the viewport must not be clickable; partly
        // visible ones are clipped to it.
        let vp = self.viewport;
        let mut kept = s.hits.split_off(self.first_hit);
        kept.retain_mut(|h| {
            let top = h.rect.y.max(vp.y);
            let bottom = h.rect.max_y().min(vp.max_y());
            h.rect.y = top;
            h.rect.h = bottom - top;
            h.rect.h > 0.0
        });
        s.hits.extend(kept);
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    pub fn ctx<'a>(state: &'a IslandState, settings: &'a Settings) -> Ctx<'a> {
        Ctx {
            state,
            settings,
            now: 1_800_000_000.0,
            hover: None,
            scroll: 0.0,
            music_panel: MusicPanel::Lyrics,
            camera_attached: false,
        }
    }

    pub fn page_rect() -> Rect {
        Rect::new(30.0, 62.0, 460.0, 180.0)
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    #[test]
    fn every_section_renders_honest_empty_state_without_data() {
        let state = IslandState::default();
        let settings = Settings::default();
        let cx = ctx(&state, &settings);
        for sec in Section::DEFAULT_ORDER {
            let mut s = Scene::default();
            page(&mut s, &cx, page_rect(), sec);
            assert!(!s.ops.is_empty(), "{sec:?} draws something");
            assert!(
                s.has_text("Module désactivé") || s.has_text("En attente"),
                "{sec:?}: {:?}",
                s.texts()
            );
        }
    }

    #[test]
    fn chrome_has_a_tab_per_section_and_settings() {
        let state = IslandState::default();
        let settings = Settings::default();
        let cx = ctx(&state, &settings);
        let g = crate::geometry::NotchGeometry::new(
            &crate::geometry::ScreenParams {
                frame: Rect::new(0.0, 0.0, 1470.0, 956.0),
                visible_frame: Rect::new(0.0, 0.0, 1470.0, 919.0),
                safe_area_top: 32.0,
                aux_left: Some(Rect::new(0.0, 924.0, 642.5, 32.0)),
                aux_right: Some(Rect::new(827.5, 924.0, 642.5, 32.0)),
                status_bar_thickness: 24.0,
            },
            true,
            nox_core::settings::IslandSilhouette::Capsule,
        );
        let secs = settings.visible_sections();
        let mut s = Scene::default();
        let content = chrome(&mut s, &cx, &g, 520.0, 260.0, &secs, Section::Music);
        let tabs = s
            .actions()
            .iter()
            .filter(|a| matches!(a, Action::SelectSection(_)))
            .count();
        assert_eq!(tabs, secs.len());
        assert!(s.actions().contains(&&Action::OpenSettings));
        assert!(content.y > g.camera_height);
        // Header text never overlaps the camera housing.
        let cam = g.camera_rect_in(520.0);
        for op in &s.ops {
            if let crate::scene::Op::Text { rect, .. } = op {
                assert!(!rect.intersects(&cam), "text under the camera: {rect:?}");
            }
        }
    }

    #[test]
    fn sparkline_breaks_on_gaps() {
        let mut h = MetricsHistory::new(6);
        for v in [Some(1.0), Some(2.0), None, Some(3.0), Some(4.0), Some(5.0)] {
            h.push(Series::Cpu, v);
        }
        let mut s = Scene::default();
        sparkline(
            &mut s,
            Rect::new(0.0, 0.0, 50.0, 20.0),
            &h,
            Series::Cpu,
            Some(10.0),
            Color::GREEN,
        );
        let lines = s
            .ops
            .iter()
            .filter(|o| matches!(o, crate::scene::Op::Line { .. }))
            .count();
        assert_eq!(lines, 2);
    }
}
