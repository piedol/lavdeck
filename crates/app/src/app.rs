//! App lifecycle: the delegate, a hidden main menu for the windows' shortcuts, and quitting
//! mid-take (the take is saved first, as the Swift app does for "Quit & Reopen").

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSEventModifierFlags, NSMenu, NSMenuItem,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol};

use crate::backend::Backend;
use crate::i18n::tr;
use crate::model::AppState;
use crate::snapshot::Args;
use crate::util::ns;
use crate::{
    demo, hotkey, login, panel, prefs, recordings, settings_window, snapshot, status_item, store,
    util,
};

thread_local! {
    static ARGS: RefCell<Option<Args>> = const { RefCell::new(None) };
    static SAVED_FOR_QUIT: Cell<bool> = const { Cell::new(false) };
    static QUITTING: Cell<bool> = const { Cell::new(false) };
}

define_class!(
    // SAFETY: NSObject subclass acting as the app delegate; no Drop impl.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckAppDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            launched(self.mtm());
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            if !store::with(|s| s.is_recording || s.saving) || SAVED_FOR_QUIT.with(|s| s.get()) {
                return NSApplicationTerminateReply::TerminateNow;
            }
            // Cancel this quit, save the take (at most 20 seconds), then quit again. Waiting
            // inside a later reply would block the main-queue work that saves the take.
            if !QUITTING.with(|q| q.replace(true)) {
                store::stop_recording();
                wait_then_quit(200);
            }
            NSApplicationTerminateReply::TerminateCancel
        }
    }
);

fn wait_then_quit(tries: u32) {
    if store::with(|s| s.is_recording || s.saving) && tries > 0 {
        util::later(0.1, move || wait_then_quit(tries - 1));
        return;
    }
    SAVED_FOR_QUIT.with(|s| s.set(true));
    NSApplication::sharedApplication(util::mtm()).terminate(None);
}

pub fn run(args: Args) {
    let mtm = MainThreadMarker::new().expect("Lavdeck must start on the main thread");
    ARGS.with(|a| *a.borrow_mut() = Some(args));
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let delegate: Retained<Delegate> = {
        let this = Delegate::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    };
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    drop(delegate);
}

fn launched(mtm: MainThreadMarker) {
    let args = ARGS.with(|a| a.borrow_mut().take()).expect("args");
    // An unattended update test runs as the demo, so it leaves the receiver to the copy of
    // Lavdeck you use.
    let demo_mode = args.demo || crate::update_ui::testing();
    install_main_menu(mtm);
    if demo_mode {
        // Made-up mics and takes: never the receiver, the shortcut or saved settings.
        let d = Rc::new(demo::Demo::new(!args.is_scripted()));
        let mut state = AppState::new(d.settings());
        state.force_low_battery = args.force_low_battery;
        demo::load(&mut state, false);
        demo::install(d.clone(), state);
        store::reload_plugins();
        store::update(|s| {
            for p in demo::plugins() {
                s.plugins.set_enabled(p.id(), true);
            }
        });
        d.animate();
    } else {
        let backend: Rc<dyn Backend> = crate::live::Live::new();
        let mut state = AppState::new(prefs::load());
        state.force_low_battery = args.force_low_battery;
        state.launch_at_login = login::enabled();
        store::install(state, backend);
        store::reload_plugins();
    }
    store::observe(status_item::refresh);
    store::observe(panel::refresh);
    store::observe(recordings::refresh);
    store::observe(settings_window::refresh);

    if let Some(dir) = args.snapshot {
        // With the real receiver: only the link, which reads the receiver's status. No hot key,
        // no button monitor, no menu bar icon; nothing a stray click or key could start.
        if !demo_mode {
            crate::live::start_link_only();
        }
        snapshot::run(dir, demo_mode);
        return;
    }
    if let Some(dir) = args.windows {
        snapshot::run_windows(dir);
        return;
    }
    status_item::install(mtm);
    if let Some(dir) = args.drive {
        crate::drive::run(dir);
        return;
    }
    store::backend().start();
    if !demo_mode && !hotkey::register() {
        util::log("⌥⌘R is taken by another app");
    }
    crate::update_ui::launched(demo_mode);
}

/// A main menu nobody sees (Lavdeck has no menu bar of its own), so ⌘W, ⌘Q, ⌘, ⌘A, ⌘F and
/// the edit shortcuts work in Lavdeck's windows.
fn install_main_menu(mtm: MainThreadMarker) {
    let item = |title: &str, action: objc2::runtime::Sel, key: &str, mods: NSEventModifierFlags| {
        // SAFETY: standard initializer; the action goes up the responder chain.
        let i = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &ns(title),
                Some(action),
                &ns(key),
            )
        };
        i.setKeyEquivalentModifierMask(mods);
        i
    };
    let cmd = NSEventModifierFlags::Command;
    let main = NSMenu::new(mtm);
    let app_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &ns("Lavdeck"));
    app_menu.addItem(&crate::canvas::menu_item(
        &tr("Settings…"),
        ",",
        cmd,
        settings_window::show,
    ));
    app_menu.addItem(&item(&tr("Quit Lavdeck"), sel!(terminate:), "q", cmd));
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), &ns(&tr("Edit")));
    edit.addItem(&item(&tr("Undo"), sel!(undo:), "z", cmd));
    edit.addItem(&item(&tr("Cut"), sel!(cut:), "x", cmd));
    edit.addItem(&item(&tr("Copy"), sel!(copy:), "c", cmd));
    edit.addItem(&item(&tr("Paste"), sel!(paste:), "v", cmd));
    edit.addItem(&item(&tr("Select All"), sel!(selectAll:), "a", cmd));
    edit.addItem(&item(&tr("Find"), sel!(performFindPanelAction:), "f", cmd));
    let window = NSMenu::initWithTitle(NSMenu::alloc(mtm), &ns(&tr("Window")));
    window.addItem(&item(&tr("Close"), sel!(performClose:), "w", cmd));
    for (title, sub) in [
        ("Lavdeck".to_string(), app_menu),
        (tr("Edit"), edit),
        (tr("Window"), window),
    ] {
        let top = item(
            &title,
            sel!(submenuAction:),
            "",
            NSEventModifierFlags::empty(),
        );
        top.setSubmenu(Some(&sub));
        main.addItem(&top);
    }
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&main));
}

/// Nothing would be lost by quitting now: no take starting, recording or being saved. An
/// update waits for this before it replaces the app.
pub fn is_idle() -> bool {
    !store::with(|s| s.is_starting || s.is_recording || s.saving)
}

/// Quits the app the normal way (a take in progress is saved first).
pub fn quit() {
    NSApplication::sharedApplication(util::mtm()).terminate(None);
}

/// Brings Lavdeck forward (a menu bar app isn't frontmost, so its windows would open behind).
pub fn activate() {
    // A scripted run never takes focus from the app in front.
    if crate::drive::driving() {
        return;
    }
    NSApplication::sharedApplication(util::mtm()).activate();
}
