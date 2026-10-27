//! Development aid: `Lavdeck --demo --drive <dir>` works the interface the way a person
//! would, through Lavdeck's own windows only (mouse and key events sent to them, never to
//! another app), checks the model after each step, draws the windows to PNGs in `<dir>`,
//! prints PASS / FAIL lines and quits with the number of failures. Windows sit off the
//! screen's right edge and Lavdeck never activates, so a run doesn't take focus.

use std::cell::RefCell;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2_app_kit::{
    NSAlertFirstButtonReturn, NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSScreen,
    NSView, NSWindow,
};
use objc2_foundation::{NSArray, NSPoint, NSSortDescriptor};

use crate::model::status::ChannelMode;
use crate::{panel, player, recordings, settings_window, shots, status_item, store, util};

thread_local! {
    static DIR: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
    static FAILED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static DRIVING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a scripted run is going (windows don't activate Lavdeck then).
pub fn driving() -> bool {
    DRIVING.with(|d| d.get())
}

fn check(name: &str, ok: bool) {
    println!("{} {name}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        FAILED.with(|f| f.borrow_mut().push(name.to_string()));
    }
}

fn shot(window: Option<Retained<NSWindow>>, name: &str) {
    if let Some(w) = window {
        let path = DIR.with(|d| d.borrow().join(format!("{name}.png")));
        shots::write_window(&w, &path);
    }
}

fn deck_shot(name: &str) {
    shot(
        panel::window()
            .map(Retained::into_super)
            .map(Retained::into_super),
        name,
    );
}

/// Moves a window to the screen's right edge, two points showing.
fn park(window: &NSWindow) {
    if let Some(screen) = NSScreen::mainScreen(util::mtm()) {
        let f = screen.frame();
        window.setFrameOrigin(NSPoint::new(
            f.origin.x + f.size.width - 2.0,
            f.origin.y + 40.0,
        ));
    }
}

fn mouse(
    kind: NSEventType,
    window: &NSWindow,
    at: NSPoint,
    clicks: isize,
) -> Option<Retained<NSEvent>> {
    NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind,
        at,
        NSEventModifierFlags::empty(),
        0.0,
        window.windowNumber(),
        None,
        0,
        clicks,
        1.0,
    )
}

/// A click in the middle of `view`, sent to its window. Stock controls track the mouse in a
/// loop of their own, so they get `performClick:` instead.
fn click(view: &NSView) -> bool {
    if let Some(control) = view.downcast_ref::<objc2_app_kit::NSControl>() {
        // SAFETY: a plain click on a control in one of Lavdeck's windows.
        unsafe { control.performClick(None) };
        return true;
    }
    let Some(window) = view.window() else {
        return false;
    };
    let b = view.bounds();
    let mid = view.convertPoint_toView(NSPoint::new(b.size.width / 2.0, b.size.height / 2.0), None);
    let (Some(down), Some(up)) = (
        mouse(NSEventType::LeftMouseDown, &window, mid, 1),
        mouse(NSEventType::LeftMouseUp, &window, mid, 1),
    ) else {
        return false;
    };
    window.sendEvent(&down);
    window.sendEvent(&up);
    true
}

/// Drags in `view` from one local point to another.
fn drag(view: &NSView, from: NSPoint, to: NSPoint) {
    let Some(window) = view.window() else { return };
    let a = view.convertPoint_toView(from, None);
    let b = view.convertPoint_toView(to, None);
    if let Some(e) = mouse(NSEventType::LeftMouseDown, &window, a, 1) {
        window.sendEvent(&e);
    }
    for i in 1..=8 {
        let t = f64::from(i) / 8.0;
        let p = NSPoint::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        if let Some(e) = mouse(NSEventType::LeftMouseDragged, &window, p, 1) {
            window.sendEvent(&e);
        }
    }
    if let Some(e) = mouse(NSEventType::LeftMouseUp, &window, b, 1) {
        window.sendEvent(&e);
    }
}

/// Posts a key press to Lavdeck's own event queue for `window` (so local monitors see it).
fn key(window: &NSWindow, code: u16, chars: &str, mods: NSEventModifierFlags) {
    let app = NSApplication::sharedApplication(util::mtm());
    for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
        if let Some(e) = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            NSPoint::new(0.0, 0.0),
            mods,
            0.0,
            window.windowNumber(),
            None,
            &util::ns(chars),
            &util::ns(chars),
            false,
            code,
        ) {
            app.postEvent_atStart(&e, false);
        }
    }
}

fn deck_view(key: &str) -> Option<Retained<NSView>> {
    panel::view(key)
}

fn click_deck(key: &str) -> bool {
    deck_view(key).is_some_and(|v| click(&v))
}

type Step = (f64, Box<dyn FnOnce()>);

fn run_steps(mut steps: Vec<Step>) {
    if steps.is_empty() {
        let failed = FAILED.with(|f| f.borrow().clone());
        println!("{} failed", failed.len());
        std::process::exit(failed.len() as i32);
    }
    let (delay, f) = steps.remove(0);
    util::later(delay, move || {
        f();
        run_steps(steps);
    });
}

pub fn run(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    DIR.with(|d| *d.borrow_mut() = dir);
    DRIVING.with(|d| d.set(true));
    shots::set_key_look(true);
    let steps: Vec<Step> = vec![
        (
            0.5,
            Box::new(|| {
                panel::open();
                if let Some(w) = panel::window() {
                    park(&w);
                }
                check("deck opens", panel::is_open());
                deck_shot("01-deck");
            }),
        ),
        // Arming: TX2 off, then mono, which explains that TX1 records alone.
        (
            0.3,
            Box::new(|| {
                check("click OFF on TX2", click_deck("tx2-arm"));
            }),
        ),
        (
            0.2,
            Box::new(|| {
                check(
                    "TX2 disarmed",
                    store::with(|s| s.settings.armed == [1].into()),
                );
                click_deck("mode-0");
                check(
                    "mono pending",
                    store::with(|s| s.pending_channel == Some(ChannelMode::Mono)),
                );
            }),
        ),
        (
            0.6,
            Box::new(|| {
                check(
                    "receiver in mono",
                    store::with(|s| {
                        s.channel_mode() == Some(ChannelMode::Mono) && s.pending_channel.is_none()
                    }),
                );
                check("solo note shown", deck_view("note-0").is_some());
                check("mono mix label", deck_view("tx1-mix").is_some());
                deck_shot("02-mono-solo");
                click_deck("tx2-arm");
                click_deck("mode-1");
            }),
        ),
        (
            0.6,
            Box::new(|| {
                check(
                    "both armed, stereo",
                    store::with(|s| {
                        s.settings.armed == [1, 2].into()
                            && s.channel_mode() == Some(ChannelMode::Stereo)
                    }),
                );
                check("gain +12 clicked", click_deck("fader-step-0"));
            }),
        ),
        (
            0.5,
            Box::new(|| {
                check(
                    "gain is +12",
                    store::with(|s| s.status.receiver.gain_db == Some(12)),
                );
                // Drag the cap from +12 down to −12.
                if let Some(f) = deck_view("fader") {
                    let h = f.bounds().size.height;
                    drag(&f, NSPoint::new(25.0, 8.0), NSPoint::new(25.0, h - 8.0));
                }
            }),
        ),
        (
            0.5,
            Box::new(|| {
                check(
                    "fader drag to −12",
                    store::with(|s| s.status.receiver.gain_db == Some(-12)),
                );
                // Rename TX1.
                check("click TX1's name", click_deck("tx1-plate"));
            }),
        ),
        (
            0.3,
            Box::new(|| {
                let field = deck_view("tx1-field")
                    .and_then(|v| v.downcast::<objc2_app_kit::NSTextField>().ok());
                check("rename field shown", field.is_some());
                if let Some(f) = field {
                    f.setStringValue(&util::ns("Anna"));
                    if let Some(w) = f.window() {
                        w.makeFirstResponder(Some(&f));
                        key(&w, 36, "\r", NSEventModifierFlags::empty());
                    }
                }
            }),
        ),
        (
            0.4,
            Box::new(|| {
                check("TX1 renamed", store::with(|s| s.display_name(1) == "Anna"));
                check("deck still open", panel::is_open());
                check("which mic is which", click_deck("identify-button"));
            }),
        ),
        (
            1.5,
            Box::new(|| {
                check("identifying", store::with(|s| s.identifying));
                deck_shot("03-identify");
                if let Some(w) = panel::window() {
                    key(&w, 53, "\u{1b}", NSEventModifierFlags::empty());
                }
            }),
        ),
        (
            0.4,
            Box::new(|| {
                check("Escape ends identify", store::with(|s| !s.identifying));
                check("Record clicked", click_deck("record"));
                check("starting", store::with(|s| s.is_starting || s.is_recording));
            }),
        ),
        (
            2.5,
            Box::new(|| {
                check("recording", store::with(|s| s.is_recording));
                check(
                    "glyph shows recording",
                    store::with(|s| s.glyph() == crate::model::Glyph::Recording),
                );
                click_deck("tx1-arm");
                check(
                    "arming locked while recording",
                    store::with(|s| s.settings.armed == [1, 2].into()),
                );
                deck_shot("04-recording");
                let menu = status_item::make_menu();
                check(
                    "menu offers Stop Recording",
                    menu.itemArray()
                        .iter()
                        .any(|i| i.title().to_string() == crate::i18n::tr("Stop Recording")),
                );
                if let Some(w) = panel::window() {
                    key(
                        &w,
                        15,
                        "r",
                        NSEventModifierFlags::Command | NSEventModifierFlags::Option,
                    );
                }
            }),
        ),
        (
            0.8,
            Box::new(|| {
                check("⌥⌘R stops", store::with(|s| !s.is_recording));
                check(
                    "transcription runs after the take",
                    store::with(|s| !s.plugins.running_jobs().is_empty()),
                );
                deck_shot("05-after-take");
                let menu = status_item::make_menu();
                let titles: Vec<String> = menu
                    .itemArray()
                    .iter()
                    .map(|i| i.title().to_string())
                    .collect();
                println!("menu: {}", titles.join(" | "));
                check(
                    "menu has Mono, Stereo, Recordings, Settings, Quit",
                    ["Mono", "Stereo", "Recordings…", "Settings…", "Quit Lavdeck"]
                        .iter()
                        .all(|t| titles.iter().any(|x| x == t)),
                );
                panel::close();
                recordings::show();
                if let Some(w) = recordings::with(|s| s.window.clone()) {
                    park(&w);
                }
            }),
        ),
        (
            1.0,
            Box::new(|| {
                let (rows, takes) =
                    recordings::with(|s| (s.rows.len(), s.takes.len())).unwrap_or((0, 0));
                check("new take listed", rows == 19 && takes == 19);
                check(
                    "newest selected",
                    recordings::with(|s| s.selection.len() == 1).unwrap_or(false),
                );
                shot(recordings::with(|s| s.window.clone()), "06-recordings");
                set_search("rug");
            }),
        ),
        (
            0.3,
            Box::new(|| {
                let ids: Vec<String> =
                    recordings::with(|s| s.rows.iter().map(|r| r.take.id.clone()).collect())
                        .unwrap_or_default();
                let all_match = recordings::with(|s| {
                    s.rows.iter().all(|r| {
                        s.transcripts
                            .get(&r.take.id)
                            .is_some_and(|d| d.plain_text().to_lowercase().contains("rug"))
                    })
                })
                .unwrap_or(false);
                check(
                    "search finds transcript words",
                    ids.iter().any(|id| id.ends_with("11.05.09")) && all_match,
                );
                println!("search 'rug': {ids:?}");
                shot(recordings::with(|s| s.window.clone()), "07-search");
                set_search("no such words anywhere");
            }),
        ),
        (
            0.3,
            Box::new(|| {
                check(
                    "no results says so",
                    recordings::overlay_view_for_drive("unavailable").is_some(),
                );
                shot(recordings::with(|s| s.window.clone()), "07b-no-results");
                set_search("");
                check(
                    "results come back",
                    recordings::overlay_view_for_drive("unavailable").is_none(),
                );
                recordings::set_filter(crate::model::recordings::Filter::Untranscribed);
            }),
        ),
        (
            0.3,
            Box::new(|| {
                let ok = recordings::with(|s| s.rows.iter().all(|r| !r.take.has_transcript()))
                    .unwrap_or(false);
                check("filter: not transcribed", ok);
                recordings::set_filter(crate::model::recordings::Filter::All);
                sort_by("duration", true);
            }),
        ),
        (
            0.3,
            Box::new(|| {
                let lengths: Vec<f64> =
                    recordings::with(|s| s.rows.iter().map(|r| r.duration()).collect())
                        .unwrap_or_default();
                check("sort by length", lengths.windows(2).all(|w| w[0] <= w[1]));
                sort_by("started", false);
                select_rows(&[0, 1, 2]);
            }),
        ),
        (
            0.4,
            Box::new(|| {
                check(
                    "three selected",
                    recordings::with(|s| recordings::selected_takes(s).len() == 3).unwrap_or(false),
                );
                shot(recordings::with(|s| s.window.clone()), "08-selection");
                select_take("11.05.09");
            }),
        ),
        (
            0.4,
            Box::new(|| {
                recordings::play_selected();
            }),
        ),
        (
            1.5,
            Box::new(|| {
                check("playing", player::with(|p| p.playing));
                check("playhead moves", player::with(|p| p.time > 0.5));
                shot(recordings::with(|s| s.window.clone()), "09-playing");
                // Click the fourth transcript line: playback jumps there.
                let line = recordings::with(|s| s.detail.clone()).and_then(|p| {
                    let take =
                        recordings::with(|s| recordings::selected_takes(s).into_iter().next())
                            .flatten()?;
                    let doc =
                        recordings::with(|s| s.transcripts.get(&take.id).cloned()).flatten()?;
                    let seg = doc.segments.get(3)?.clone();
                    Some((p, seg))
                });
                if let Some((p, seg)) = line {
                    let view = p.body_view(&format!("line-{}", seg.id()));
                    check("transcript line clicked", view.is_some_and(|v| click(&v)));
                    util::later(0.3, move || {
                        check(
                            "plays from the line",
                            player::with(|p| (p.time - seg.start).abs() < 1.0),
                        )
                    });
                }
            }),
        ),
        (
            1.0,
            Box::new(|| {
                if let Some(w) = recordings::with(|s| s.window.clone()) {
                    w.makeFirstResponder(None);
                    key(&w, 49, " ", NSEventModifierFlags::empty());
                }
            }),
        ),
        (
            0.4,
            Box::new(|| {
                check("Space pauses", player::with(|p| !p.playing));
                shot(recordings::with(|s| s.window.clone()), "10-paused");
                let doomed =
                    recordings::with(|s| recordings::selected_takes(s)).unwrap_or_default();
                recordings::ask_to_delete(Some(doomed));
            }),
        ),
        (
            0.6,
            Box::new(|| {
                let window = recordings::with(|s| s.window.clone());
                let sheet = window.as_ref().and_then(|w| w.attachedSheet());
                check("asks before Trash", sheet.is_some());
                if let (Some(w), Some(sheet)) = (window, sheet) {
                    w.endSheet_returnCode(&sheet, NSAlertFirstButtonReturn);
                }
            }),
        ),
        (
            0.5,
            Box::new(|| {
                // A take with a finished video plays in the video view.
                select_take("09.15.02");
            }),
        ),
        (
            0.6,
            Box::new(|| {
                let video =
                    recordings::with(|s| s.detail.clone()).and_then(|p| p.body_view("video"));
                check("video view shown", video.is_some());
                check(
                    "finished video loaded",
                    player::with(|p| {
                        p.media
                            .as_ref()
                            .is_some_and(|m| m.kind == crate::model::takes::MediaKind::Video)
                    }),
                );
                recordings::play_selected();
            }),
        ),
        (
            1.2,
            Box::new(|| {
                check("video plays", player::with(|p| p.playing && p.time > 0.3));
                shot(recordings::with(|s| s.window.clone()), "10b-video");
                // A camera-only take: the raw recording plays with the mic audio, and Make Video makes one.
                select_take("16.48.30");
            }),
        ),
        (
            0.8,
            Box::new(|| {
                check(
                    "raw camera loaded",
                    player::with(|p| {
                        p.media
                            .as_ref()
                            .is_some_and(|m| m.kind == crate::model::takes::MediaKind::Camera)
                            && p.error.is_none()
                    }),
                );
                let make =
                    recordings::with(|s| s.detail.clone()).and_then(|p| p.body_view("video-make"));
                check(
                    "Make Video offered",
                    make.as_ref().is_some_and(|m| click(m)),
                );
                shot(recordings::with(|s| s.window.clone()), "10c-raw-camera");
            }),
        ),
        (
            3.2,
            Box::new(|| {
                let made = recordings::with(|s| {
                    s.takes
                        .iter()
                        .find(|t| t.stem == "16.48.30")
                        .is_some_and(|t| t.finished_video().is_some())
                })
                .unwrap_or(false);
                check("finished video made", made);
                check(
                    "player switches to it",
                    player::with(|p| {
                        p.media
                            .as_ref()
                            .is_some_and(|m| m.kind == crate::model::takes::MediaKind::Video)
                    }),
                );
                player::stop();
                // The deck with no receiver.
                panel::open();
                if let Some(w) = panel::window() {
                    park(&w);
                }
                store::update(|s| s.is_connected = false);
            }),
        ),
        (
            0.4,
            Box::new(|| {
                check(
                    "absent glyph",
                    store::with(|s| s.glyph() == crate::model::Glyph::Absent),
                );
                check("absent panel", deck_view("absent").is_some());
                deck_shot("12-absent");
                panel::close();
                store::update(|s| s.is_connected = true);
            }),
        ),
        (
            0.5,
            Box::new(|| {
                let gone = recordings::with(|s| !s.takes.iter().any(|t| t.stem == "11.05.09"))
                    .unwrap_or(false);
                check("take moved to Trash", gone);
                check(
                    "next take selected",
                    recordings::with(|s| s.selection.len() == 1).unwrap_or(false),
                );
                settings_window::show();
                settings_window::park_for_drive();
            }),
        ),
        (
            0.6,
            Box::new(|| {
                let before = store::with(|s| s.settings.split_tracks);
                settings_window::click_for_drive("split");
                util::later(0.2, move || {
                    check(
                        "split tracks switch",
                        store::with(|s| s.settings.split_tracks != before),
                    )
                });
            }),
        ),
        (
            0.5,
            Box::new(|| {
                settings_window::select_tab_for_drive(2);
            }),
        ),
        (
            0.6,
            Box::new(|| {
                let before =
                    store::with(|s| s.plugins.is_enabled("io.github.piedol.lavdeck.studio"));
                settings_window::click_for_drive("enable-io.github.piedol.lavdeck.studio");
                util::later(0.2, move || {
                    check(
                        "plugin switch",
                        store::with(|s| {
                            s.plugins.is_enabled("io.github.piedol.lavdeck.studio") != before
                        }),
                    );
                    settings_window::shot_for_drive(
                        &DIR.with(|d| d.borrow().join("11-settings-plugins.png")),
                    );
                });
            }),
        ),
        (0.6, Box::new(|| {})),
    ];
    run_steps(steps);
}

fn set_search(text: &str) {
    recordings::search_for_drive(text);
}

fn sort_by(key: &str, ascending: bool) {
    let d = NSSortDescriptor::sortDescriptorWithKey_ascending(Some(&util::ns(key)), ascending);
    recordings::sort_for_drive(&NSArray::from_retained_slice(&[d]));
}

fn select_rows(rows: &[usize]) {
    recordings::select_rows_for_drive(rows);
}

fn select_take(stem: &str) {
    if let Some(id) = recordings::with(|s| {
        s.takes
            .iter()
            .find(|t| t.stem == stem)
            .map(|t| t.id.clone())
    })
    .flatten()
    {
        recordings::select_only(&id);
    }
}
