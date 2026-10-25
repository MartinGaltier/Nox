//! Main-thread controller: owns the island model, the panel, timers and the
//! auxiliary windows. Woken by the hub (coalesced); timers run only while
//! something on screen changes (animation, countdown, hover delay, alert TTL).

use super::island_window::{HotZoneView, IslandPanel, IslandView};
use super::painter::Painter;
use super::util::{ActionTarget, Observer, Timer, mono, ns_rect};
use super::{files, haptics, screens};
use crate::UiContext;
use crate::geom::{Point, Rect, Size};
use crate::geometry::{NotchGeometry, ignores_clicks, island_contains};
use crate::gesture::{GestureAction, ScrollInput, movement};
use crate::island::{IslandModel, Surface, machine_config};
use crate::machine::Mode;
use crate::scene::{Action, Scene};
use block2::RcBlock;
use nox_core::model::files::FileCommand;
use nox_core::model::music::MusicCommand;
use nox_core::model::watch::WatchCommand;
use nox_core::{Command, Hub, IslandState, Section, Settings, SettingsStore};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, msg_send};
use objc2_app_kit::{
    NSAppearanceCustomization, NSApplicationDidChangeScreenParametersNotification, NSColor,
    NSEvent, NSEventMask, NSEventPhase, NSPasteboardTypeFileURL, NSView,
    NSWindowCollectionBehavior, NSWindowSharingType, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceDidActivateApplicationNotification,
};
use objc2_core_graphics::CGContext;
use objc2_foundation::{NSArray, NSNotificationCenter, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

/// Room around the largest island in the fixed window: shadow on each side,
/// and below the island.
const SHADOW_X: f64 = 24.0;
const SHADOW_BOTTOM: f64 = 32.0;

/// Widest compact activity wing (the strip's wings), and the margin added
/// around the compact shape for the hot zone.
const HOT_WING: f64 = 64.0;
const HOT_MARGIN: f64 = 6.0;

/// Hot zone: a small window under the compact island, above the menu bar,
/// that always takes the pointer. The island window itself ignores clicks
/// outside its shape, so this is what the pointer meets to re-enter it.
fn hot_zone_frame(geometry: &NotchGeometry) -> Rect {
    geometry.hot_zone(HOT_WING, HOT_MARGIN)
}

/// Size of the fixed panel: the largest island the settings allow, plus room
/// for its shadow. Recomputed only when the screen or the settings change.
fn window_size(geometry: &NotchGeometry, settings: &Settings) -> Size {
    let e = geometry.expanded_size(&settings.island);
    Size::new(e.w.ceil() + 2.0 * SHADOW_X, e.h.ceil() + SHADOW_BOTTOM)
}

fn union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect::new(
        x,
        y,
        a.max_x().max(b.max_x()) - x,
        a.max_y().max(b.max_y()) - y,
    )
}

/// Display-synchronised clock for the animation: a `CADisplayLink` from the
/// view, so it follows the view's screen and refresh rate. It runs only while
/// something animates and is invalidated when dropped.
struct FrameClock {
    link: Retained<CADisplayLink>,
    _target: Retained<ActionTarget>,
}

impl FrameClock {
    fn start(view: &NSView) -> Self {
        let mtm = MainThreadMarker::new().expect("the animation clock runs on the main thread");
        let target = ActionTarget::new(mtm, |_: &AnyObject| {
            if let Some(a) = get() {
                a.tick_animation();
            }
        });
        // SAFETY: `ActionTarget::selector()` is `fire:`, which takes the link
        // as its only argument, and the link retains the target.
        let link =
            unsafe { view.displayLinkWithTarget_selector(&target, ActionTarget::selector()) };
        // SAFETY: the main run loop and its common modes are static Foundation values.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Self {
            link,
            _target: target,
        }
    }
}

impl Drop for FrameClock {
    fn drop(&mut self) {
        self.link.invalidate();
    }
}

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
    /// Left edge of the shape inside the view (the window is fixed, see `fit_window`).
    offset: f64,
    /// The fixed window frame, global AppKit coordinates.
    frame: Rect,
    /// Shape rectangle last drawn, in view coordinates: the dirty region.
    drawn: Option<Rect>,
    anim: Option<FrameClock>,
    last_tick: f64,
    deadline: Option<Timer>,
    refresh: Option<Timer>,
    metrics_sent: Option<bool>,
    camera_on: bool,
    hidden_fullscreen: bool,
    screen_id: Option<u32>,
    artwork: HashMap<u64, Arc<Vec<u8>>>,
    seek_drag: Option<(f64, f64, f64)>,
    page_shown: Option<Section>,
    music_visible: bool,
    mixer_visible: bool,
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
    hot_panel: Retained<IslandPanel>,
    hot_view: Retained<HotZoneView>,
    observers: RefCell<Vec<Observer>>,
    pub status: RefCell<Option<super::status::StatusBar>>,
    pub settings_window: RefCell<Option<super::settings_window::SettingsWindow>>,
    pub monitor_window: RefCell<Option<super::monitor_window::MonitorWindow>>,
    pub overlay: RefCell<Option<super::overlay::SelectionOverlay>>,
    hotkey: RefCell<Option<super::hotkey::HotKey>>,
    pointer_monitor: RefCell<Option<Retained<AnyObject>>>,
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
        let frame = geometry.window_frame(window_size(&geometry, &settings));
        let hot = hot_zone_frame(&geometry);
        let offset = geometry.shape_x(size.w) - frame.x;
        let panel = IslandPanel::new(mtm, ns_rect(frame));
        let view = IslandView::new(mtm, ns_rect(Rect::new(0.0, 0.0, frame.w, frame.h)));
        configure_panel(&panel, &view, &settings);
        // Clicks pass through until the pointer is over the shape.
        panel.setIgnoresMouseEvents(true);
        let hot_panel = IslandPanel::new(mtm, ns_rect(hot));
        let hot_view = HotZoneView::new(mtm, ns_rect(Rect::new(0.0, 0.0, hot.w, hot.h)));
        configure_hot_panel(&hot_panel, &hot_view, &settings);

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
                offset,
                frame,
                drawn: None,
                anim: None,
                last_tick: 0.0,
                deadline: None,
                refresh: None,
                metrics_sent: None,
                camera_on: false,
                hidden_fullscreen: false,
                screen_id: screen.as_deref().and_then(screens::display_id),
                artwork: HashMap::new(),
                seek_drag: None,
                page_shown: None,
                music_visible: false,
                mixer_visible: false,
                pending_selection: None,
                damage: None,
                dragging_files: false,
            }),
            panel,
            view,
            hot_panel,
            hot_view,
            observers: RefCell::new(vec![]),
            status: RefCell::new(None),
            settings_window: RefCell::new(None),
            monitor_window: RefCell::new(None),
            overlay: RefCell::new(None),
            hotkey: RefCell::new(None),
            pointer_monitor: RefCell::new(None),
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
        self.hot_panel.orderFrontRegardless();
        self.panel.orderFrontRegardless();
        self.on_wake();
        self.update_fullscreen();
        #[cfg(debug_assertions)]
        super::snapshot::schedule(self);
        #[cfg(any(debug_assertions, feature = "hover-probe"))]
        super::hover_probe::schedule(self);
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
            {
                let paths: Vec<String> = snapshot
                    .captures
                    .iter()
                    .flat_map(|c| c.items.iter().map(|it| it.path.clone()))
                    .collect();
                let keep: Vec<&str> = paths.iter().map(String::as_str).collect();
                i.painter.retain_thumbnails(&keep);
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
        let open = self.inner.borrow().model.machine.mode == Mode::Expanded;
        self.set_pointer_monitor(open);
        if animating {
            self.start_animation();
        } else {
            self.apply_frame();
        }
        self.relayout();
        self.schedule_deadline();
    }

    fn start_animation(&self) {
        let idle = self.inner.borrow().anim.is_none();
        if idle {
            let clock = FrameClock::start(&self.view);
            let mut i = self.inner.borrow_mut();
            i.last_tick = mono();
            i.anim = Some(clock);
        }
        self.apply_frame();
    }

    fn tick_animation(&self) {
        let _p = crate::perf::Probe::new("tick");
        let settled = {
            let mut i = self.inner.borrow_mut();
            let t = mono();
            let dt = (t - i.last_tick).clamp(0.0, 0.05);
            i.last_tick = t;
            i.model.motion.step(dt);
            i.model.motion.settled()
        };
        if settled {
            // Dropping the clock invalidates the display link.
            self.inner.borrow_mut().anim = None;
        }
        self.apply_frame();
        if settled {
            self.panel.invalidateShadow();
            self.recheck_pointer();
        }
        self.relayout();
    }

    /// Places the shape for the size drawn now: its centre stays on the camera
    /// at every frame, inside the fixed window (no window resize while animating).
    fn apply_frame(&self) {
        let expanded = {
            let mut i = self.inner.borrow_mut();
            let size = i.model.size();
            i.offset = i.model.geometry.shape_x(size.w) - i.frame.x;
            i.model.motion.open.value > 0.5
        };
        self.panel.setHasShadow(expanded);
    }

    /// Sizes the fixed window for the current screen and settings. Called only
    /// when either changes, never per frame.
    fn fit_window(&self) {
        let hot = hot_zone_frame(&self.inner.borrow().model.geometry);
        self.hot_panel.setFrame_display(ns_rect(hot), false);
        self.hot_view
            .setFrame(ns_rect(Rect::new(0.0, 0.0, hot.w, hot.h)));
        let frame = {
            let i = self.inner.borrow();
            i.model
                .geometry
                .window_frame(window_size(&i.model.geometry, &i.settings))
        };
        if frame == self.inner.borrow().frame {
            return;
        }
        self.inner.borrow_mut().frame = frame;
        self.panel.setFrame_display(ns_rect(frame), false);
        self.view
            .setFrame(ns_rect(Rect::new(0.0, 0.0, frame.w, frame.h)));
        self.inner.borrow_mut().drawn = None;
    }

    fn relayout(&self) {
        let (shows_metrics, shows_camera, refresh, extent, section) = {
            let mut i = self.inner.borrow_mut();
            let hover = i.hover.map(|p| Point::new(p.x - i.offset, p.y));
            let scene = i
                .model
                .scene(&i.state, &i.settings, nox_core::model::now(), hover);
            {
                // Covers, symbols and text attributes are made here, before a
                // transition needs them, not in its first draw.
                let Inner {
                    painter, artwork, ..
                } = &mut *i;
                let art = |id: u64| artwork.get(&id).cloned();
                painter.prepare(&scene, &art);
            }
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
        // Music page visibility (the module may sample levels only then).
        let music_now = shown == Some(Section::Music);
        let music_changed = {
            let mut i = self.inner.borrow_mut();
            let c = i.music_visible != music_now;
            i.music_visible = music_now;
            c
        };
        if music_changed {
            use nox_core::model::music::MusicCommand;
            self.hub
                .send(Command::Music(MusicCommand::Visible(music_now)));
        }
        // Mixer page visibility (taps only exist for apps set below 100 %).
        let mixer_now = shown == Some(Section::Mixer);
        let mixer_changed = {
            let mut i = self.inner.borrow_mut();
            let c = i.mixer_visible != mixer_now;
            i.mixer_visible = mixer_now;
            c
        };
        if mixer_changed {
            use nox_core::model::mixer::MixerCommand;
            self.hub
                .send(Command::Mixer(MixerCommand::Visible(mixer_now)));
        }
        match appeared {
            Some(Section::Shortcuts) => {
                use nox_core::model::shortcuts::ShortcutCommand;
                self.hub
                    .send(Command::Shortcuts(ShortcutCommand::RefreshControls));
            }
            Some(Section::Captures) => {
                use nox_core::model::captures::CaptureCommand;
                self.hub.send(Command::Captures(CaptureCommand::Refresh));
            }
            _ => {}
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
        let editor_rect = i.scene.ops.iter().find_map(|o| match o {
            crate::scene::Op::TextEditor { rect } => Some(rect.offset(i.offset, 0.0)),
            _ => None,
        });
        let initial = i
            .state
            .scratchpad
            .as_ref()
            .map(|p| p.text.clone())
            .unwrap_or_default();
        let dmg = i.damage.take();
        let (offset, animating) = (i.offset, i.anim.is_some());
        // While animating, only the shape's old and new rectangles change.
        let size = i.model.size();
        let shape = Rect::new(offset, 0.0, size.w, size.h);
        let dirty = match i.drawn.replace(shape) {
            Some(prev) if animating => union(prev, shape),
            _ => shape,
        };
        drop(i);
        super::editor::sync(&self.view, editor_rect, || initial);
        // Unchanged scene: nothing to draw; a small change (equalizer bars,
        // countdown digits): only that rectangle.
        match dmg {
            _ if animating => self.view.setNeedsDisplayInRect(ns_rect(Rect::new(
                dirty.x - 4.0,
                dirty.y - 4.0,
                dirty.w + 8.0,
                dirty.h + 8.0,
            ))),
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
        let _p = crate::perf::Probe::new("draw");
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

    /// Probe: the pointer enters the middle of the island or leaves it.
    #[cfg(any(debug_assertions, feature = "hover-probe"))]
    pub fn debug_hover(&self, inside: bool) {
        let p = inside.then(|| {
            let f = super::util::rect(self.panel.frame());
            Point::new(f.w / 2.0, 10.0)
        });
        self.pointer_moved(p);
    }

    pub fn pointer_moved(&self, p: Option<Point>) {
        let inside = p.is_some_and(|p| self.shape_contains(p));
        // Outside the shape the window takes no clicks, so the menu bar beside
        // the island stays usable.
        if self.panel.ignoresMouseEvents() == inside {
            self.panel.setIgnoresMouseEvents(ignores_clicks(inside));
        }
        let redraw = {
            let mut i = self.inner.borrow_mut();
            let had_hover = i.hover.is_some();
            i.hover = p.filter(|_| inside);
            let before = i.model.machine.mode;
            i.model.machine.pointer(inside, mono());
            // Hover highlights need a redraw only while the pointer is on the page.
            i.model.machine.mode != before
                || (inside && i.model.machine.mode == Mode::Expanded)
                || (had_hover && !inside)
        };
        if redraw {
            self.update();
        } else {
            self.schedule_deadline();
        }
    }

    /// Pointer in the island's view coordinates, if it is inside the window.
    fn pointer_in_panel(&self) -> Option<Point> {
        let m = NSEvent::mouseLocation();
        let f = super::util::rect(self.panel.frame());
        let p = Point::new(m.x - f.x, f.max_y() - m.y);
        let inside_window = p.x >= 0.0 && p.y >= 0.0 && p.x <= f.w && p.y <= f.h;
        inside_window.then_some(p)
    }

    fn recheck_pointer(&self) {
        self.pointer_moved(self.pointer_in_panel());
    }

    /// The hot zone was entered, left or moved over.
    pub fn hot_zone_event(&self) {
        self.recheck_pointer();
    }

    /// A click on the hot zone (the island ignores clicks there): the island
    /// takes the pointer and handles the click as if it had received it.
    pub fn hot_zone_click(&self) {
        let Some(p) = self.pointer_in_panel() else {
            return;
        };
        self.pointer_moved(Some(p));
        if self.shape_contains(p) {
            self.mouse_down(p);
        }
    }

    /// Global mouse movement is watched only while the island is open: it lets
    /// the pointer come back into an open shape the window ignores (mouse moves
    /// need no Accessibility permission). Nothing is watched at rest.
    fn set_pointer_monitor(&self, on: bool) {
        let mut monitor = self.pointer_monitor.borrow_mut();
        if on && monitor.is_none() {
            let block = RcBlock::new(|_e: NonNull<NSEvent>| {
                if let Some(a) = get() {
                    a.recheck_pointer();
                }
            });
            let mask = NSEventMask::MouseMoved | NSEventMask::LeftMouseDragged;
            *monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &block);
        } else if !on && let Some(old) = monitor.take() {
            // SAFETY: the token was returned by addGlobalMonitorForEvents and is removed once.
            unsafe { NSEvent::removeMonitor(&old) };
        }
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
            Action::Send(Command::Mixer(cmd)) => self.send_mixer(cmd),
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
            Action::SetAppVolume { app, x, w } => {
                if let Some(p) = click {
                    let off = self.inner.borrow().offset;
                    let f = ((p.x - off - x) / w.max(1.0)).clamp(0.0, 1.0);
                    // 0–200 % in steps of 1 %.
                    let volume = (f * 200.0).round() as f32 / 100.0;
                    use nox_core::model::mixer::MixerCommand;
                    self.send_mixer(MixerCommand::SetVolume { app, volume });
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
            Action::PreferPlayer(bundle) => {
                let mut s = (*self.settings()).clone();
                s.music.preferred_player = Some(bundle);
                self.save_settings(s);
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

    /// Sends a mixer command and keeps `settings.mixer` in step, so a volume
    /// is applied again at the next start.
    fn send_mixer(&self, cmd: nox_core::model::mixer::MixerCommand) {
        let mut s = (*self.settings()).clone();
        let apps = self
            .inner
            .borrow()
            .state
            .mixer
            .as_ref()
            .map(|m| m.apps.clone())
            .unwrap_or_default();
        if crate::views::mixer::remember(&mut s.mixer, &apps, &cmd) {
            self.save_settings(s);
        }
        self.hub.send(Command::Mixer(cmd));
    }

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
        self.fit_window();
        self.apply_frame();
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
                self.hot_panel.orderOut(None);
            } else {
                self.hot_panel.orderFrontRegardless();
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

/// The hot zone window: same level and spaces as the island, never shadowed.
fn configure_hot_panel(panel: &IslandPanel, view: &HotZoneView, s: &Settings) {
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
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
    panel.setContentView(Some(view));
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
