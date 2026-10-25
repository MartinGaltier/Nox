//! The island model: what surface to show (rest, activity strip, notice,
//! expanded page), its animated size, and the scene for a frame. Pure Rust so
//! the whole behaviour is testable without AppKit.

use crate::alerts::AlertQueue;
use crate::compact::{self, Activity};
use crate::geom::{Point, Size};
use crate::geometry::NotchGeometry;
use crate::gesture::{GestureAction, GestureTracker, ScrollInput};
use crate::machine::{Machine, MachineConfig, Mode};
use crate::scene::{Group, LayeredScene, MusicPanel, Scene};
use crate::spring::IslandMotion;
use crate::views::{self, Ctx};
use nox_core::{Alert, IslandState, Section, Settings};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Surface {
    /// Nothing running: the camera region only (or nothing if hidden).
    Rest,
    Strip(Activity),
    Notice(Alert),
    Expanded,
}

pub struct IslandModel {
    pub machine: Machine,
    pub motion: IslandMotion,
    pub alerts: AlertQueue,
    pub geometry: NotchGeometry,
    pub section: Option<Section>,
    pub scroll: HashMap<Section, f64>,
    pub music_panel: MusicPanel,
    pub gestures: GestureTracker,
    /// Surface the motion currently aims at.
    pub surface: Surface,
    pub camera_attached: bool,
}

pub fn machine_config(s: &Settings) -> MachineConfig {
    MachineConfig {
        expand_on_hover: s.island.expand_on_hover,
        hover_delay: s.island.hover_delay_ms as f64 / 1000.0,
        close_delay: s.island.close_delay_ms as f64 / 1000.0,
    }
}

impl IslandModel {
    pub fn new(geometry: NotchGeometry, settings: &Settings) -> Self {
        let rest = geometry.compact_size(0.0);
        Self {
            machine: Machine::new(machine_config(settings)),
            motion: IslandMotion::new(rest.w, rest.h),
            alerts: AlertQueue::default(),
            geometry,
            section: None,
            scroll: HashMap::new(),
            music_panel: MusicPanel::default(),
            gestures: GestureTracker::default(),
            surface: Surface::Rest,
            camera_attached: false,
        }
    }

    /// Current section, falling back to the first visible one.
    pub fn current_section(&self, settings: &Settings) -> Option<Section> {
        let visible = settings.visible_sections();
        match self.section {
            Some(s) if visible.contains(&s) => Some(s),
            _ => visible.first().copied(),
        }
    }

    pub fn select_section(&mut self, s: Section) {
        self.section = Some(s);
    }

    /// Next/previous visible section (wraps). Returns true if it changed.
    pub fn step_section(&mut self, settings: &Settings, forward: bool) -> bool {
        let visible = settings.visible_sections();
        if visible.len() < 2 {
            return false;
        }
        let cur = self.current_section(settings).unwrap_or(visible[0]);
        let i = visible.iter().position(|s| *s == cur).unwrap_or(0);
        let n = visible.len();
        let j = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.section = Some(visible[j]);
        true
    }

    pub fn push_alerts(&mut self, alerts: Vec<Alert>, settings: &Settings, now: f64) {
        if settings.island.show_alerts && !alerts.is_empty() {
            self.alerts.push(alerts, now);
        }
    }

    /// Decides the surface from mode, alerts and live activity.
    pub fn decide(&self, state: &IslandState, settings: &Settings, now: f64) -> Surface {
        if self.machine.mode == Mode::Expanded {
            return Surface::Expanded;
        }
        if let Some(a) = self.alerts.current() {
            return Surface::Notice(a.clone());
        }
        match compact::select(state, settings, now) {
            Some(a) => Surface::Strip(a),
            None => Surface::Rest,
        }
    }

    pub fn surface_size(&self, surface: &Surface, settings: &Settings) -> Size {
        let g = &self.geometry;
        match surface {
            Surface::Rest => g.compact_size(0.0),
            Surface::Strip(a) => g.compact_size(a.wing()),
            Surface::Notice(_) => views::compact::notice_size(g),
            Surface::Expanded => g.expanded_size(&settings.island),
        }
    }

    /// Re-targets the motion. Returns true if an animation must run.
    pub fn retarget(&mut self, state: &IslandState, settings: &Settings, now: f64) -> bool {
        let surface = self.decide(state, settings, now);
        let size = self.surface_size(&surface, settings);
        let open = surface == Surface::Expanded;
        self.surface = surface;
        self.motion.set_target(size.w, size.h, open);
        !self.motion.settled()
    }

    /// Whether the shape is a fully rounded capsule (closed, floating).
    pub fn capsule_compact(&self) -> bool {
        self.geometry.floats() && self.motion.open.value < 0.5
    }

    /// Size currently drawn.
    pub fn size(&self) -> Size {
        Size::new(self.motion.w.value.max(0.0), self.motion.h.value.max(0.0))
    }

    /// The content as layers, each laid out for its own target size: the
    /// page for the expanded shape, the closed surface for its strip or
    /// notice. Nothing depends on the animated width, so a layer only changes
    /// when its content does (see [`Group::generation`]).
    pub fn layers(
        &self,
        state: &IslandState,
        settings: &Settings,
        now: f64,
        hover: Option<Point>,
    ) -> LayeredScene {
        let _p = crate::perf::Probe::new("layers");
        let g = &self.geometry;
        let silhouette = self.size();
        let open = self.motion.open.value.clamp(0.0, 1.0);

        let closed_target = self.surface_size(&self.surface, settings);
        let mut closed = Scene::default();
        match &self.surface {
            Surface::Strip(a) => {
                views::compact::strip(&mut closed, g, closed_target, a, now, &settings.companion);
                if open < 0.01 {
                    let r = crate::geom::Rect::new(0.0, 0.0, closed_target.w, closed_target.h);
                    closed.hit(
                        r,
                        crate::scene::Action::Open(views::compact::section_of(a)),
                        "Ouvrir",
                    );
                }
            }
            Surface::Notice(a) => views::compact::notice(&mut closed, g, closed_target, a),
            Surface::Rest | Surface::Expanded => {}
        }

        let page_target = g.expanded_size(&settings.island);
        let mut page = Scene::default();
        if open > 0.0 || self.surface == Surface::Expanded {
            // Hover arrives in the animated shape's space; the page is laid out
            // at its own target, so shift it by the difference of the two origins.
            let shift = g.shape_x(silhouette.w) - g.shape_x(page_target.w);
            let hover = hover.map(|p| Point::new(p.x + shift, p.y));
            let section = self.current_section(settings);
            let cx = Ctx {
                state,
                settings,
                now,
                hover,
                scroll: section
                    .and_then(|s| self.scroll.get(&s).copied())
                    .unwrap_or(0.0),
                music_panel: self.music_panel,
                camera_attached: self.camera_attached,
            };
            match section {
                Some(section) => {
                    let visible = settings.visible_sections();
                    let content = views::chrome(
                        &mut page,
                        &cx,
                        g,
                        page_target.w,
                        page_target.h,
                        &visible,
                        section,
                    );
                    views::page(&mut page, &cx, content, section);
                }
                None => {
                    let r = crate::geom::Rect::new(
                        0.0,
                        g.camera_height,
                        page_target.w,
                        page_target.h - g.camera_height,
                    );
                    views::empty(
                        &mut page,
                        r,
                        "square.dashed",
                        "Aucune section",
                        "Activez des sections dans les réglages.",
                    );
                    page.hit(r, crate::scene::Action::OpenSettings, "Réglages");
                }
            }
            if open < 0.99 {
                // No clicks on a half-open page.
                page.hits.clear();
            }
        }

        let refresh = match (closed.refresh, page.refresh) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        LayeredScene {
            silhouette,
            refresh,
            shows_metrics: page.shows_metrics,
            shows_camera: page.shows_camera,
            scroll_extent: page.scroll_extent,
            closed: Group::new(
                closed,
                closed_target,
                Point::new(g.shape_x(closed_target.w), 0.0),
            ),
            page: Group::new(page, page_target, Point::new(g.shape_x(page_target.w), 0.0)),
        }
    }

    /// Scroll gesture; returns the action to apply (section change etc.).
    pub fn scroll(
        &mut self,
        e: ScrollInput,
        settings: &Settings,
        in_header: bool,
        extent: f64,
    ) -> Option<GestureAction> {
        if !settings.island.gestures {
            return None;
        }
        let expanded = self.machine.mode == Mode::Expanded;
        // Inside a scrollable page, vertical scrolling scrolls the list.
        if expanded
            && !in_header
            && extent > 0.0
            && let Some(sec) = self.current_section(settings)
            && e.y.abs() > e.x.abs()
            && !e.momentum
        {
            let v = self.scroll.entry(sec).or_insert(0.0);
            *v = (*v - e.y).clamp(0.0, extent);
            return None;
        }
        let allow_vertical = !expanded || in_header || extent <= 0.0;
        self.gestures.handle(e, allow_vertical, expanded, expanded)
    }

    /// Clamps scroll after content changed.
    pub fn clamp_scroll(&mut self, section: Section, extent: f64) {
        if let Some(v) = self.scroll.get_mut(&section) {
            *v = v.clamp(0.0, extent.max(0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;
    use crate::geometry::ScreenParams;
    use crate::scene::Op;
    use nox_core::model::timers::{CountdownTimer, TimerRun, TimersState};
    use nox_core::settings::{IdleContent, IslandSilhouette};
    use nox_core::{AlertKind, ModuleId};

    /// Both layers as one scene, for assertions on text and hits.
    fn whole(l: &LayeredScene) -> Scene {
        let mut s = Scene {
            refresh: l.refresh,
            ..Default::default()
        };
        for g in [&l.closed, &l.page] {
            s.ops.extend(g.ops.iter().cloned());
            s.hits.extend(g.hits.iter().cloned());
        }
        s
    }

    fn model(settings: &Settings) -> IslandModel {
        let g = NotchGeometry::new(
            &ScreenParams {
                frame: Rect::new(0.0, 0.0, 1470.0, 956.0),
                visible_frame: Rect::new(0.0, 0.0, 1470.0, 919.0),
                safe_area_top: 32.0,
                aux_left: Some(Rect::new(0.0, 924.0, 642.5, 32.0)),
                aux_right: Some(Rect::new(827.5, 924.0, 642.5, 32.0)),
                status_bar_thickness: 24.0,
            },
            true,
            IslandSilhouette::Capsule,
        );
        IslandModel::new(g, settings)
    }

    fn settle(m: &mut IslandModel) {
        for _ in 0..1000 {
            m.motion.step(1.0 / 120.0);
        }
        assert!(m.motion.settled());
    }

    #[test]
    fn rest_is_exactly_the_camera_and_idle_scene_is_empty() {
        let mut s = Settings::default();
        s.island.idle_content = IdleContent::Nothing;
        let mut m = model(&s);
        let st = IslandState::default();
        assert!(!m.retarget(&st, &s, 0.0), "nothing to animate at rest");
        assert_eq!(m.size(), Size::new(185.0, 32.0));
        let scene = whole(&m.layers(&st, &s, 0.0, None));
        assert!(scene.refresh.is_none(), "no timer at rest");
        assert!(scene.hits.is_empty());
    }

    #[test]
    fn hover_expands_with_animation_and_scene_has_page() {
        let s = Settings::default();
        let mut m = model(&s);
        let st = IslandState::default();
        m.machine.pointer(true, 0.0);
        m.machine.tick(0.3);
        assert!(m.retarget(&st, &s, 0.3));
        settle(&mut m);
        assert_eq!(m.size().w, 520.0);
        let scene = whole(&m.layers(&st, &s, 0.3, None));
        assert!(scene.has_text("Musique"), "{:?}", scene.texts());
        assert!(!scene.hits.is_empty());
    }

    #[test]
    fn alert_shows_notice_then_returns_to_activity() {
        let s = Settings::default();
        let mut m = model(&s);
        let st = IslandState {
            timers: Some(TimersState {
                timers: vec![CountdownTimer {
                    ends_at: None,
                    id: 1,
                    label: String::new(),
                    duration_secs: 60.0,
                    remaining_secs: 30.0,
                    observed_at: 0.0,
                    run: TimerRun::Running,
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        m.retarget(&st, &s, 0.0);
        assert!(matches!(m.surface, Surface::Strip(Activity::Timer { .. })));
        m.push_alerts(
            vec![Alert::new(AlertKind::TimerDone, "Fini", "timer").ttl(3.0)],
            &s,
            1.0,
        );
        m.retarget(&st, &s, 1.0);
        assert!(matches!(m.surface, Surface::Notice(_)));
        assert_eq!(m.alerts.deadline(), Some(4.0));
        m.alerts.tick(4.0);
        m.retarget(&st, &s, 4.0);
        assert!(matches!(m.surface, Surface::Strip(_)));
    }

    #[test]
    fn alerts_can_be_disabled() {
        let mut s = Settings::default();
        s.island.show_alerts = false;
        let mut m = model(&s);
        m.push_alerts(vec![Alert::new(AlertKind::Info, "x", "bell")], &s, 0.0);
        assert!(m.alerts.current().is_none());
    }

    #[test]
    fn section_paging_skips_disabled_modules() {
        let mut s = Settings::default();
        let mut m = model(&s);
        assert_eq!(m.current_section(&s), Some(Section::Music));
        m.step_section(&s, true);
        assert_eq!(m.current_section(&s), Some(Section::Calendar));
        s.set_module_enabled(ModuleId::Calendar, false);
        assert_eq!(m.current_section(&s), Some(Section::Music), "falls back");
        m.step_section(&s, false);
        assert_eq!(m.current_section(&s), s.visible_sections().last().copied());
    }

    #[test]
    fn list_scroll_is_clamped() {
        let s = Settings::default();
        let mut m = model(&s);
        m.machine.open();
        let e = ScrollInput {
            x: 0.0,
            y: -500.0,
            timestamp: 0.0,
            began: true,
            ended: false,
            momentum: false,
            precise: true,
            has_phase: true,
        };
        assert_eq!(m.scroll(e, &s, false, 120.0), None);
        assert_eq!(m.scroll.get(&Section::Music), Some(&120.0));
    }

    fn timer_state(remaining: f64) -> IslandState {
        IslandState {
            timers: Some(TimersState {
                timers: vec![CountdownTimer {
                    ends_at: None,
                    id: 1,
                    label: String::new(),
                    duration_secs: 60.0,
                    remaining_secs: remaining,
                    observed_at: 0.0,
                    run: TimerRun::Running,
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn layers_hold_no_alpha_and_hits_follow_the_open_fraction() {
        let s = Settings::default();
        let mut m = model(&s);
        let st = IslandState::default();
        let page = m.geometry.expanded_size(&s.island);
        m.machine.open();
        m.retarget(&st, &s, 0.0);
        m.motion.snap(page.w, page.h, true);
        let open = m.layers(&st, &s, 0.0, None);
        assert!(!open.page.ops.iter().any(|o| matches!(o, Op::Alpha(_))));
        assert!(!open.closed.ops.iter().any(|o| matches!(o, Op::Alpha(_))));
        assert!(!open.page.hits.is_empty(), "a fully open page takes clicks");
        m.motion.snap(page.w, page.h, false);
        m.surface = Surface::Expanded;
        let half = m.layers(&st, &s, 0.0, None);
        assert!(half.page.hits.is_empty(), "no clicks on a half-open page");
        assert_eq!(half.page.ops, open.page.ops, "same content at any opacity");
        assert_eq!(
            half.page.generation, open.page.generation,
            "no redraw during the fade"
        );
    }

    #[test]
    fn page_layer_ignores_the_animated_width() {
        let s = Settings::default();
        let mut m = model(&s);
        let st = IslandState::default();
        let page = m.geometry.expanded_size(&s.island);
        m.machine.open();
        m.retarget(&st, &s, 0.0);
        m.motion.snap(page.w, page.h, true);
        let full = m.layers(&st, &s, 0.0, None);
        m.motion.snap(page.w - 80.0, page.h, true);
        let narrower = m.layers(&st, &s, 0.0, None);
        assert_eq!(narrower.page.target, page);
        assert_eq!(narrower.page.ops, full.page.ops);
        assert_eq!(narrower.page.generation, full.page.generation);
        assert_eq!(narrower.silhouette.w, page.w - 80.0);
    }

    #[test]
    fn closed_layer_is_laid_out_for_its_strip_not_the_shape() {
        let s = Settings::default();
        let mut m = model(&s);
        let st = timer_state(30.0);
        m.retarget(&st, &s, 0.0);
        let target = m.surface_size(&m.surface, &s);
        m.motion.snap(target.w + 90.0, target.h, false);
        let a = m.layers(&st, &s, 0.0, None);
        m.motion.snap(target.w, target.h, false);
        let b = m.layers(&st, &s, 0.0, None);
        assert!(!a.closed.ops.is_empty());
        assert_eq!(a.closed.ops, b.closed.ops);
        assert_eq!(a.closed.generation, b.closed.generation);
        assert_eq!(a.closed.target, target);
        assert!(
            a.page.ops.is_empty(),
            "nothing open, nothing on the page layer"
        );
    }

    #[test]
    fn generation_changes_only_with_content() {
        let s = Settings::default();
        let mut m = model(&s);
        let st = timer_state(30.0);
        m.retarget(&st, &s, 0.0);
        let first = m.layers(&st, &s, 0.0, None);
        let again = m.layers(&st, &s, 0.0, None);
        assert_eq!(first.closed.generation, again.closed.generation);
        // The strip's activity is chosen by `retarget`, as on every state update.
        let later = timer_state(5.0);
        m.retarget(&later, &s, 0.0);
        let ticked = m.layers(&later, &s, 0.0, None);
        assert_ne!(first.closed.generation, ticked.closed.generation);
    }

    #[test]
    fn closed_strip_opens_on_click_only_while_closed() {
        let s = Settings::default();
        let mut m = model(&s);
        let st = timer_state(30.0);
        m.retarget(&st, &s, 0.0);
        let closed = m.layers(&st, &s, 0.0, None);
        assert!(
            matches!(closed.closed.hits.as_slice(), [h] if h.action == crate::scene::Action::Open(Some(Section::Timers))),
            "{:?}",
            closed.closed.hits
        );
        m.motion
            .snap(m.geometry.expanded_size(&s.island).w, 100.0, true);
        let fading = m.layers(&st, &s, 0.0, None);
        assert!(fading.closed.hits.is_empty());
    }
}
