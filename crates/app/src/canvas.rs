//! The building blocks of Lavdeck's custom surfaces.
//!
//! - [`Canvas`]: a flipped view that draws with a closure and calls closures on click, drag,
//!   hover and right-click, and can drag files out to other apps.
//! - [`Layer`]: a flat set of keyed subviews, rebuilt from the view model like SwiftUI's
//!   body. Views keep their identity between rebuilds (a meter stays the same view, so it can
//!   animate; a text field keeps focus), and views no longer built are removed.
//! - [`on_action`] and [`menu_item`]: closures as AppKit target-action.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSControl, NSControlTextEditingDelegate,
    NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource,
    NSEvent, NSEventModifierFlags, NSMenu, NSMenuItem, NSTextFieldDelegate, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWorkspace,
};
use objc2_foundation::{NSArray, NSPoint, NSRect};

use crate::draw::rect;
use crate::util::{self, ns};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragPhase {
    Began,
    Moved,
    Ended,
}

type Draw = Rc<dyn Fn(NSRect)>;
type Action = Rc<dyn Fn()>;
type Drag = Rc<dyn Fn(DragPhase, NSPoint)>;
type MenuMaker = Rc<dyn Fn() -> Option<Retained<NSMenu>>>;
type Hover = Rc<dyn Fn(bool)>;

#[derive(Default)]
pub struct CanvasIvars {
    draw: RefCell<Option<Draw>>,
    click: RefCell<Option<Action>>,
    drag: RefCell<Option<Drag>>,
    menu: RefCell<Option<MenuMaker>>,
    hover: RefCell<Option<Hover>>,
    resize: RefCell<Option<Action>>,
    drag_out: RefCell<Vec<PathBuf>>,
    down_at: Cell<Option<NSPoint>>,
    pressed: Cell<bool>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    // SAFETY: NSView subclass that draws with a closure; no Drop impl beyond the ivars.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckCanvas"]
    #[ivars = CanvasIvars]
    pub struct Canvas;

    unsafe impl NSObjectProtocol for Canvas {}

    unsafe impl NSDraggingSource for Canvas {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_mask(&self, _s: &NSDraggingSession, _c: NSDraggingContext) -> NSDragOperation {
            NSDragOperation::Copy
        }
    }

    impl Canvas {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: objc2_foundation::NSSize) {
            let old = self.frame().size;
            // SAFETY: plain call to the superclass implementation.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            if old != size
                && let Some(f) = self.ivars().resize.borrow().clone()
            {
                f();
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let draw = self.ivars().draw.borrow().clone();
            if let Some(draw) = draw {
                let b = self.bounds();
                if self.ivars().pressed.get() {
                    crate::draw::with_alpha(0.7, || draw(b));
                } else {
                    draw(b);
                }
            }
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            // SAFETY: plain call to the superclass implementation.
            let hit: *mut NSView = unsafe { msg_send![super(self), hitTest: point] };
            // A decorative canvas lets clicks through to what's under it; its subviews still
            // get theirs.
            let iv = self.ivars();
            let interactive = iv.click.borrow().is_some()
                || iv.drag.borrow().is_some()
                || iv.menu.borrow().is_some()
                || iv.hover.borrow().is_some()
                || !iv.drag_out.borrow().is_empty()
                || self.toolTip().is_some();
            if std::ptr::eq(hit, self as *const Canvas as *const NSView) && !interactive {
                return std::ptr::null_mut();
            }
            hit
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let p = self.local(event);
            let iv = self.ivars();
            iv.down_at.set(Some(p));
            if let Some(drag) = iv.drag.borrow().clone() {
                drag(DragPhase::Began, p);
            } else if iv.click.borrow().is_some() {
                iv.pressed.set(true);
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let p = self.local(event);
            let iv = self.ivars();
            if let Some(drag) = iv.drag.borrow().clone() {
                drag(DragPhase::Moved, p);
                return;
            }
            let files = iv.drag_out.borrow().clone();
            if let Some(start) = iv.down_at.get()
                && !files.is_empty()
                && ((p.x - start.x).powi(2) + (p.y - start.y).powi(2)).sqrt() > 3.0
            {
                iv.down_at.set(None);
                iv.pressed.set(false);
                self.drag_files(&files, event, start);
                return;
            }
            let inside = self.contains(p);
            if iv.click.borrow().is_some() && iv.pressed.get() != inside {
                iv.pressed.set(inside);
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let p = self.local(event);
            let iv = self.ivars();
            let was_down = iv.down_at.take().is_some();
            if let Some(drag) = iv.drag.borrow().clone() {
                drag(DragPhase::Ended, p);
                return;
            }
            if iv.pressed.replace(false) {
                self.setNeedsDisplay(true);
            }
            if was_down
                && self.contains(p)
                && let Some(click) = iv.click.borrow().clone()
            {
                click();
            }
        }

        #[unsafe(method(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> *mut NSMenu {
            let maker = self.ivars().menu.borrow().clone();
            match maker.and_then(|m| m()) {
                Some(menu) => Retained::autorelease_return(menu),
                None => std::ptr::null_mut(),
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            if let Some(old) = self.ivars().tracking.borrow_mut().take() {
                self.removeTrackingArea(&old);
            }
            if self.ivars().hover.borrow().is_some() {
                // SAFETY: the view owns the area and is its owner for its lifetime.
                let area = unsafe {
                    NSTrackingArea::initWithRect_options_owner_userInfo(
                        NSTrackingArea::alloc(),
                        NSRect::ZERO,
                        NSTrackingAreaOptions::MouseEnteredAndExited
                            | NSTrackingAreaOptions::ActiveAlways
                            | NSTrackingAreaOptions::InVisibleRect,
                        Some(self),
                        None,
                    )
                };
                self.addTrackingArea(&area);
                *self.ivars().tracking.borrow_mut() = Some(area);
            }
            // SAFETY: plain call to the superclass implementation.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _e: &NSEvent) {
            if let Some(h) = self.ivars().hover.borrow().clone() {
                h(true);
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _e: &NSEvent) {
            if let Some(h) = self.ivars().hover.borrow().clone() {
                h(false);
            }
        }
    }
);

impl Canvas {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Canvas> {
        let this = Canvas::alloc(mtm).set_ivars(CanvasIvars::default());
        // SAFETY: NSView's designated initializer.
        let c: Retained<Canvas> = unsafe { msg_send![super(this), initWithFrame: frame] };
        // Views don't clip their drawing by default since macOS 14; parts here rely on it.
        c.setClipsToBounds(true);
        c
    }

    fn local(&self, event: &NSEvent) -> NSPoint {
        self.convertPoint_fromView(event.locationInWindow(), None)
    }

    fn contains(&self, p: NSPoint) -> bool {
        let b = self.bounds();
        p.x >= 0.0 && p.y >= 0.0 && p.x <= b.size.width && p.y <= b.size.height
    }

    /// Forgets every closure and the tooltip, for reuse in a rebuild.
    pub fn reset(&self) {
        let iv = self.ivars();
        iv.click.replace(None);
        iv.drag.replace(None);
        iv.menu.replace(None);
        let had_hover = iv.hover.replace(None).is_some();
        iv.drag_out.replace(Vec::new());
        self.setToolTip(None);
        if had_hover {
            self.updateTrackingAreas();
        }
    }

    pub fn on_draw(&self, f: impl Fn(NSRect) + 'static) -> &Self {
        self.ivars().draw.replace(Some(Rc::new(f)));
        self.setNeedsDisplay(true);
        self
    }

    /// Calls `f` after the view changes size (not reset by a rebuild).
    pub fn on_resize(&self, f: impl Fn() + 'static) -> &Self {
        self.ivars().resize.replace(Some(Rc::new(f)));
        self
    }

    pub fn on_click(&self, f: impl Fn() + 'static) -> &Self {
        self.ivars().click.replace(Some(Rc::new(f)));
        self
    }

    pub fn on_drag(&self, f: impl Fn(DragPhase, NSPoint) + 'static) -> &Self {
        self.ivars().drag.replace(Some(Rc::new(f)));
        self
    }

    pub fn on_menu(&self, f: impl Fn() -> Option<Retained<NSMenu>> + 'static) -> &Self {
        self.ivars().menu.replace(Some(Rc::new(f)));
        self
    }

    pub fn on_hover(&self, f: impl Fn(bool) + 'static) -> &Self {
        let had = self.ivars().hover.replace(Some(Rc::new(f))).is_some();
        if !had {
            self.updateTrackingAreas();
        }
        self
    }

    /// Dragging the view drags these files out (the first one's icon shows).
    pub fn drag_out(&self, files: Vec<PathBuf>) -> &Self {
        self.ivars().drag_out.replace(files);
        self
    }

    pub fn tip(&self, text: &str) -> &Self {
        self.setToolTip((!text.is_empty()).then(|| ns(text)).as_deref());
        self
    }

    /// Names the view for VoiceOver, as a button when it's clickable.
    pub fn a11y(&self, label: &str, value: Option<&str>) -> &Self {
        self.setAccessibilityElement(true);
        self.setAccessibilityLabel(Some(&ns(label)));
        if let Some(v) = value {
            // SAFETY: a string is a valid accessibility value.
            unsafe { self.setAccessibilityValue(Some(&ns(v))) };
        }
        if self.ivars().click.borrow().is_some() {
            // SAFETY: AppKit's role constant.
            self.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
        }
        self
    }

    fn drag_files(&self, files: &[PathBuf], event: &NSEvent, at: NSPoint) {
        let items: Vec<Retained<NSDraggingItem>> = files
            .iter()
            .take(1)
            .map(|f| {
                let url = util::file_url(f);
                let item = NSDraggingItem::initWithPasteboardWriter(
                    NSDraggingItem::alloc(),
                    ProtocolObject::from_ref(&*url),
                );
                let icon = NSWorkspace::sharedWorkspace().iconForFile(&ns(&f.to_string_lossy()));
                // SAFETY: an NSImage is valid dragging contents.
                unsafe {
                    item.setDraggingFrame_contents(
                        rect(at.x - 16.0, at.y - 16.0, 32.0, 32.0),
                        Some(&icon),
                    )
                };
                item
            })
            .collect();
        let array = NSArray::from_retained_slice(&items);
        self.beginDraggingSessionWithItems_event_source(
            &array,
            event,
            ProtocolObject::from_ref(self),
        );
    }
}

/// A flat set of keyed views inside `root`.
pub struct Layer {
    pub root: Retained<NSView>,
    views: RefCell<HashMap<String, Retained<NSView>>>,
}

impl Layer {
    pub fn new(root: Retained<NSView>) -> Layer {
        Layer {
            root,
            views: RefCell::new(HashMap::new()),
        }
    }

    /// Starts a rebuild; views not built again are removed by [`Build::finish`].
    pub fn build(&self) -> Build<'_> {
        Build {
            layer: self,
            used: HashSet::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<Retained<NSView>> {
        self.views.borrow().get(key).cloned()
    }
}

pub struct Build<'a> {
    layer: &'a Layer,
    used: HashSet<String>,
}

impl Build<'_> {
    /// The canvas for `key` at `frame`, reused when it exists, with its closures cleared.
    pub fn canvas(&mut self, key: &str, frame: NSRect) -> Retained<Canvas> {
        let mtm = util::mtm();
        self.used.insert(key.to_string());
        let existing = self.layer.views.borrow().get(key).cloned();
        if let Some(v) = existing
            && let Ok(c) = v.downcast::<Canvas>()
        {
            c.reset();
            if c.frame() != frame {
                c.setFrame(frame);
                c.setNeedsDisplay(true);
            }
            return c;
        }
        let c = Canvas::new(mtm, frame);
        self.layer.root.addSubview(&c);
        self.layer
            .views
            .borrow_mut()
            .insert(key.to_string(), Retained::into_super(c.clone()));
        c
    }

    /// A native view for `key`, made once by `make`, placed at `frame`.
    pub fn native<T>(
        &mut self,
        key: &str,
        frame: NSRect,
        make: impl FnOnce() -> Retained<T>,
    ) -> Retained<T>
    where
        T: objc2::DowncastTarget + AsRef<NSView> + objc2::Message,
    {
        self.used.insert(key.to_string());
        let existing = self.layer.views.borrow().get(key).cloned();
        if let Some(v) = existing
            && let Ok(t) = v.downcast::<T>()
        {
            let view: &NSView = (*t).as_ref();
            if view.frame() != frame {
                view.setFrame(frame);
            }
            return t;
        }
        let t = make();
        let view: Retained<NSView> = (*t).as_ref().retain();
        view.setFrame(frame);
        self.layer.root.addSubview(&view);
        self.layer.views.borrow_mut().insert(key.to_string(), view);
        t
    }

    pub fn finish(self) {
        let mut views = self.layer.views.borrow_mut();
        views.retain(|k, v| {
            let keep = self.used.contains(k);
            if !keep {
                v.removeFromSuperview();
            }
            keep
        });
    }
}

define_class!(
    // SAFETY: NSObject subclass holding a closure; no Drop impl beyond the ivars.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckAction"]
    #[ivars = Box<dyn Fn()>]
    struct ActionTarget;

    impl ActionTarget {
        #[unsafe(method(fire:))]
        fn fire(&self, _sender: Option<&AnyObject>) {
            (self.ivars())();
        }
    }
);

unsafe extern "C" {
    fn objc_setAssociatedObject(
        object: *const AnyObject,
        key: *const c_void,
        value: *const AnyObject,
        policy: usize,
    );
}

static TARGET_KEY: u8 = 0;

fn target(f: impl Fn() + 'static) -> Retained<ActionTarget> {
    let this = ActionTarget::alloc(util::mtm()).set_ivars(Box::new(f));
    // SAFETY: NSObject's initializer.
    unsafe { msg_send![super(this), init] }
}

/// Keeps `target` alive as long as `owner`.
fn associate(owner: &AnyObject, target: &AnyObject) {
    const RETAIN_NONATOMIC: usize = 1;
    // SAFETY: both are valid objects; the key is a static address.
    unsafe {
        objc_setAssociatedObject(
            owner,
            &TARGET_KEY as *const u8 as *const c_void,
            target,
            RETAIN_NONATOMIC,
        )
    };
}

/// Calls `f` when the control fires its action (replacing any earlier closure).
pub fn on_action(control: &NSControl, f: impl Fn() + 'static) {
    let t = target(f);
    // SAFETY: the target stays alive through the association below.
    unsafe {
        control.setTarget(Some(&t));
        control.setAction(Some(sel!(fire:)));
    }
    associate(control, &t);
}

/// Calls `f` when a toolbar button is clicked.
pub fn on_toolbar_item(item: &objc2_app_kit::NSToolbarItem, f: impl Fn() + 'static) {
    let t = target(f);
    // SAFETY: the target stays alive through the association below.
    unsafe {
        item.setTarget(Some(&t));
        item.setAction(Some(sel!(fire:)));
    }
    associate(item, &t);
}

/// A menu item that calls `f`. `key` and `modifiers` are its shortcut (shown, not handled).
pub fn menu_item(
    title: &str,
    key: &str,
    modifiers: NSEventModifierFlags,
    f: impl Fn() + 'static,
) -> Retained<NSMenuItem> {
    let mtm = util::mtm();
    // SAFETY: standard initializer with a selector the target implements.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns(title),
            Some(sel!(fire:)),
            &ns(key),
        )
    };
    item.setKeyEquivalentModifierMask(modifiers);
    let t = target(f);
    // SAFETY: the target stays alive through the association below.
    unsafe { item.setTarget(Some(&t)) };
    associate(&item, &t);
    item
}

/// A plain menu item with no shortcut.
pub fn item(title: &str, f: impl Fn() + 'static) -> Retained<NSMenuItem> {
    menu_item(title, "", NSEventModifierFlags::empty(), f)
}

/// A disabled menu item (a heading or a note).
pub fn disabled_item(title: &str) -> Retained<NSMenuItem> {
    let i = item(title, || {});
    i.setEnabled(false);
    i
}

/// Closures for a text field's editing: every change, the end of editing (Return or focus
/// leaving), and Escape.
#[derive(Default)]
pub struct FieldIvars {
    pub changed: RefCell<Option<TextHandler>>,
    pub ended: RefCell<Option<TextHandler>>,
    pub cancelled: RefCell<Option<Box<dyn Fn()>>>,
}

/// Called with a text field's text.
pub type TextHandler = Box<dyn Fn(String)>;

define_class!(
    // SAFETY: NSObject subclass acting as a text field delegate; no Drop impl beyond the ivars.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckFieldDelegate"]
    #[ivars = FieldIvars]
    pub struct FieldDelegate;

    unsafe impl NSObjectProtocol for FieldDelegate {}

    unsafe impl NSControlTextEditingDelegate for FieldDelegate {}

    unsafe impl NSTextFieldDelegate for FieldDelegate {}

    impl FieldDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn did_change(&self, n: &objc2_foundation::NSNotification) {
            if let Some(f) = self.ivars().changed.borrow().as_ref() {
                f(field_text(n));
            }
        }

        #[unsafe(method(controlTextDidEndEditing:))]
        fn did_end(&self, n: &objc2_foundation::NSNotification) {
            if let Some(f) = self.ivars().ended.borrow().as_ref() {
                f(field_text(n));
            }
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn command(&self, _c: &NSControl, _t: &objc2_app_kit::NSTextView, selector: objc2::runtime::Sel) -> objc2::runtime::Bool {
            if selector == sel!(cancelOperation:)
                && let Some(f) = self.ivars().cancelled.borrow().as_ref()
            {
                f();
                return objc2::runtime::Bool::YES;
            }
            objc2::runtime::Bool::NO
        }
    }
);

fn field_text(n: &objc2_foundation::NSNotification) -> String {
    n.object()
        .and_then(|o| o.downcast::<objc2_app_kit::NSTextField>().ok())
        .map(|f| f.stringValue().to_string())
        .unwrap_or_default()
}

impl FieldDelegate {
    pub fn new() -> Retained<FieldDelegate> {
        let this = FieldDelegate::alloc(util::mtm()).set_ivars(FieldIvars::default());
        // SAFETY: NSObject's initializer.
        unsafe { msg_send![super(this), init] }
    }

    /// Makes this the field's delegate, kept alive by the field.
    pub fn attach(&self, field: &objc2_app_kit::NSTextField) {
        // SAFETY: the delegate is kept alive by the association below.
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(self))) };
        static KEY: u8 = 0;
        const RETAIN_NONATOMIC: usize = 1;
        // SAFETY: both are valid objects; the key is a static address.
        unsafe {
            objc_setAssociatedObject(
                field as *const _ as *const AnyObject,
                &KEY as *const u8 as *const c_void,
                self as *const _ as *const AnyObject,
                RETAIN_NONATOMIC,
            )
        };
    }
}
