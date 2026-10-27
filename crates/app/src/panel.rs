//! The deck's window: a borderless panel under the menu bar icon, like SwiftUI's
//! `MenuBarExtra` window style. A click on the icon, outside the deck, or Escape closes it.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSPanel, NSScreen,
    NSStatusWindowLevel, NSView, NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSRect};

use crate::canvas::Layer;
use crate::deck::{self, W};
use crate::draw::rect;
use crate::{store, util};

const KEY_ESCAPE: u16 = 53;
const KEY_R: u16 = 15;

define_class!(
    // SAFETY: NSPanel subclass that only lets itself become key (borderless windows can't by
    // default), so the rename field and Escape work.
    #[unsafe(super = NSPanel)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckDeckPanel"]
    pub struct KeyPanel;

    impl KeyPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }
    }
);

define_class!(
    // SAFETY: NSObject subclass acting as the panel's delegate; no Drop impl.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckDeckDelegate"]
    struct PanelDelegate;

    unsafe impl NSObjectProtocol for PanelDelegate {}

    unsafe impl NSWindowDelegate for PanelDelegate {
        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, _n: &NSNotification) {
            // Another window took focus. A scripted run opens and closes the deck itself.
            if !crate::drive::driving() {
                util::main_async(close);
            }
        }
    }
);

struct Panel {
    window: Retained<KeyPanel>,
    layer: Layer,
    _delegate: Retained<PanelDelegate>,
    monitors: Vec<Retained<AnyObject>>,
}

thread_local! {
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
    static LAST_CLOSE: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Bumped to stop the once-a-second clock refresh.
    static CLOCK: Cell<u64> = const { Cell::new(0) };
}

pub const LINKS: deck::Links = deck::Links {
    recordings: crate::recordings::show,
    settings: crate::settings_window::show,
};

/// A flipped view the deck is laid out in, with the deck colour behind it.
pub fn deck_root(mtm: MainThreadMarker) -> Retained<NSView> {
    let root = crate::canvas::Canvas::new(mtm, rect(0.0, 0.0, W, 300.0));
    root.setWantsLayer(true);
    Retained::into_super(root)
}

pub fn is_open() -> bool {
    PANEL.with(|p| p.borrow().is_some())
}

/// Opens or closes the deck (the icon's click).
pub fn toggle() {
    if is_open() {
        close();
    } else if LAST_CLOSE
        .with(|l| l.get())
        .is_none_or(|t| t.elapsed().as_secs_f64() > 0.3)
    {
        // A click on the icon can first close the deck by taking key focus; don't reopen it
        // from the same click.
        open();
    }
}

fn anchor(mtm: MainThreadMarker) -> Option<NSRect> {
    crate::status_item::button_frame(mtm)
}

fn frame_for(mtm: MainThreadMarker, height: f64) -> NSRect {
    let screen = NSScreen::mainScreen(mtm)
        .map(|s| s.visibleFrame())
        .unwrap_or(rect(0.0, 0.0, 1440.0, 900.0));
    let (left, top) = match anchor(mtm) {
        Some(a) => (a.origin.x, a.origin.y - 1.0),
        None => (
            screen.origin.x + screen.size.width - W - 8.0,
            screen.origin.y + screen.size.height - 1.0,
        ),
    };
    let min_x = screen.origin.x + 8.0;
    let max_x = screen.origin.x + screen.size.width - W - 8.0;
    rect(left.clamp(min_x, max_x.max(min_x)), top - height, W, height)
}

pub fn open() {
    let mtm = util::mtm();
    if is_open() {
        return;
    }
    let window: Retained<KeyPanel> = unsafe {
        msg_send![
            KeyPanel::alloc(mtm),
            initWithContentRect: frame_for(mtm, 300.0),
            styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            backing: NSBackingStoreType::Buffered,
            defer: false
        ]
    };
    // SAFETY: the panel is kept in PANEL until closed.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setLevel(NSStatusWindowLevel);
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setHasShadow(true);
    window.setHidesOnDeactivate(false);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Transient,
    );
    let root = deck_root(mtm);
    if let Some(l) = root.layer() {
        l.setCornerRadius(10.0);
        l.setMasksToBounds(true);
    }
    window.setContentView(Some(&root));
    let delegate: Retained<PanelDelegate> = {
        let this = PanelDelegate::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    };
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let mut monitors = Vec::new();
    // Escape ends a rename, then identify, then closes; ⌥⌘R records (the global hot key does
    // too, except in the demo, which doesn't claim it).
    let keys = RcBlock::new(|e: NonNull<NSEvent>| -> *mut NSEvent {
        let ev = unsafe { e.as_ref() };
        if !is_open() {
            return e.as_ptr();
        }
        if ev.keyCode() == KEY_ESCAPE {
            if !deck::cancel_editing() {
                if store::with(|s| s.identifying) {
                    store::stop_identify();
                } else {
                    util::main_async(close);
                }
            }
            return std::ptr::null_mut();
        }
        let mods = ev.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
        if ev.keyCode() == KEY_R
            && mods == NSEventModifierFlags::Command | NSEventModifierFlags::Option
        {
            store::toggle_recording();
            return std::ptr::null_mut();
        }
        e.as_ptr()
    });
    if let Some(m) = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &keys)
    } {
        monitors.push(m);
    }
    let outside = RcBlock::new(|_e: NonNull<NSEvent>| {
        if !crate::drive::driving() {
            util::main_async(close)
        }
    });
    if let Some(m) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown,
        &outside,
    ) {
        monitors.push(m);
    }
    let layer = Layer::new(root);
    PANEL.with(|p| {
        *p.borrow_mut() = Some(Panel {
            window: window.clone(),
            layer,
            _delegate: delegate,
            monitors,
        })
    });
    refresh();
    window.makeKeyAndOrderFront(None);
    crate::status_item::highlight(true);
    tick_clock();
}

/// Redraws the deck from the model, resizing the window to fit (its top stays put).
pub fn refresh() {
    PANEL.with(|p| {
        let p = p.borrow();
        let Some(p) = p.as_ref() else { return };
        let height = {
            let s = store::with(|s| s.clone());
            deck::render(&p.layer, &s, &LINKS)
        };
        let mtm = util::mtm();
        let mut frame = p.window.frame();
        if (frame.size.height - height).abs() > 0.1 {
            let top = frame.origin.y + frame.size.height;
            frame = if p.window.isVisible() {
                rect(frame.origin.x, top - height, W, height)
            } else {
                frame_for(mtm, height)
            };
            p.window.setFrame_display(frame, true);
        }
        p.layer.root.setFrame(rect(0.0, 0.0, W, height));
        deck::focus_editing(&p.layer);
    });
}

/// Once a second while recording, so the running time ticks.
fn tick_clock() {
    let run = CLOCK.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    fn step(run: u64) {
        util::main_after(1.0, move || {
            if CLOCK.with(|c| c.get()) != run || !is_open() {
                return;
            }
            if store::with(|s| s.is_recording) {
                refresh();
            }
            step(run);
        });
    }
    step(run);
}

pub fn close() {
    let Some(p) = PANEL.with(|p| p.borrow_mut().take()) else {
        return;
    };
    deck::cancel_editing();
    CLOCK.with(|c| c.set(c.get() + 1));
    for m in &p.monitors {
        unsafe { NSEvent::removeMonitor(m) };
    }
    p.window.setDelegate(None);
    p.window.orderOut(None);
    p.window.close();
    crate::status_item::highlight(false);
    LAST_CLOSE.with(|l| l.set(Some(Instant::now())));
}

/// The deck window, for scripted checks.
pub fn window() -> Option<Retained<KeyPanel>> {
    PANEL.with(|p| p.borrow().as_ref().map(|p| p.window.clone()))
}

/// A view of the deck by its key, for scripted checks.
pub fn view(key: &str) -> Option<Retained<NSView>> {
    PANEL.with(|p| p.borrow().as_ref().and_then(|p| p.layer.get(key)))
}
