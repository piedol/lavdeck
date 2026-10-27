//! Small helpers shared across modules.

use std::path::{Path, PathBuf};

use dispatch2::{DispatchQueue, DispatchTime};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSWorkspace};
use objc2_foundation::{NSArray, NSString, NSURL};

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

pub fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("UI work must run on the main thread")
}

/// Run `f` on the main queue soon.
pub fn main_async(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

/// Run `f` on the main queue after `secs` seconds.
pub fn main_after(secs: f64, f: impl FnOnce() + Send + 'static) {
    let when = DispatchTime::NOW.time((secs * 1_000_000_000.0) as i64);
    let _ = DispatchQueue::main().after(when, f);
}

/// Wraps a main-thread closure for the main queue, which needs `Send`.
struct MainOnly<F>(F);

// SAFETY: only built on the main thread (checked in `later`) and run on the main queue.
unsafe impl<F> Send for MainOnly<F> {}

/// Run a main-thread closure (one that holds AppKit objects or `Rc`s) on the main queue after
/// `secs` seconds. Call only from the main thread.
pub fn later(secs: f64, f: impl FnOnce() + 'static) {
    assert!(
        MainThreadMarker::new().is_some(),
        "later() is for the main thread"
    );
    let job = MainOnly(f);
    main_after(secs, move || {
        let job = job;
        (job.0)()
    });
}

pub fn log(msg: impl AsRef<str>) {
    if cfg!(debug_assertions) || std::env::var_os("LAVDECK_LOG").is_some() {
        eprintln!("[lavdeck] {}", msg.as_ref());
    }
}

pub fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&ns(&path.to_string_lossy()))
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `~/Recordings/Lavdeck` for `/Users/me/Recordings/Lavdeck`.
pub fn tilde(path: &Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        Ok(_) => "~".into(),
        Err(_) => path.display().to_string(),
    }
}

/// Opens a file or folder with its default app.
pub fn open(path: &Path) {
    NSWorkspace::sharedWorkspace().openURL(&file_url(path));
}

/// Opens Finder with these files selected.
pub fn reveal(paths: &[PathBuf]) {
    let urls: Vec<Retained<NSURL>> = paths.iter().map(|p| file_url(p)).collect();
    let array = NSArray::from_retained_slice(&urls);
    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&array);
}

pub fn copy_text(text: &str) {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    // SAFETY: a valid string for the standard string type.
    unsafe { board.setString_forType(&ns(text), NSPasteboardTypeString) };
}

/// Whether the user asked macOS to reduce motion (animations are skipped then).
pub fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}
