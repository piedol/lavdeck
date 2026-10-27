//! The menu bar icon. A click opens the deck; a right-click (or Control-click) shows quick
//! actions: record, mono or stereo, Recordings, Settings, Quit.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSEventMask, NSEventModifierFlags, NSEventType, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::NSRect;

use crate::canvas::{disabled_item, item, menu_item, on_action};
use crate::i18n::tr;
use crate::model::Glyph;
use crate::model::status::ChannelMode;
use crate::{glyph, panel, store, util};

thread_local! {
    static ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
    static SHOWN: Cell<Option<Glyph>> = const { Cell::new(None) };
}

pub fn install(mtm: MainThreadMarker) {
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        on_action(&button, clicked);
        button.sendActionOn(NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown);
    }
    ITEM.with(|i| *i.borrow_mut() = Some(item));
    refresh();
}

/// Redraws the icon when the glyph state changes.
pub fn refresh() {
    let state = store::with(|s| s.glyph());
    if SHOWN.with(|s| s.replace(Some(state))) == Some(state) {
        return;
    }
    let mtm = util::mtm();
    ITEM.with(|i| {
        let Some(button) = i.borrow().as_ref().and_then(|i| i.button(mtm)) else {
            return;
        };
        button.setImage(Some(&glyph::image(state)));
        button.setAccessibilityLabel(Some(&util::ns(&glyph::accessibility(state))));
    });
}

pub fn button_frame(mtm: MainThreadMarker) -> Option<NSRect> {
    ITEM.with(|i| {
        i.borrow()
            .as_ref()
            .and_then(|i| i.button(mtm))
            .and_then(|b| b.window())
            .map(|w| w.frame())
    })
}

pub fn highlight(on: bool) {
    let mtm = util::mtm();
    ITEM.with(|i| {
        if let Some(b) = i.borrow().as_ref().and_then(|i| i.button(mtm)) {
            b.highlight(on);
        }
    });
}

fn clicked() {
    let mtm = util::mtm();
    let secondary = NSApplication::sharedApplication(mtm)
        .currentEvent()
        .is_some_and(|e| {
            e.r#type() == NSEventType::RightMouseDown
                || e.modifierFlags().contains(NSEventModifierFlags::Control)
        });
    if !secondary {
        panel::toggle();
        return;
    }
    panel::close();
    let menu = make_menu();
    ITEM.with(|i| {
        if let Some(i) = i.borrow().as_ref() {
            #[allow(deprecated)]
            i.popUpStatusItemMenu(&menu);
        }
    });
}

/// The right-click menu, as `StatusMenu.swift` builds it.
pub fn make_menu() -> Retained<NSMenu> {
    let mtm = util::mtm();
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let opt_cmd = NSEventModifierFlags::Option | NSEventModifierFlags::Command;
    let (recording, connected, none_armed, mode) = store::with(|s| {
        (
            s.is_recording,
            s.is_connected,
            s.settings.armed.is_empty(),
            s.channel_mode(),
        )
    });
    if recording {
        menu.addItem(&menu_item(
            &tr("Stop Recording"),
            "r",
            opt_cmd,
            store::stop_recording,
        ));
    } else {
        let title = if none_armed {
            tr("Start Recording (no mic armed)")
        } else {
            tr("Start Recording")
        };
        let start = menu_item(&title, "r", opt_cmd, store::start_recording);
        start.setEnabled(connected && !none_armed);
        menu.addItem(&start);
    }
    if connected && !recording {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        for (title, target) in [
            (tr("Mono"), ChannelMode::Mono),
            (tr("Stereo"), ChannelMode::Stereo),
        ] {
            let entry = item(&title, move || store::set_channel(target));
            entry.setState(if mode == Some(target) {
                objc2_app_kit::NSControlStateValueOn
            } else {
                objc2_app_kit::NSControlStateValueOff
            });
            menu.addItem(&entry);
        }
    } else if !connected {
        menu.addItem(&disabled_item(&tr("Receiver not connected")));
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item(&tr("Recordings…"), crate::recordings::show));
    menu.addItem(&menu_item(
        &tr("Settings…"),
        ",",
        NSEventModifierFlags::Command,
        crate::settings_window::show,
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let quit = menu_item(
        &tr("Quit Lavdeck"),
        "q",
        NSEventModifierFlags::Command,
        || {},
    );
    // SAFETY: NSApplication handles terminate:.
    unsafe {
        quit.setAction(Some(sel!(terminate:)));
        quit.setTarget(None);
    }
    menu.addItem(&quit);
    menu
}
