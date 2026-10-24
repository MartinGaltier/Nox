//! Generic flipped view drawing a Scene, used by the monitor window and the
//! region selection overlay.

use super::util::rect;
use crate::geom::{Point, Rect};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSResponder, NSView};
use objc2_foundation::NSRect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mouse {
    Down(Point),
    Dragged(Point),
    Up(Point),
}

pub struct CanvasIvars {
    pub draw: Box<dyn Fn(Rect)>,
    pub mouse: Option<Box<dyn Fn(Mouse)>>,
    /// Key code handler (e.g. 53 = Escape).
    pub key: Option<Box<dyn Fn(u16)>>,
}

define_class!(
    // SAFETY: NSView subclass without Drop; ivars are main-thread closures.
    #[unsafe(super(NSView, NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NoxCanvasView"]
    #[ivars = CanvasIvars]
    pub struct CanvasView;

    impl CanvasView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            self.ivars().key.is_some()
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _r: NSRect) {
            (self.ivars().draw)(rect(self.bounds()));
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            self.mouse(e, Mouse::Down);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            self.mouse(e, Mouse::Dragged);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) {
            self.mouse(e, Mouse::Up);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            if let Some(k) = &self.ivars().key {
                k(e.keyCode());
            }
        }
    }

    unsafe impl NSObjectProtocol for CanvasView {}
);

impl CanvasView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, ivars: CanvasIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: designated NSView initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn mouse(&self, e: &NSEvent, f: fn(Point) -> Mouse) {
        let p = self.convertPoint_fromView(e.locationInWindow(), None);
        if let Some(m) = &self.ivars().mouse {
            m(f(Point::new(p.x, p.y)));
        }
    }
}
