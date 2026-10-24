//! Main-thread controller: owns the island model, the panel, timers and the
//! auxiliary windows. Woken by the hub (coalesced); timers run only while
//! something on screen changes (animation, countdown, hover delay, alert TTL).

use super::island_window::{IslandPanel, IslandView};
use super::painter::Painter;
use super::util::{Observer, Timer, mono, ns_rect};
use super::{files, haptics, screens};
use crate::UiContext;
use crate::geom::{Point, Rect, Size};
use crate::geometry::{NotchGeometry, island_contains};
use crate::gesture::{GestureAction, ScrollInput, movement};
use crate::island::{IslandModel, Surface, machine_config};
use crate::machine::Mode;
use crate::scene::{Action, Scene};
use nox_core::model::files::FileCommand;
use nox_core::model::music::MusicCommand;
use nox_core::model::watch::WatchCommand;
use nox_core::{Command, Hub, IslandState, Section, Settings, SettingsStore};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, msg_send};
use objc2_app_kit::{
    NSAppearanceCustomization, NSApplicationDidChangeScreenParametersNotification, NSColor,
    NSEvent, NSEventPhase, NSPasteboardTypeFileURL, NSWindowCollectionBehavior,
    NSWindowSharingType, NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification,
    NSWorkspaceDidActivateApplicationNotification,
};
use objc2_core_graphics::CGContext;
use objc2_foundation::{NSArray, NSNotificationCenter};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

pub fn get() -> Option<Rc<App>> {
    APP.with(|a| a.borrow().clone())
}

/// Debug-only overrides for visual verification (never compiled in release).
#[derive(Debug, Clone, Default)]
pub struct DebugOptions {
    pub fixture: bool,
    pub force: Option<(Mode, Option<Section>)>,
}

impl DebugOptions {
    #[cfg(debug_assertions)]
    pub fn from_env() -> Self {
        let fixture = std::env::var("NOX_UI_FIXTURE").is_ok_and(|v| v == "1");
        let force = std::env::var("NOX_UI_FORCE").ok().and_then(|v| {
            let (mode, sec) = v.split_once(':').unwrap_or((v.as_str(), ""));
            let mode = match mode {
                "expanded" => Mode::Expanded,
                "compact" => Mode::Compact,
                _ => return None,
            };
            let sec = Section::DEFAULT_ORDER
                .into_iter()
                .find(|s| format!("{s:?}").eq_ignore_ascii_case(sec));
            Some((mode, sec))
        });
        Self { fixture, force }
    }
    #[cfg(not(debug_assertions))]
    pub fn from_env() -> Self {
        Self::default()
    }
}

struct Inner {
    settings: Arc<Settings>,
    state: IslandState,
    model: IslandModel,
    painter: Painter,
    scene: Scene,
    hover: Option<Point>,
    /// Shape offset inside the view (the window is the animation's bounding box).
    offset: f64,
    anim: Option<Timer>,
    last_tick: f64,
    anim_box: Size,
    deadline: Option<Timer>,
    refresh: Option<Timer>,
    metrics_sent: Option<bool>,
    camera_on: bool,
    hidden_fullscreen: bool,
    screen_id: Option<u32>,
    artwork: HashMap<u64, Arc<Vec<u8>>>,
    seek_drag: Option<(f64, f64, f64)>,
    page_shown: Option<Section>,
    /// Region selection asked; the overlay opens once watch reports Selecting.
    pending_selection: Option<nox_core::model::watch::WatchTrigger>,
    /// Damage of the last relayout, consumed when invalidating the view.
    damage: Option<crate::scene::Damage>,
    dragging_files: bool,
}

pub struct App {
    pub mtm: MainThreadMarker,
    pub hub: Hub,
    store: SettingsStore,
    on_settings_changed: Box<dyn Fn(Arc<Settings>)>,
    on_quit: RefCell<Option<Box<dyn FnOnce()>>>,
    debug: DebugOptions,
    inner: RefCell<Inner>,
    panel: Retained<IslandPanel>,
    view: Retained<IslandView>,
    observers: RefCell<Vec<Observer>>,
    pub status: RefCell<Option<super::status::StatusBar>>,
    pub settings_window: RefCell<Option<super::settings_window::SettingsWindow>>,
    pub monitor_window: RefCell<Option<super::monitor_window::MonitorWindow>>,
    pub overlay: RefCell<Option<super::overlay::SelectionOverlay>>,
    hotkey: RefCell<Option<super::hotkey::HotKey>>,
}

impl App {
    pub fn install(mtm: MainThreadMarker, ctx: UiContext) -> Rc<App> {
        let settings = ctx.settings.clone();
        let screen = screens::choose(mtm, settings.island.display);
        let params =
            screen
                .as_deref()
                .map(screens::params)
                .unwrap_or(crate::geometry::ScreenParams {
                    frame: Rect::new(0.0, 0.0, 1440.0, 900.0),
                    visible_frame: Rect::new(0.0, 0.0, 1440.0, 875.0),
                    safe_area_top: 0.0,
                    aux_left: None,
                    aux_right: None,
                    status_bar_thickness: 24.0,
                });
        let geometry = NotchGeometry::new(
            &params,
            settings.island.simulate_notch,
            settings.island.silhouette,
        );
        log::info!(
            "island geometry: notched={} camera={}x{} centre={} bar={}",
            geometry.notched,
            geometry.camera_width,
            geometry.camera_height,
            geometry.center_x,
            geometry.bar_height
        );
        let model = IslandModel::new(geometry, &settings);
        let size = model.size();
        let frame = geometry.panel_frame(size);
        let panel = IslandPanel::new(mtm, ns_rect(frame));
        let view = IslandView::new(mtm, ns_rect(Rect::new(0.0, 0.0, frame.w, frame.h)));
        configure_panel(&panel, &view, &settings);

        let debug = DebugOptions::from_env();
        let app = Rc::new(App {
            mtm,
            hub: ctx.hub,
            store: ctx.store,
            on_settings_changed: ctx.on_settings_changed,
            on_quit: RefCell::new(Some(ctx.on_quit)),
            debug,
            inner: RefCell::new(Inner {
                settings,
                state: IslandState::default(),
                model,
                painter: Painter::default(),
                scene: Scene::default(),
                hover: None,
                offset: 0.0,
                anim: None,
                last_tick: 0.0,
                anim_box: size,
                deadline: None,
                refresh: None,
                metrics_sent: None,
                camera_on: false,
                hidden_fullscreen: false,
                screen_id: screen.as_deref().and_then(screens::display_id),
                artwork: HashMap::new(),
                seek_drag: None,
                page_shown: None,
                pending_selection: None,
                damage: None,
                dragging_files: false,
            }),
            panel,
            view,
            observers: RefCell::new(vec![]),
            status: RefCell::new(None),
            settings_window: RefCell::new(None),
            monitor_window: RefCell::new(None),
            overlay: RefCell::new(None),
            hotkey: RefCell::new(None),
        });
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));
        app.after_install();
        app
    }

    fn after_install(self: &Rc<Self>) {
        // Hub waker → main queue (coalesced by the hub until acknowledged).
        self.hub.set_waker(|| {
            nox_core::main_thread::run_async(|| {
                if let Some(a) = get() {
                    a.on_wake();
                }
            })
        });
        let nc = NSNotificationCenter::defaultCenter();
        let ws = NSWorkspace::sharedWorkspace().notificationCenter();
        let mut obs = self.observers.borrow_mut();
        // SAFETY: AppKit notification names are valid static NSStrings.
        let (screens_changed, space, activate) = unsafe {
            (
                NSApplicationDidChangeScreenParametersNotification,
                NSWorkspaceActiveSpaceDidChangeNotification,
                NSWorkspaceDidActivateApplicationNotification,
            )
        };
        obs.push(Observer::new(nc, screens_changed, || {
            if let Some(a) = get() {
                a.screens_changed();
            }
        }));
        obs.push(Observer::new(ws.clone(), space, || {
            if let Some(a) = get() {
                a.update_fullscreen();
            }
        }));
        obs.push(Observer::new(ws, activate, || {
            if let Some(a) = get() {
                a.update_fullscreen();
            }
        }));
        drop(obs);

        *self.status.borrow_mut() = Some(super::status::StatusBar::new(self.mtm));
        self.apply_hotkey();
        if let Some((mode, sec)) = self.debug.force {
            let mut i = self.inner.borrow_mut();
            if let Some(s) = sec {
                i.model.select_section(s);
            }
            if mode == Mode::Expanded {
                i.model.machine.open();
                i.model.machine.hold(true, mono());
            }
        }
        self.panel.orderFrontRegardless();
        self.on_wake();
        self.update_fullscreen();
        #[cfg(debug_assertions)]
        super::snapshot::schedule(self);
    }

    #[cfg(debug_assertions)]
    pub fn with_view(&self, f: impl FnOnce(&objc2_app_kit::NSView)) {
        f(&self.view);
    }

    /// Writes the real screen geometry and panel frame (debug verification).
    #[cfg(debug_assertions)]
    pub fn debug_dump(&self, path: &str) {
        let i = self.inner.borrow();
        let screen = screens::choose(self.mtm, i.settings.island.display);
        let raw = screen.as_deref().map(|s| {
            (
                s.frame(),
                s.safeAreaInsets(),
                s.auxiliaryTopLeftArea(),
                s.auxiliaryTopRightArea(),
                s.backingScaleFactor(),
            )
        });
        let app = objc2_app_kit::NSApplication::sharedApplication(self.mtm);
        let focus = format!(
            "panel: canBecomeKey={} canBecomeMain={} isKey={} nonactivating={} level={} \
             behavior={:?} sharing={:?} ignoresMouse={} | app active={} policy={:?}\n",
            self.panel.canBecomeKeyWindow(),
            self.panel.canBecomeMainWindow(),
            self.panel.isKeyWindow(),
            self.panel
                .styleMask()
                .contains(objc2_app_kit::NSWindowStyleMask::NonactivatingPanel),
            self.panel.level(),
            self.panel.collectionBehavior(),
            self.panel.sharingType(),
            self.panel.ignoresMouseEvents(),
            app.isActive(),
            app.activationPolicy(),
        );
        let text = focus
            + &format!(
                "raw NSScreen (frame, safeAreaInsets, auxLeft, auxRight, scale): {raw:#?}\n\
             geometry: {:#?}\npanel frame: {:?}\nshape size: {:?}\nsurface: {:?}\n",
                i.model.geometry,
                super::util::rect(self.panel.frame()),
                i.model.size(),
                i.model.surface,
            );
        let _ = std::fs::write(path, text);
    }

    pub fn settings(&self) -> Arc<Settings> {
        self.inner.borrow().settings.clone()
    }

    /// Hub woke us: re-read state and alerts.
    pub fn on_wake(&self) {
        self.hub.acknowledge_wake();
        let mut snapshot = self.hub.snapshot();
        let alerts = self.hub.drain_alerts();
        #[cfg(debug_assertions)]
        if self.debug.fixture {
            super::fixture::overlay(&mut snapshot, nox_core::model::now());
        }
        {
            let mut i = self.inner.borrow_mut();
            // Keep artwork bytes by id for the painter's cache.
            if let Some(np) = snapshot.music.as_ref().and_then(|m| m.now_playing.as_ref())
                && let Some(bytes) = &np.artwork
            {
                i.artwork.insert(np.artwork_id, bytes.clone());
                let keep = np.artwork_id;
                i.artwork.retain(|k, _| *k == keep);
                i.painter.retain_artwork(&[keep]);
            }
            i.state = snapshot;
            let settings = i.settings.clone();
            i.model.push_alerts(alerts, &settings, mono());
        }
        self.sync_watch();
        self.update();
        if let Some(s) = self.status.borrow().as_ref() {
            s.refresh_icon(&self.inner.borrow().state);
        }
        if let Some(m) = self.monitor_window.borrow().as_ref() {
            m.redraw();
        }
    }

    /// Re-decides the surface, (re)starts animation, lays out and redraws.
    pub fn update(&self) {
        let now_m = mono();
        let animating = {
            let mut i = self.inner.borrow_mut();
            let Inner {
                model,
                state,
                settings,
                ..
            } = &mut *i;
            model.machine.tick(now_m);
            model.alerts.tick(now_m);
            model.retarget(state, settings, nox_core::model::now())
        };
        if animating {
            self.start_animation();
        } else {
            self.apply_frame(true);
        }
        self.relayout();
        self.schedule_deadline();
    }

    fn start_animation(&self) {
        let mut i = self.inner.borrow_mut();
        let target = Size::new(i.model.motion.w.target, i.model.motion.h.target);
        let cur = i.model.size();
        let b = i.anim_box;
        i.anim_box = Size::new(b.w.max(target.w).max(cur.w), b.h.max(target.h).max(cur.h));
        if i.anim.is_none() {
            i.last_tick = mono();
            i.anim = Some(Timer::new(1.0 / 120.0, true, || {
                if let Some(a) = get() {
                    a.tick_animation();
                }
            }));
        }
        drop(i);
        self.apply_frame(false);
    }

    fn tick_animation(&self) {
        let settled = {
            let mut i = self.inner.borrow_mut();
            let t = mono();
            let dt = (t - i.last_tick).clamp(0.0, 0.05);
            i.last_tick = t;
            i.model.motion.step(dt);
            i.model.motion.settled()
        };
        if settled {
            {
                let mut i = self.inner.borrow_mut();
                i.anim = None;
                i.anim_box = i.model.size();
            }
            self.apply_frame(true);
            self.panel.invalidateShadow();
            self.recheck_pointer();
        }
        self.relayout();
    }

    /// Positions the panel. During an animation the window keeps the
    /// bounding box of all sizes involved (two resizes per animation).
    fn apply_frame(&self, settled: bool) {
        let (frame, offset, expanded) = {
            let mut i = self.inner.borrow_mut();
            let size = i.model.size();
            let bx = if settled {
                i.anim_box = size;
                size
            } else {
                i.anim_box
            };
            let f = i.model.geometry.panel_frame(bx);
            i.offset = (f.w - size.w) / 2.0;
            (f, i.offset, i.model.motion.open.value > 0.5)
        };
        let _ = offset;
        let cur = super::util::rect(self.panel.frame());
        if cur != frame {
            self.inner.borrow_mut().damage = Some(crate::scene::Damage::All);
            self.panel.setFrame_display(ns_rect(frame), false);
            self.view
                .setFrame(ns_rect(Rect::new(0.0, 0.0, frame.w, frame.h)));
        }
        self.panel.setHasShadow(expanded);
    }

    fn relayout(&self) {
        let (shows_metrics, shows_camera, refresh, extent, section) = {
            let mut i = self.inner.borrow_mut();
            let hover = i.hover.map(|p| Point::new(p.x - i.offset, p.y));
            let scene = i
                .model
                .scene(&i.state, &i.settings, nox_core::model::now(), hover);
            let r = (
                scene.shows_metrics,
                scene.shows_camera,
                scene.refresh,
                scene.scroll_extent,
                i.model.current_section(&i.settings),
            );
            let dmg = crate::scene::damage(&i.scene, &scene);
            i.scene = scene;
            i.damage = Some(dmg);
            r
        };
        if let Some(sec) = section {
            self.inner.borrow_mut().model.clamp_scroll(sec, extent);
        }
        let expanded_page = self.inner.borrow().model.machine.mode == Mode::Expanded;
        // One-shot refreshes when a page appears (never polled).
        let shown = section.filter(|_| expanded_page);
        let appeared = {
            let mut i = self.inner.borrow_mut();
            let changed = i.page_shown != shown;
            i.page_shown = shown;
            changed.then_some(shown).flatten()
        };
        if appeared == Some(Section::Shortcuts) {
            use nox_core::model::shortcuts::ShortcutCommand;
            self.hub
                .send(Command::Shortcuts(ShortcutCommand::RefreshControls));
        }
        self.set_metrics_visible(shows_metrics && expanded_page);
        self.set_camera_visible(shows_camera && expanded_page);
        // Content refresh (countdowns, progress): one-shot timer, only if needed.
        let mut i = self.inner.borrow_mut();
        i.refresh = refresh.filter(|_| !i.hidden_fullscreen).map(|secs| {
            Timer::new(secs, false, || {
                if let Some(a) = get() {
                    a.content_tick();
                }
            })
        });
        let dmg = i.damage.take();
        let (offset, animating) = (i.offset, i.anim.is_some());
        drop(i);
        // Unchanged scene: nothing to draw; a small change (equalizer bars,
        // countdown digits): only that rectangle.
        match dmg {
            _ if animating => self.view.setNeedsDisplay(true),
            Some(crate::scene::Damage::None) => {}
            Some(crate::scene::Damage::Rect(r)) => self
                .view
                .setNeedsDisplayInRect(super::util::ns_rect(r.offset(offset, 0.0))),
            _ => self.view.setNeedsDisplay(true),
        }
    }

    fn content_tick(&self) {
        self.inner.borrow_mut().refresh = None;
        self.update();
    }

    fn schedule_deadline(&self) {
        let mut i = self.inner.borrow_mut();
        let next = match (i.model.machine.deadline(), i.model.alerts.deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        i.deadline = next.map(|t| {
            Timer::new((t - mono()).max(0.0) + 0.001, false, || {
                if let Some(a) = get() {
                    a.inner.borrow_mut().deadline = None;
                    a.update();
                }
            })
        });
    }

    pub fn set_metrics_visible(&self, page: bool) {
        let window = self
            .monitor_window
            .borrow()
            .as_ref()
            .is_some_and(|m| m.is_visible());
        let want = page || window;
        let mut i = self.inner.borrow_mut();
        if i.metrics_sent != Some(want) {
            i.metrics_sent = Some(want);
            drop(i);
            self.hub.send(Command::MetricsVisible(want));
        }
    }

    fn set_camera_visible(&self, on: bool) {
        let changed = {
            let mut i = self.inner.borrow_mut();
            let c = i.camera_on != on;
            i.camera_on = on;
            c
        };
        if !changed {
            return;
        }
        use nox_core::model::camera::CameraCommand;
        if on {
            // Host first, then Start (camera contract).
            let attached = super::camera::set_visible(&self.view, true);
            self.inner.borrow_mut().model.camera_attached = attached;
            self.hub.send(Command::Camera(CameraCommand::Start));
        } else {
            // Stop first (indicator off), then detach and release the host.
            self.hub.send(Command::Camera(CameraCommand::Stop));
            super::camera::set_visible(&self.view, false);
            self.inner.borrow_mut().model.camera_attached = false;
        }
    }

    // ---------- drawing ----------

    pub fn draw_island(&self) {
        let Ok(mut i) = self.inner.try_borrow_mut() else {
            return;
        };
        let size = i.model.size();
        let offset = i.offset;
        let floats = i.model.geometry.floating_gap;
        let capsule = i.model.capsule_compact();
        let open = i.model.motion.open.value;
        let s = &i.settings.island;
        let (translucent, outline) = (s.translucent, s.outline);
        let rest_hidden = matches!(i.model.surface, Surface::Rest)
            && open <= 0.0
            && (!i.model.geometry.rest_visible
                || (s.hide_until_hover && !i.model.machine.inside()));
        if rest_hidden || size.w < 1.0 {
            return;
        }
        let Some(gc) = objc2_app_kit::NSGraphicsContext::currentContext() else {
            return;
        };
        let cg = gc.CGContext();
        CGContext::save_g_state(Some(&cg));
        CGContext::translate_ctm(Some(&cg), offset, 0.0);
        let path = super::painter::path(
            &crate::geometry::island_path(size, floats, capsule),
            0.0,
            0.0,
        );
        let bg = if translucent && open > 0.5 {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.82)
        } else {
            NSColor::blackColor()
        };
        bg.setFill();
        path.fill();
        if outline {
            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.18).setStroke();
            path.setLineWidth(1.5);
            path.stroke();
        }
        gc.saveGraphicsState();
        path.addClip();
        let Inner {
            painter,
            scene,
            artwork,
            ..
        } = &mut *i;
        let art = |id: u64| artwork.get(&id).cloned();
        painter.paint(scene, &art);
        gc.restoreGraphicsState();
        CGContext::restore_g_state(Some(&cg));
        // Position the camera host layer if the page shows it.
        let host = scene.ops.iter().find_map(|o| match o {
            crate::scene::Op::CameraHost { rect, radius } => {
                Some((rect.offset(offset, 0.0), *radius))
            }
            _ => None,
        });
        super::camera::place(host);
    }

    // ---------- pointer ----------

    pub fn shape_contains(&self, p: Point) -> bool {
        let Ok(i) = self.inner.try_borrow() else {
            return true;
        };
        let size = i.model.size();
        island_contains(
            size,
            i.model.geometry.floating_gap,
            i.model.capsule_compact(),
            Point::new(p.x - i.offset, p.y),
        )
    }

    pub fn pointer_moved(&self, p: Option<Point>) {
        let inside = p.is_some_and(|p| self.shape_contains(p));
        let redraw = {
            let mut i = self.inner.borrow_mut();
            i.hover = p.filter(|_| inside);
            let before = i.model.machine.mode;
            i.model.machine.pointer(inside, mono());
            i.model.machine.mode != before || i.model.machine.mode == Mode::Expanded
        };
        if redraw {
            self.update();
        } else {
            self.schedule_deadline();
        }
    }

    fn recheck_pointer(&self) {
        let m = NSEvent::mouseLocation();
        let f = super::util::rect(self.panel.frame());
        let p = Point::new(m.x - f.x, f.max_y() - m.y);
        let inside_window = p.x >= 0.0 && p.y >= 0.0 && p.x <= f.w && p.y <= f.h;
        self.pointer_moved(inside_window.then_some(p));
    }

    pub fn mouse_down(&self, p: Point) {
        let (hit, mode) = {
            let i = self.inner.borrow();
            let local = Point::new(p.x - i.offset, p.y);
            (
                i.scene.hit_at(local).map(|h| h.action.clone()),
                i.model.machine.mode,
            )
        };
        match hit {
            Some(Action::Seek { x, w, duration }) => {
                self.inner.borrow_mut().seek_drag = Some((x, w, duration));
                self.perform(Action::Seek { x, w, duration }, Some(p));
            }
            Some(a) => self.perform(a, Some(p)),
            None if mode == Mode::Compact => {
                self.inner.borrow_mut().model.machine.click(mono());
                self.update();
            }
            None => {}
        }
    }

    pub fn mouse_dragged(&self, _p: Point) {
        // Seek is applied on mouse down; dragging along the bar re-seeks on
        // release of the next click (no continuous seek flood to the player).
    }

    pub fn scroll(&self, e: &NSEvent, p: Point) {
        let precise = e.hasPreciseScrollingDeltas();
        let inverted = e.isDirectionInvertedFromDevice();
        let phase = e.phase();
        let momentum = e.momentumPhase() != NSEventPhase::empty();
        // Finger travel: +x = right, +y = down (whatever the scroll direction setting).
        let input = ScrollInput {
            x: movement(e.scrollingDeltaX(), precise, inverted),
            y: movement(e.scrollingDeltaY(), precise, inverted),
            timestamp: e.timestamp(),
            began: phase.contains(NSEventPhase::Began),
            ended: phase.contains(NSEventPhase::Ended) || phase.contains(NSEventPhase::Cancelled),
            momentum,
            precise,
            has_phase: phase != NSEventPhase::empty(),
        };
        let action = {
            let mut i = self.inner.borrow_mut();
            let in_header = p.y < i.model.geometry.camera_height + 4.0;
            let extent = i.scene.scroll_extent;
            let settings = i.settings.clone();
            i.model.scroll(input, &settings, in_header, extent)
        };
        let haptic = self.settings().island.haptics;
        match action {
            Some(GestureAction::Open) => {
                self.inner.borrow_mut().model.machine.open();
            }
            Some(GestureAction::Close) => {
                self.inner.borrow_mut().model.machine.close(mono());
            }
            Some(g @ (GestureAction::NextSection | GestureAction::PreviousSection)) => {
                let changed = {
                    let mut i = self.inner.borrow_mut();
                    let settings = i.settings.clone();
                    i.model
                        .step_section(&settings, g == GestureAction::NextSection)
                };
                if changed && haptic {
                    haptics::tick();
                }
            }
            None => {}
        }
        self.update();
    }

    // ---------- drag & drop ----------

    pub fn drag_entered(&self) {
        {
            let mut i = self.inner.borrow_mut();
            i.dragging_files = true;
            i.model.machine.hold(true, mono());
            i.model.machine.open();
            if i.settings.visible_sections().contains(&Section::Files) {
                i.model.select_section(Section::Files);
            }
        }
        self.update();
    }

    pub fn drag_ended(&self, paths: Option<Vec<String>>) {
        {
            let mut i = self.inner.borrow_mut();
            i.dragging_files = false;
            if self.debug.force.is_none() {
                i.model.machine.hold(false, mono());
            }
        }
        if let Some(p) = paths.filter(|p| !p.is_empty()) {
            self.hub.send(Command::Files(FileCommand::Add(p)));
        }
        self.recheck_pointer();
        self.update();
    }

    // ---------- actions ----------

    pub fn perform(&self, a: Action, click: Option<Point>) {
        let haptic = self.settings().island.haptics;
        match a {
            Action::Send(c) => self.hub.send(c),
            Action::SendAll(cs) => cs.into_iter().for_each(|c| self.hub.send(c)),
            Action::SelectSection(s) => {
                self.inner.borrow_mut().model.select_section(s);
                if haptic {
                    haptics::tick();
                }
            }
            Action::Seek { x, w, duration } => {
                if let Some(p) = click {
                    let off = self.inner.borrow().offset;
                    let f = ((p.x - off - x) / w.max(1.0)).clamp(0.0, 1.0);
                    self.hub
                        .send(Command::Music(MusicCommand::Seek(f * duration)));
                }
            }
            Action::MusicPanel(p) => self.inner.borrow_mut().model.music_panel = p,
            Action::OpenSettings => super::settings_window::show(self),
            Action::OpenMonitor => super::monitor_window::show(self),
            Action::InstallHelper => super::alerts::install_helper(self),
            Action::BeginWatchSelection(trigger) => {
                self.hub.send(Command::Watch(WatchCommand::BeginSelection));
                let mut i = self.inner.borrow_mut();
                i.pending_selection = Some(trigger);
                i.model.machine.close(mono());
            }
            Action::OpenUrl(u) => files::open_url(&u),
            Action::PickFiles => {
                let paths = files::pick_files(self.mtm);
                if !paths.is_empty() {
                    self.hub.send(Command::Files(FileCommand::Add(paths)));
                }
            }
            Action::SetLevel { x, w, control } => {
                if let Some(p) = click {
                    let off = self.inner.borrow().offset;
                    let f = ((p.x - off - x) / w.max(1.0)).clamp(0.0, 1.0) as f32;
                    use nox_core::model::shortcuts::ShortcutCommand;
                    self.hub.send(Command::Shortcuts(match control {
                        crate::scene::LevelControl::Volume => ShortcutCommand::SetVolume(f),
                        crate::scene::LevelControl::Brightness => ShortcutCommand::SetBrightness(f),
                    }));
                }
            }
            Action::Confirm {
                title,
                detail,
                command,
            } => {
                if super::alerts::confirm(self.mtm, &title, &detail) {
                    self.hub.send(command);
                }
            }
            Action::Collapse => self.inner.borrow_mut().model.machine.close(mono()),
            Action::Open(sec) => {
                let mut i = self.inner.borrow_mut();
                if let Some(s) = sec {
                    i.model.select_section(s);
                }
                i.model.alerts.dismiss(mono());
                i.model.machine.open();
            }
        }
        self.update();
    }

    /// Opens or closes the island (status menu, global shortcut).
    pub fn toggle_island(&self) {
        {
            let mut i = self.inner.borrow_mut();
            if i.model.machine.mode == Mode::Expanded {
                i.model.machine.close(mono());
            } else {
                i.model.machine.open();
                // Opened without the pointer: close when it leaves after entering.
            }
        }
        self.update();
    }

    // ---------- settings ----------

    pub fn save_settings(&self, s: Settings) {
        let s = Arc::new(s);
        if let Err(e) = self.store.save(&s) {
            log::error!("saving settings failed: {e}");
            super::alerts::error(
                self.mtm,
                "Impossible d’enregistrer les réglages",
                &e.to_string(),
            );
        }
        {
            let mut i = self.inner.borrow_mut();
            i.settings = s.clone();
            i.model.machine.configure(machine_config(&s));
        }
        (self.on_settings_changed)(s.clone());
        configure_panel(&self.panel, &self.view, &s);
        self.apply_hotkey();
        self.screens_changed();
    }

    fn apply_hotkey(&self) {
        let on = self.settings().island.shortcut_enabled;
        let mut hk = self.hotkey.borrow_mut();
        if on && hk.is_none() {
            *hk = super::hotkey::HotKey::register(|| {
                if let Some(a) = get() {
                    a.toggle_island();
                }
            });
        } else if !on {
            *hk = None;
        }
    }

    pub fn screens_changed(&self) {
        let settings = self.settings();
        let screen = screens::choose(self.mtm, settings.island.display);
        let Some(screen) = screen else {
            return;
        };
        let params = screens::params(&screen);
        let g = NotchGeometry::new(
            &params,
            settings.island.simulate_notch,
            settings.island.silhouette,
        );
        {
            let mut i = self.inner.borrow_mut();
            i.model.geometry = g;
            i.screen_id = screens::display_id(&screen);
            // Snap to the new geometry without animating across displays.
            let surface = i
                .model
                .decide(&i.state, &i.settings, nox_core::model::now());
            let size = i.model.surface_size(&surface, &i.settings);
            let open = surface == Surface::Expanded;
            i.model.surface = surface;
            i.model.motion.snap(size.w, size.h, open);
            i.anim = None;
        }
        self.apply_frame(true);
        self.update_fullscreen();
        self.update();
    }

    pub fn update_fullscreen(&self) {
        let (enabled, id) = {
            let i = self.inner.borrow();
            (i.settings.island.hide_in_fullscreen, i.screen_id)
        };
        let covered =
            enabled && id.is_some_and(|d| screens::frontmost_covers(d, std::process::id() as i32));
        let changed = {
            let mut i = self.inner.borrow_mut();
            let c = i.hidden_fullscreen != covered;
            i.hidden_fullscreen = covered;
            c
        };
        if changed {
            if covered {
                self.panel.orderOut(None);
            } else {
                self.panel.orderFrontRegardless();
            }
            self.update();
        }
    }

    /// Opens the selection overlay when the watch module is ready for it, and
    /// keeps the island out of its captures while a region is watched.
    fn sync_watch(&self) {
        use nox_core::model::watch::WatchPhase;
        let (phase, denied, pending) = {
            let i = self.inner.borrow();
            let w = i.state.watch.as_ref();
            (
                w.map(|w| w.phase),
                w.is_some_and(|w| w.error.is_some() && w.phase == WatchPhase::Idle),
                i.pending_selection,
            )
        };
        if let Some(trigger) = pending {
            if phase == Some(WatchPhase::Selecting) && self.overlay.borrow().is_none() {
                self.inner.borrow_mut().pending_selection = None;
                super::overlay::begin(self, trigger);
            } else if denied || phase.is_none() {
                // Refused (permission) or module stopped: the page explains.
                self.inner.borrow_mut().pending_selection = None;
            }
        }
        let watching = matches!(
            phase,
            Some(WatchPhase::Watching | WatchPhase::Changing | WatchPhase::Selecting)
        );
        self.set_capture_exclusion(watching);
    }

    /// Excludes the island from captures while a region is watched.
    pub fn set_capture_exclusion(&self, watching: bool) {
        let hide = watching || self.settings().island.hide_in_captures;
        self.panel.setSharingType(if hide {
            NSWindowSharingType::None
        } else {
            NSWindowSharingType::ReadOnly
        });
    }

    pub fn quit(&self) {
        if let Some(f) = self.on_quit.borrow_mut().take() {
            f();
        }
        *self.hotkey.borrow_mut() = None;
        self.observers.borrow_mut().clear();
        let app = objc2_app_kit::NSApplication::sharedApplication(self.mtm);
        // SAFETY: terminate: with a nil sender.
        let _: () =
            unsafe { msg_send![&*app, terminate: std::ptr::null::<objc2::runtime::AnyObject>()] };
    }

    pub fn state(&self) -> IslandState {
        self.inner.borrow().state.clone()
    }
}

fn configure_panel(panel: &IslandPanel, view: &IslandView, s: &Settings) {
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    // Above the menu bar and status items, below native menus.
    panel.setLevel(objc2_app_kit::NSStatusWindowLevel + 1);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    panel.setHidesOnDeactivate(false);
    // SAFETY: the panel is owned by the controller; it must not free itself.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setMovable(false);
    panel.setSharingType(if s.island.hide_in_captures {
        NSWindowSharingType::None
    } else {
        NSWindowSharingType::ReadOnly
    });
    if let Some(dark) = objc2_app_kit::NSAppearance::appearanceNamed(
        // SAFETY: static appearance name provided by AppKit.
        unsafe { objc2_app_kit::NSAppearanceNameDarkAqua },
    ) {
        panel.setAppearance(Some(&dark));
    }
    let view_ptr: *const objc2_app_kit::NSView = &**view;
    if !panel
        .contentView()
        .is_some_and(|v| std::ptr::eq(&*v as *const objc2_app_kit::NSView, view_ptr))
    {
        panel.setContentView(Some(view));
        // SAFETY: NSPasteboardTypeFileURL is a static pasteboard type.
        let types = NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]);
        view.registerForDraggedTypes(&types);
        view.setWantsLayer(true);
    }
}
