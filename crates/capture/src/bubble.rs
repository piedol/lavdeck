//! A draggable camera preview that floats above other windows while recording. Double-click cycles its
//! size: a round bubble, a large 16:9 view of the whole frame, and full screen (the screen's usable area,
//! so the menu bar stays reachable). The preview is mirrored like a mirror; the recorded file is not. The
//! screen recorder leaves this window out, so it never ends up in the screen recording at any size.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::time::Duration;

use block2::StackBlock;
use dispatch2::MainThreadBound;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSBackingStoreType, NSColor, NSEvent,
    NSFloatingWindowLevel, NSFont, NSFontWeightSemibold, NSLayoutConstraint, NSPanel, NSScreen,
    NSTextAlignment, NSTextField, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_av_foundation::{
    AVCaptureSession, AVCaptureVideoPreviewLayer, AVLayerVideoGravityResizeAspect,
    AVLayerVideoGravityResizeAspectFill,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSPointFromString, NSString};
use objc2_quartz_core::CAAutoresizingMask;

use crate::layout::{self, PREVIEW_DIAMETER, Rect};
use crate::settings::PreviewSize;
use crate::util::{main_after, ns};

/// Where the bubble was last left, as `NSStringFromPoint`, in Lavdeck's settings domain.
const POSITION_KEY: &str = "bubbleOrigin";

fn cg_rect(r: Rect) -> CGRect {
    CGRect::new(CGPoint::new(r.x, r.y), CGSize::new(r.w, r.h))
}

struct ViewIvars {
    preview: Retained<AVCaptureVideoPreviewLayer>,
    hint: Retained<NSTextField>,
    on_double_click: RefCell<Option<Box<dyn Fn()>>>,
    /// Bumped by each hint, so an older hint's fade-out leaves a newer one alone.
    hint_generation: Cell<u64>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements we break, and PreviewView has no Drop impl.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckCameraPreviewView"]
    #[ivars = ViewIvars]
    struct PreviewView;

    impl PreviewView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if event.clickCount() == 2 {
                if let Some(f) = &*self.ivars().on_double_click.borrow() {
                    f();
                }
            } else if let Some(window) = self.window() {
                window.performWindowDragWithEvent(event);
            }
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(layout))]
        fn layout(&self) {
            unsafe { msg_send![super(self), layout] }
            if let Some(layer) = self.layer()
                && layer.borderWidth() == 3.0
            {
                layer.setCornerRadius(self.bounds().size.width / 2.0);
            }
        }
    }
);

/// The preview layer plus a short-lived hint label. Drag moves the panel; double-click reports up.
impl PreviewView {
    fn new(mtm: MainThreadMarker, session: &AVCaptureSession) -> Retained<Self> {
        let frame = cg_rect(Rect::new(0.0, 0.0, PREVIEW_DIAMETER, PREVIEW_DIAMETER));
        let preview = unsafe { AVCaptureVideoPreviewLayer::layerWithSession(session) };
        let hint = NSTextField::labelWithString(&ns(""), mtm);
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            preview: preview.clone(),
            hint: hint.clone(),
            on_double_click: RefCell::new(None),
            hint_generation: Cell::new(0),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setWantsLayer(true);
        if let Some(layer) = this.layer() {
            layer.setMasksToBounds(true);
            layer.setBackgroundColor(Some(&NSColor::blackColor().CGColor()));
            layer.setBorderColor(Some(
                &NSColor::whiteColor().colorWithAlphaComponent(0.9).CGColor(),
            ));
            preview.setFrame(this.bounds());
            preview.setAutoresizingMask(
                CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
            );
            unsafe {
                if let Some(connection) = preview.connection() {
                    connection.setAutomaticallyAdjustsVideoMirroring(false);
                    connection.setVideoMirrored(true);
                }
            }
            layer.addSublayer(&preview);
        }

        hint.setFont(Some(&NSFont::systemFontOfSize_weight(11.0, unsafe {
            NSFontWeightSemibold
        })));
        hint.setTextColor(Some(&NSColor::whiteColor()));
        hint.setAlignment(NSTextAlignment::Center);
        hint.setWantsLayer(true);
        if let Some(layer) = hint.layer() {
            layer.setBackgroundColor(Some(
                &NSColor::blackColor()
                    .colorWithAlphaComponent(0.55)
                    .CGColor(),
            ));
            layer.setCornerRadius(8.0);
        }
        hint.setAlphaValue(0.0);
        hint.setTranslatesAutoresizingMaskIntoConstraints(false);
        this.addSubview(&hint);
        let constraints = [
            hint.centerXAnchor()
                .constraintEqualToAnchor(&this.centerXAnchor()),
            hint.bottomAnchor()
                .constraintEqualToAnchor_constant(&this.bottomAnchor(), -22.0),
            hint.widthAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&this.widthAnchor(), -24.0),
        ];
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&constraints));
        this
    }

    /// Round with aspect-fill for the bubble; the whole frame (letterboxed) when large or full screen.
    fn style(&self, round: bool, fill: bool, corner_radius: f64) {
        if let Some(layer) = self.layer() {
            layer.setCornerRadius(if round {
                self.bounds().size.width / 2.0
            } else {
                corner_radius
            });
            layer.setBorderWidth(if round { 3.0 } else { 0.0 });
        }
        let gravity = unsafe {
            if fill {
                AVLayerVideoGravityResizeAspectFill
            } else {
                AVLayerVideoGravityResizeAspect
            }
        };
        if let Some(gravity) = gravity {
            unsafe { self.ivars().preview.setVideoGravity(gravity) };
        }
    }

    fn flash_hint(&self, text: &str) {
        let ivars = self.ivars();
        ivars.hint.setStringValue(&ns(&format!("  {text}  ")));
        let generation = ivars.hint_generation.get() + 1;
        ivars.hint_generation.set(generation);
        fade(&ivars.hint, 0.2, 1.0);
        let mtm = self.mtm();
        let view = MainThreadBound::new(self.retain(), mtm);
        main_after(Duration::from_secs(3), move || {
            let mtm = MainThreadMarker::new().expect("main queue");
            let view = view.get(mtm);
            if view.ivars().hint_generation.get() == generation {
                fade(&view.ivars().hint, 0.4, 0.0);
            }
        });
    }
}

fn fade(view: &NSTextField, duration: f64, alpha: f64) {
    let changes = StackBlock::new(|context: NonNull<NSAnimationContext>| {
        unsafe { context.as_ref() }.setDuration(duration);
        view.animator().setAlphaValue(alpha);
    });
    NSAnimationContext::runAnimationGroup(&changes);
}

struct Inner {
    panel: Retained<NSPanel>,
    surface: Retained<PreviewView>,
    size: Cell<PreviewSize>,
    /// Words a hint (the app's `tr`).
    translate: fn(&str) -> String,
}

/// The floating camera preview. Main thread only.
pub struct CameraBubble {
    inner: Rc<Inner>,
}

impl CameraBubble {
    /// A preview of `session` at `size`; `translate` words its hints (the app's `tr`).
    pub fn new(
        mtm: MainThreadMarker,
        session: &AVCaptureSession,
        size: PreviewSize,
        translate: fn(&str) -> String,
    ) -> Self {
        let d = PREVIEW_DIAMETER;
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            cg_rect(Rect::new(0.0, 0.0, d, d)),
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setLevel(NSFloatingWindowLevel);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setHidesOnDeactivate(false);
        let surface = PreviewView::new(mtm, session);
        panel.setContentView(Some(&surface));
        let inner = Rc::new(Inner {
            panel,
            surface,
            size: Cell::new(size),
            translate,
        });
        let weak: Weak<Inner> = Rc::downgrade(&inner);
        *inner.surface.ivars().on_double_click.borrow_mut() = Some(Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.cycle();
            }
        }));
        inner.apply(size, false);
        CameraBubble { inner }
    }

    /// The preview's window number, for leaving it out of the screen recording.
    pub fn window_number(&self) -> isize {
        self.inner.panel.windowNumber()
    }

    pub fn show(&self) {
        self.inner.panel.orderFrontRegardless();
        self.inner.flash_hint();
    }

    pub fn close(&self) {
        let inner = &self.inner;
        if inner.size.get() == PreviewSize::Bubble {
            inner.save_position();
        }
        inner.panel.orderOut(None);
    }
}

impl Drop for CameraBubble {
    fn drop(&mut self) {
        // The view's double-click handler holds only a weak reference; drop it with the window.
        *self.inner.surface.ivars().on_double_click.borrow_mut() = None;
    }
}

impl Inner {
    fn save_position(&self) {
        let origin = self.panel.frame().origin;
        let value = NSString::from_point(origin);
        unsafe {
            lavdeck_kit::defaults::domain().setObject_forKey(Some(&value), &ns(POSITION_KEY))
        };
    }

    fn saved_position() -> Option<(f64, f64)> {
        let saved = lavdeck_kit::defaults::domain().stringForKey(&ns(POSITION_KEY))?;
        let p = NSPointFromString(&saved);
        Some((p.x, p.y))
    }

    fn flash_hint(&self) {
        self.surface
            .flash_hint(&(self.translate)(self.size.get().hint()));
    }

    fn cycle(&self) {
        if self.size.get() == PreviewSize::Bubble {
            self.save_position();
        }
        let next = self.size.get().next();
        self.size.set(next);
        self.apply(next, true);
        self.flash_hint();
    }

    fn apply(&self, size: PreviewSize, animate: bool) {
        let mtm = self.panel.mtm();
        let screen = self
            .panel
            .screen()
            .or_else(|| NSScreen::mainScreen(mtm))
            .map(|s| {
                let f = s.visibleFrame();
                Rect::new(f.origin.x, f.origin.y, f.size.width, f.size.height)
            })
            .unwrap_or(Rect::new(0.0, 0.0, 1440.0, 900.0));
        let saved = if size == PreviewSize::Bubble {
            Self::saved_position()
        } else {
            None
        };
        let frame = layout::preview_frame(size, screen, saved);
        self.panel
            .setFrame_display_animate(cg_rect(frame), true, animate);
        let bubble = size == PreviewSize::Bubble;
        self.surface.style(
            bubble,
            bubble,
            if size == PreviewSize::Full { 0.0 } else { 12.0 },
        );
    }
}
