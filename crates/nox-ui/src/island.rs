//! The island model: what surface to show (rest, activity strip, notice,
//! expanded page), its animated size, and the scene for a frame. Pure Rust so
//! the whole behaviour is testable without AppKit.

use crate::alerts::AlertQueue;
use crate::compact::{self, Activity};
use crate::geom::{Point, Size};
use crate::geometry::NotchGeometry;
use crate::gesture::{GestureAction, GestureTracker, ScrollInput};
use crate::machine::{Machine, MachineConfig, Mode};
use crate::scene::{MusicPanel, Op, Scene};
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

    /// Scene for the current frame.
    pub fn scene(
        &self,
        state: &IslandState,
        settings: &Settings,
        now: f64,
        hover: Option<Point>,
    ) -> Scene {
        let size = self.size();
        let mut s = Scene::default();
        let open = self.motion.open.value.clamp(0.0, 1.0);
        // Closed content fades out as the island opens and vice versa.
        if open < 1.0 {
            s.ops.push(Op::Alpha(1.0 - open));
            let target = self.surface_size(&self.surface, settings);
            let settled_size = (size.w - target.w).abs() < 1.0 && (size.h - target.h).abs() < 1.0;
            match &self.surface {
                Surface::Strip(a) => {
                    views::compact::strip(
                        &mut s,
                        &self.geometry,
                        size,
                        a,
                        now,
                        &settings.companion.character,
                    );
                    s.hit(
                        crate::geom::Rect::new(0.0, 0.0, size.w, size.h),
                        crate::scene::Action::Open(views::compact::section_of(a)),
                        "Ouvrir",
                    );
                }
                Surface::Notice(a) if settled_size => {
                    views::compact::notice(&mut s, &self.geometry, size, a)
                }
                _ => {}
            }
        }
        if open > 0.0 {
            let hits_before = s.hits.len();
            if hits_before > 0 {
                s.hits.clear();
            }
            s.ops.push(Op::Alpha(open));
            if let Some(section) = self.current_section(settings) {
                let cx = Ctx {
                    state,
                    settings,
                    now,
                    hover,
                    scroll: self.scroll.get(&section).copied().unwrap_or(0.0),
                    music_panel: self.music_panel,
                    camera_attached: self.camera_attached,
                };
                let visible = settings.visible_sections();
                let content = views::chrome(
                    &mut s,
                    &cx,
                    &self.geometry,
                    size.w,
                    size.h,
                    &visible,
                    section,
                );
                if open > 0.6 {
                    views::page(&mut s, &cx, content, section);
                }
            } else {
                let r = crate::geom::Rect::new(
                    0.0,
                    self.geometry.camera_height,
                    size.w,
                    size.h - self.geometry.camera_height,
                );
                views::empty(
                    &mut s,
                    r,
                    "square.dashed",
                    "Aucune section",
                    "Activez des sections dans les réglages.",
                );
                s.hit(r, crate::scene::Action::OpenSettings, "Réglages");
            }
            if open < 0.99 {
                // No clicks on a half-open page.
                s.hits.clear();
            }
        }
        s.ops.push(Op::Alpha(1.0));
        s
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
    use nox_core::model::timers::{CountdownTimer, TimerRun, TimersState};
    use nox_core::settings::{IdleContent, IslandSilhouette};
    use nox_core::{AlertKind, ModuleId};

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
        let scene = m.scene(&st, &s, 0.0, None);
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
        let scene = m.scene(&st, &s, 0.3, None);
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
}
