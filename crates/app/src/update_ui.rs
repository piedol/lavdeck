//! Lavdeck's side of the updater (the shared part is in `update/`, copied from Tamka): the
//! menu item, the daily check and its setting, the notification and the update window.
//!
//! `LAVDECK_UPDATE_TEST=install` checks at launch and installs anything newer without asking,
//! for testing the whole install. That run uses the demo's made-up mics (see [`testing`]), so it
//! never opens the receiver, registers the shortcut or reads or writes saved settings.

use std::cell::{Cell, RefCell};
use std::time::SystemTime;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, define_class, msg_send};
use objc2_app_kit::{
    NSAlert, NSApplication, NSBorderType, NSButton, NSFont, NSFontAttributeName,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSScrollView, NSTextAlignment,
    NSTextTab, NSTextView, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSError, NSObject, NSObjectProtocol, NSPoint, NSRange, NSRect,
    NSSet, NSSize, NSURL,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
    UNNotificationCategoryOptions, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNNotificationSettings, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

use crate::canvas::on_action;
use crate::i18n::{tr, trf};
use crate::update::notes::{Line, Style};
use crate::update::{self, Check, Release, Step};
use crate::util::{self, ns};
use crate::{prefs, store};

pub static CONFIG: update::Config = update::Config {
    repo: "piedol/lavdeck",
    asset: "Lavdeck-macOS-arm64.zip",
    name: "Lavdeck",
    bundle_id: "io.github.piedol.lavdeck",
    team_id: "D9JNQPYFBM",
    env_prefix: "LAVDECK",
    fallback_version: env!("CARGO_PKG_VERSION"),
    log: |m| util::log(m),
};

/// What the menu item offers.
enum State {
    Idle,
    /// A check chosen from the menu is running.
    Checking,
    Available(Release),
    Installing(Release, Step),
}

/// The update window's and the messages' buttons.
#[derive(Clone, Copy, PartialEq)]
enum Choice {
    Install,
    Later,
    Ok,
}

/// The notification's category, action and request id.
const NOTIFICATION: &str = "update";

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State::Idle) };
    /// The update window or a message, while one is open.
    static ALERT: RefCell<Option<Retained<NSAlert>>> = const { RefCell::new(None) };
    /// The version last announced in a notification, so each is announced once per run.
    static NOTIFIED: RefCell<Option<update::Version>> = const { RefCell::new(None) };
    /// The notification centre's delegate (it holds it weakly).
    static DELEGATE: RefCell<Option<Retained<Notifications>>> = const { RefCell::new(None) };
    /// The demo checks only when asked, and never records when it did.
    static DEMO: Cell<bool> = const { Cell::new(false) };
    /// LAVDECK_UPDATE_TEST=install: unattended, so results go to the log only.
    static TESTING: Cell<bool> = const { Cell::new(false) };
}

/// Whether this run tests the install (`LAVDECK_UPDATE_TEST=install`). It runs as the demo.
pub fn testing() -> bool {
    std::env::var("LAVDECK_UPDATE_TEST").as_deref() == Ok("install")
}

fn current() -> update::Version {
    update::current_version(&CONFIG)
}

fn set_state(s: State) {
    STATE.with(|st| *st.borrow_mut() = s);
}

fn available() -> Option<Release> {
    STATE.with(|st| match &*st.borrow() {
        State::Available(r) | State::Installing(r, _) => Some(r.clone()),
        _ => None,
    })
}

/// Called once the app is running (not for snapshots or scripted checks).
pub fn launched(demo: bool) {
    DEMO.with(|d| d.set(demo));
    if testing() {
        TESTING.with(|t| t.set(true));
        if let Some(v) = update::launched(&CONFIG) {
            util::log(format!("update test: running {v}, after an update"));
        }
        util::log("update test: checking now, installing without asking");
        update::check(&CONFIG, |result| match result {
            Ok(Check::Available(r)) => {
                set_state(State::Available(r.clone()));
                install(r);
            }
            Ok(Check::UpToDate) => util::log("update test: nothing newer"),
            Err(e) => util::log(format!("update test: {e}")),
        });
        return;
    }
    if demo {
        return;
    }
    if let Some(v) = update::launched(&CONFIG) {
        util::log(format!("updated to Lavdeck {v}"));
    }
    setup_notifications();
    start_schedule();
}

fn start_schedule() {
    update::schedule::start(
        || store::with(|s| s.settings.check_updates),
        prefs::last_update_check,
        auto_check,
    );
}

/// Check for updates automatically changed in Settings.
pub fn setting_changed() {
    if !DEMO.with(Cell::get) {
        start_schedule();
    }
}

fn auto_check() {
    if STATE.with(|s| matches!(&*s.borrow(), State::Checking | State::Installing(..))) {
        return;
    }
    update::check(&CONFIG, |result| checked(result, false));
}

/// Check for Updates… in the menu.
fn check_now() {
    set_state(State::Checking);
    update::check(&CONFIG, |result| checked(result, true));
}

/// Whether GitHub answered, so the next automatic check can wait a day. A 404 is GitHub saying
/// the repository has no release yet, which is an answer too.
fn answered(result: &Result<Check, update::Error>) -> bool {
    matches!(result, Ok(_) | Err(update::Error::Http(404)))
}

fn checked(result: Result<Check, update::Error>, manual: bool) {
    if answered(&result) && !DEMO.with(Cell::get) {
        prefs::set_last_update_check(SystemTime::now());
    }
    if update::installing() {
        return;
    }
    match result {
        Ok(Check::Available(r)) => {
            set_state(State::Available(r.clone()));
            if manual {
                show_update(&r);
            } else {
                notify(&r);
            }
        }
        Ok(Check::UpToDate) => {
            set_state(State::Idle);
            if manual {
                let version = current().to_string();
                message(
                    &tr("Lavdeck is up to date"),
                    &trf(
                        "Lavdeck {version} is the newest version.",
                        &[("version", &version)],
                    ),
                );
            }
        }
        Err(e) => {
            if STATE.with(|s| matches!(&*s.borrow(), State::Checking)) {
                set_state(State::Idle);
            }
            if manual {
                message(&tr("Couldn’t check for updates"), &error_text(&e));
            }
        }
    }
}

/// The core's errors, in Lavdeck's words and language. Their English details go to the log.
fn error_text(e: &update::Error) -> String {
    use update::Error;
    match e {
        // The core's own words for a download that arrived but couldn't be kept.
        Error::Network(why) if why.starts_with("The download") => tr("The download didn’t finish."),
        // macOS's description of the failure, already in Lavdeck's language.
        Error::Network(why) => trf("Couldn’t reach GitHub. {reason}", &[("reason", why)]),
        Error::Http(403 | 429) => {
            tr("GitHub is limiting requests from this network. Try again in an hour.")
        }
        Error::Http(404) => tr("GitHub has no published release to compare with."),
        Error::Http(code) => trf(
            "GitHub answered with HTTP status {code}. Try again later.",
            &[("code", &code.to_string())],
        ),
        Error::Release(_) => tr("GitHub didn’t answer with a release Lavdeck can install."),
        Error::Verify(_) => tr("The download didn’t pass Lavdeck’s checks."),
        Error::NotWritable(_) => not_writable_text(),
        Error::NotBundled(_) => {
            tr("Lavdeck isn’t running from its app bundle, so it can’t replace itself.")
        }
        Error::Replace(_) => tr("The new version couldn’t be put in place."),
        Error::Relaunch(_) => tr("Lavdeck couldn’t reopen itself. Quit Lavdeck and open it again."),
    }
}

/// Why Lavdeck can't replace itself where it is.
fn not_writable_text() -> String {
    let bundle = std::path::PathBuf::from(NSBundle::mainBundle().bundlePath().to_string());
    let folder = bundle.parent().unwrap_or(&bundle).display().to_string();
    if folder.contains("/AppTranslocation/") {
        tr(
            "macOS is running Lavdeck from a read-only copy, so it can’t replace itself. Move Lavdeck into the Applications folder and open it from there.",
        )
    } else {
        trf(
            "Lavdeck can’t replace itself because your account can’t make changes in {folder}.",
            &[("folder", &folder)],
        )
    }
}

// MARK: The menu item

/// The menu item: (title, subtitle, enabled).
pub fn menu_item() -> (String, Option<String>, bool) {
    STATE.with(|st| match &*st.borrow() {
        State::Idle => (tr("Check for Updates…"), None, true),
        State::Checking => (tr("Checking for Updates…"), None, false),
        State::Available(r) => (
            trf(
                "Update to Lavdeck {version}…",
                &[("version", &r.version.to_string())],
            ),
            Some(trf(
                "You have {version}",
                &[("version", &current().to_string())],
            )),
            true,
        ),
        State::Installing(r, step) => (
            trf(
                "Installing Lavdeck {version}…",
                &[("version", &r.version.to_string())],
            ),
            Some(step_text(*step, r.size, store::with(|s| s.saving))),
            false,
        ),
    })
}

/// Where the install has got to. `saving`: the take is being saved (else it's recording).
fn step_text(step: Step, size: u64, saving: bool) -> String {
    match step {
        Step::Downloading { received, total } => {
            let total = if total > 0 { total } else { size };
            match (received * 100).checked_div(total) {
                Some(percent) => trf(
                    "Downloading, {percent}%",
                    &[("percent", &percent.min(100).to_string())],
                ),
                None => tr("Downloading"),
            }
        }
        Step::Verifying => tr("Checking the download"),
        Step::Waiting if saving => tr("Installs once the take is saved"),
        Step::Waiting => tr("Installs when you stop recording"),
        Step::Replacing => tr("Replacing the app"),
    }
}

pub fn menu_chosen() {
    match available() {
        Some(r) if !update::installing() => show_update(&r),
        Some(_) => {}
        None if STATE.with(|s| matches!(&*s.borrow(), State::Idle)) => check_now(),
        None => {}
    }
}

// MARK: Installing

fn install(r: Release) {
    set_state(State::Installing(
        r.clone(),
        Step::Downloading {
            received: 0,
            total: 0,
        },
    ));
    let hooks = update::Hooks {
        is_idle: crate::app::is_idle,
        progress,
        done: installed,
    };
    update::install(&CONFIG, r, hooks);
}

fn progress(step: Step) {
    STATE.with(|st| {
        if let State::Installing(_, s) = &mut *st.borrow_mut() {
            *s = step;
        }
    });
}

fn installed(result: Result<(), update::Error>) {
    let Some(r) = available() else { return };
    let version = r.version.to_string();
    match result {
        // Quitting still waits for a take that started in the meantime (see `app`).
        Ok(()) => crate::app::quit(),
        Err(e) if TESTING.with(Cell::get) => {
            util::log(format!("update test: not installed: {e}"));
            set_state(State::Available(r));
        }
        Err(update::Error::NotWritable(_)) => {
            if let Some(url) = NSURL::URLWithString(&ns(&r.page)) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
            set_state(State::Available(r));
            message(
                &trf(
                    "Download Lavdeck {version} from GitHub",
                    &[("version", &version)],
                ),
                &trf(
                    "{reason} The release page is open in your browser.",
                    &[("reason", &not_writable_text())],
                ),
            );
        }
        Err(e @ update::Error::Relaunch(_)) => {
            set_state(State::Idle);
            message(
                &trf("Lavdeck {version} is installed", &[("version", &version)]),
                &error_text(&e),
            );
        }
        Err(e) => {
            set_state(State::Available(r));
            message(
                &trf(
                    "Lavdeck {version} wasn’t installed",
                    &[("version", &version)],
                ),
                &trf(
                    "{reason} Lavdeck {version} is still installed.",
                    &[
                        ("reason", &error_text(&e)),
                        ("version", &current().to_string()),
                    ],
                ),
            );
        }
    }
}

// MARK: Windows

/// The version, the installed version and the release notes, with Install and Relaunch and Later.
fn show_update(r: &Release) {
    let mtm = util::mtm();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns(&trf(
        "Lavdeck {version} is available",
        &[("version", &r.version.to_string())],
    )));
    alert.setInformativeText(&ns(&trf(
        "You have Lavdeck {version}. Installing quits Lavdeck and opens the new version. During a take, the install waits until you stop and the take is saved. Your settings and recordings carry over.",
        &[("version", &current().to_string())],
    )));
    alert.setAccessoryView(Some(&notes_view(mtm, &r.notes)));
    let install = alert.addButtonWithTitle(&ns(&tr("Install and Relaunch")));
    let later = alert.addButtonWithTitle(&ns(&tr("Later")));
    later.setKeyEquivalent(&ns("\u{1b}"));
    present(
        alert,
        vec![(install, Choice::Install), (later, Choice::Later)],
    );
}

/// A message with an OK button.
fn message(title: &str, text: &str) {
    let alert = NSAlert::new(util::mtm());
    alert.setMessageText(&ns(title));
    alert.setInformativeText(&ns(text));
    let ok = alert.addButtonWithTitle(&ns(&tr("OK")));
    present(alert, vec![(ok, Choice::Ok)]);
}

/// Shows an alert without making it modal, so the deck, the menu and ⌥⌘R keep working while
/// it's open.
fn present(alert: Retained<NSAlert>, buttons: Vec<(Retained<NSButton>, Choice)>) {
    close_alert(false);
    for (b, choice) in buttons {
        on_action(&b, move || button(choice));
    }
    alert.layout();
    let window = alert.window();
    window.center();
    crate::app::activate();
    // The answer to a check arrives after the menu has closed, without fresh user input, and
    // macOS 14's cooperative activation then refuses `activate`, leaving the window behind
    // other apps' windows. The older call still brings it forward.
    if !crate::drive::driving() {
        #[allow(deprecated)]
        NSApplication::sharedApplication(MainThreadMarker::new().expect("main thread"))
            .activateIgnoringOtherApps(true);
        window.orderFrontRegardless();
    }
    window.makeKeyAndOrderFront(None);
    ALERT.with(|a| *a.borrow_mut() = Some(alert));
}

fn close_alert(deactivate: bool) {
    let Some(alert) = ALERT.with(|a| a.borrow_mut().take()) else {
        return;
    };
    let window = alert.window();
    window.orderOut(None);
    // Hand focus back to the app you were in, unless another of Lavdeck's windows is open.
    let app = NSApplication::sharedApplication(util::mtm());
    let others = app.windows().iter().any(|w| {
        Retained::as_ptr(&w) != Retained::as_ptr(&window)
            && w.isVisible()
            && w.styleMask().contains(NSWindowStyleMask::Titled)
    });
    if deactivate && !others && !crate::drive::driving() {
        app.deactivate();
    }
}

fn button(choice: Choice) {
    close_alert(true);
    if choice == Choice::Install
        && let Some(r) = available()
        && !update::installing()
    {
        install(r);
    }
}

/// The release notes in a scrolling text view.
fn notes_view(mtm: MainThreadMarker, markdown: &str) -> Retained<NSScrollView> {
    let scroll = NSTextView::scrollableTextView(mtm);
    scroll.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(440.0, 220.0),
    ));
    scroll.setBorderType(NSBorderType::BezelBorder);
    let Some(view) = scroll
        .documentView()
        .and_then(|v| v.downcast::<NSTextView>().ok())
    else {
        return scroll;
    };
    view.setEditable(false);
    view.setTextContainerInset(NSSize::new(6.0, 8.0));
    let lines = update::notes::parse(markdown);
    let (text, styles) = if lines.is_empty() {
        (tr("No release notes."), Vec::new())
    } else {
        layout(&lines)
    };
    view.setString(&ns(&text));
    view.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    let Some(storage) = (unsafe { view.textStorage() }) else {
        return scroll;
    };
    let bullet = NSMutableParagraphStyle::new();
    // SAFETY: a left tab stop with no options.
    let tab = unsafe {
        NSTextTab::initWithTextAlignment_location_options(
            NSTextTab::alloc(),
            NSTextAlignment::Left,
            14.0,
            &NSDictionary::new(),
        )
    };
    bullet.setTabStops(Some(&NSArray::from_retained_slice(&[tab])));
    bullet.setHeadIndent(14.0);
    bullet.setParagraphSpacing(3.0);
    let fonts = [
        NSFont::boldSystemFontOfSize(13.0),
        NSFont::boldSystemFontOfSize(12.0),
        NSFont::monospacedSystemFontOfSize_weight(11.0, 0.0),
        NSFont::systemFontOfSize(5.0),
    ];
    for (range, look) in styles {
        let (key, value): (_, &AnyObject) = match look {
            Look::Heading => (unsafe { NSFontAttributeName }, &fonts[0]),
            Look::Bold => (unsafe { NSFontAttributeName }, &fonts[1]),
            Look::Code => (unsafe { NSFontAttributeName }, &fonts[2]),
            Look::Gap => (unsafe { NSFontAttributeName }, &fonts[3]),
            Look::Bullet => (unsafe { NSParagraphStyleAttributeName }, &bullet),
        };
        // SAFETY: fonts and a paragraph style are what these keys take; ranges lie within the text.
        unsafe { storage.addAttribute_value_range(key, value, range) };
    }
    scroll
}

#[derive(Clone, Copy)]
enum Look {
    Heading,
    Bold,
    Code,
    Gap,
    Bullet,
}

/// The notes as one string, with the ranges (in UTF-16 units) to style.
fn layout(lines: &[Line]) -> (String, Vec<(NSRange, Look)>) {
    let mut text = String::new();
    let mut styles = Vec::new();
    let len = |s: &str| s.encode_utf16().count();
    let mut after_gap = false;
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            // A blank line is the newline that ends an empty paragraph: make it short.
            if after_gap {
                styles.push((NSRange::new(len(&text), 1), Look::Gap));
            }
            text.push('\n');
        }
        after_gap = matches!(line, Line::Gap);
        let start = len(&text);
        let spans = match line {
            Line::Gap => continue,
            Line::Bullet(s) => {
                text.push_str("•\t");
                s
            }
            Line::Heading(s) | Line::Text(s) => s,
        };
        for span in spans {
            let at = len(&text);
            text.push_str(&span.text);
            let look = match span.style {
                Style::Plain => continue,
                Style::Bold => Look::Bold,
                Style::Code => Look::Code,
            };
            styles.push((NSRange::new(at, len(&span.text)), look));
        }
        let whole = NSRange::new(start, len(&text) - start);
        match line {
            Line::Heading(_) => styles.push((whole, Look::Heading)),
            Line::Bullet(_) => styles.push((whole, Look::Bullet)),
            _ => {}
        }
    }
    (text, styles)
}

// MARK: The notification

define_class!(
    // SAFETY: NSObject subclass acting as the notification centre's delegate; no Drop impl.
    // macOS may call it off the main thread, so it only hands work to the main queue.
    #[unsafe(super = NSObject)]
    #[name = "LavdeckUpdateNotifications"]
    struct Notifications;

    unsafe impl NSObjectProtocol for Notifications {}

    unsafe impl UNUserNotificationCenterDelegate for Notifications {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            // Show the banner even while a Lavdeck window is in front.
            handler
                .call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let action = response.actionIdentifier().to_string();
            util::main_async(move || notification_response(&action));
            handler.call(());
        }
    }
);

/// UserNotifications only works inside the app bundle, not in a development build.
fn center() -> Option<Retained<UNUserNotificationCenter>> {
    let main = NSBundle::mainBundle();
    let bundled =
        main.bundleIdentifier().is_some() && main.bundlePath().to_string().ends_with(".app");
    bundled.then(UNUserNotificationCenter::currentNotificationCenter)
}

/// The delegate and the notification's Update… button. No permission prompt.
fn setup_notifications() {
    let Some(c) = center() else { return };
    let delegate: Retained<Notifications> = {
        let this = Notifications::alloc().set_ivars(());
        // SAFETY: NSObject's plain initializer.
        unsafe { msg_send![super(this), init] }
    };
    c.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
    let action = UNNotificationAction::actionWithIdentifier_title_options(
        &ns(NOTIFICATION),
        &ns(&tr("Update…")),
        UNNotificationActionOptions::Foreground,
    );
    let category = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &ns(NOTIFICATION),
        &NSArray::from_retained_slice(&[action]),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    );
    c.setNotificationCategories(&NSSet::from_retained_slice(&[category]));
}

/// Announces an update found by the daily check. macOS asks for permission the first time
/// this happens, not at launch.
fn notify(r: &Release) {
    let seen = NOTIFIED.with(|n| n.borrow_mut().replace(r.version.clone()));
    if seen.as_ref() == Some(&r.version) {
        return;
    }
    let Some(c) = center() else { return };
    let content = UNMutableNotificationContent::new();
    content.setTitle(&ns(&trf(
        "Lavdeck {version} is available",
        &[("version", &r.version.to_string())],
    )));
    content.setBody(&ns(&trf(
        "You have {version}. Click to see what’s new and install it.",
        &[("version", &current().to_string())],
    )));
    content.setCategoryIdentifier(&ns(NOTIFICATION));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &ns(NOTIFICATION),
        &content,
        None,
    );
    let center = c.clone();
    let block = RcBlock::new(move |s: std::ptr::NonNull<UNNotificationSettings>| {
        // SAFETY: the system passes a valid settings object.
        let status = unsafe { s.as_ref() }.authorizationStatus();
        let post = {
            let (center, request) = (center.clone(), request.clone());
            move || center.addNotificationRequest_withCompletionHandler(&request, None)
        };
        if status == UNAuthorizationStatus::NotDetermined {
            let asked = RcBlock::new(move |granted: Bool, _e: *mut NSError| {
                util::log(format!(
                    "notifications {}",
                    if granted.as_bool() {
                        "allowed"
                    } else {
                        "declined"
                    }
                ));
                if granted.as_bool() {
                    post();
                }
            });
            center.requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert,
                &asked,
            );
        } else if status == UNAuthorizationStatus::Authorized
            || status == UNAuthorizationStatus::Provisional
        {
            post();
        }
    });
    c.getNotificationSettingsWithCompletionHandler(&block);
}

/// A click on the notification or its Update… button (not a dismissal).
fn notification_response(action: &str) {
    if action == "com.apple.UNNotificationDismissActionIdentifier" {
        return;
    }
    if let Some(r) = available()
        && !update::installing()
    {
        show_update(&r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_layout_styles_the_right_text() {
        let lines = update::notes::parse("## New\n\n- **Gain presets.** Pick one.\n- `a`é");
        let (text, styles) = layout(&lines);
        assert_eq!(text, "New\n\n•\tGain presets. Pick one.\n•\taé");
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let slice =
            |r: NSRange| String::from_utf16(&utf16[r.location..r.location + r.length]).unwrap();
        let bold = styles
            .iter()
            .find(|(_, l)| matches!(l, Look::Bold))
            .unwrap()
            .0;
        assert_eq!(slice(bold), "Gain presets.");
        let code = styles
            .iter()
            .find(|(_, l)| matches!(l, Look::Code))
            .unwrap()
            .0;
        assert_eq!(slice(code), "a");
        assert_eq!(
            styles
                .iter()
                .filter(|(_, l)| matches!(l, Look::Bullet))
                .count(),
            2
        );
        assert!(
            styles
                .iter()
                .all(|(r, _)| r.location + r.length <= utf16.len())
        );
        let gap = styles
            .iter()
            .find(|(_, l)| matches!(l, Look::Gap))
            .unwrap()
            .0;
        assert_eq!((gap.location, slice(gap)), (4, "\n".to_string()));
    }

    #[test]
    fn progress_reads_as_text() {
        let down = |received, total| Step::Downloading { received, total };
        assert_eq!(step_text(down(5, 10), 0, false), "Downloading, 50%");
        assert_eq!(step_text(down(5, 0), 20, false), "Downloading, 25%");
        assert_eq!(step_text(down(30, 20), 0, false), "Downloading, 100%");
        assert_eq!(step_text(down(0, 0), 0, false), "Downloading");
        assert_eq!(
            step_text(Step::Waiting, 0, false),
            "Installs when you stop recording"
        );
        assert_eq!(
            step_text(Step::Waiting, 0, true),
            "Installs once the take is saved"
        );
    }

    #[test]
    fn a_missing_release_counts_as_an_answer() {
        use update::Error;
        assert!(answered(&Ok(Check::UpToDate)));
        assert!(answered(&Err(Error::Http(404))));
        assert!(!answered(&Err(Error::Http(403))));
        assert!(!answered(&Err(Error::Network("offline".into()))));
    }

    #[test]
    fn errors_read_in_lavdecks_words() {
        use update::Error;
        assert_eq!(
            error_text(&Error::Http(404)),
            "GitHub has no published release to compare with."
        );
        assert_eq!(
            error_text(&Error::Http(502)),
            "GitHub answered with HTTP status 502. Try again later."
        );
        assert_eq!(
            error_text(&Error::Network(
                "The Internet connection appears to be offline.".into()
            )),
            "Couldn’t reach GitHub. The Internet connection appears to be offline."
        );
        assert_eq!(
            error_text(&Error::Network("The download didn't arrive.".into())),
            "The download didn’t finish."
        );
        assert_eq!(
            error_text(&Error::Verify(
                "The download doesn't match the checksum GitHub lists for it.".into()
            )),
            "The download didn’t pass Lavdeck’s checks."
        );
    }

    #[test]
    fn the_menu_item_follows_the_state() {
        assert_eq!(menu_item(), (tr("Check for Updates…"), None, true));
        set_state(State::Checking);
        assert_eq!(menu_item(), (tr("Checking for Updates…"), None, false));
        set_state(State::Available(Release {
            version: update::Version::parse("v9.1.0").unwrap(),
            notes: String::new(),
            page: String::new(),
            url: String::new(),
            digest: None,
            size: 100,
        }));
        let (title, subtitle, enabled) = menu_item();
        assert_eq!(title, "Update to Lavdeck 9.1.0…");
        assert!(subtitle.is_some_and(|s| s.starts_with("You have ")) && enabled);
        set_state(State::Idle);
    }
}
