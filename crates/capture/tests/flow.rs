//! The recording flow on the main thread, without a camera, screen or permission: sources Lavdeck has no
//! permission for fail before anything starts, waits time out, stop is safe to repeat, and the preview
//! window builds. Runs without the test harness so it owns the main thread and its run loop.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use lavdeck_capture::{
    CameraBubble, CaptureError, PreviewSize, Source, VideoCapture, VideoSettings, wait_until,
};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use objc2_av_foundation::AVCaptureSession;
use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};

/// Runs the main run loop until `done` or `limit`.
fn pump(limit: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() && Instant::now() < deadline {
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, 0.01, false);
    }
}

fn unpermitted_sources_fail_before_starting(mtm: MainThreadMarker) {
    let events = Rc::new(Cell::new(0));
    let counter = events.clone();
    let capture = VideoCapture::new(
        mtm,
        |s| s.to_string(),
        move || counter.set(counter.get() + 1),
    );
    let settings = VideoSettings {
        screen: true,
        camera: true,
        ..VideoSettings::default()
    };
    let granted = HashSet::new();
    assert!(VideoCapture::expected(&settings, &granted).is_empty());
    let started = Rc::new(Cell::new(false));
    let flag = started.clone();
    capture.start(
        std::env::temp_dir().as_path(),
        "00.00.00",
        &settings,
        &granted,
        move || flag.set(true),
    );
    assert!(started.get(), "nothing to start, so start finishes at once");
    assert_eq!(events.get(), 2);
    assert!(capture.started().is_empty());
    assert_eq!(
        capture.failed(),
        vec![
            (Source::Screen, CaptureError::NotGranted(Source::Screen)),
            (Source::Camera, CaptureError::NotGranted(Source::Camera)),
        ]
    );
    // Failed sources count as settled, so Record doesn't wait for them.
    assert!(capture.is_settled(&HashSet::from([Source::Screen, Source::Camera])));

    let stopped = Rc::new(RefCell::new(None));
    let result = stopped.clone();
    capture.stop(move |r| *result.borrow_mut() = Some(r));
    let r = stopped
        .borrow_mut()
        .take()
        .expect("stop with nothing running finishes at once");
    assert!(r.outputs.is_empty() && r.problems.is_empty());
    // Stopping again hands over nothing.
    let again = Rc::new(Cell::new(false));
    let flag = again.clone();
    capture.stop(move |r| flag.set(r.outputs.is_empty() && r.problems.is_empty()));
    assert!(again.get());
    println!("ok: unpermitted sources fail before starting");
}

fn waits_time_out_and_finish_early(mtm: MainThreadMarker) {
    let capture = VideoCapture::new(mtm, |s| s.to_string(), || {});
    // A source that never starts: the wait ends at its timeout.
    let done = Rc::new(Cell::new(false));
    let flag = done.clone();
    let begun = Instant::now();
    capture.wait_for_first_frames(
        HashSet::from([Source::Camera]),
        Duration::from_millis(200),
        move || flag.set(true),
    );
    assert!(!done.get());
    pump(Duration::from_secs(3), || done.get());
    assert!(done.get());
    let waited = begun.elapsed();
    assert!(
        waited >= Duration::from_millis(200) && waited < Duration::from_secs(2),
        "{waited:?}"
    );

    // Nothing expected: done at once.
    let flag = Rc::new(Cell::new(false));
    let f = flag.clone();
    capture.wait_for_first_frames(HashSet::new(), Duration::from_secs(5), move || f.set(true));
    assert!(flag.get());

    // A condition that comes true ends the wait before its timeout.
    let ready = Rc::new(Cell::new(false));
    let finished = Rc::new(Cell::new(false));
    let (r, f) = (ready.clone(), finished.clone());
    let begun = Instant::now();
    wait_until(Duration::from_secs(5), move || r.get(), move || f.set(true));
    let r = ready.clone();
    wait_until(Duration::from_millis(100), || false, move || r.set(true));
    pump(Duration::from_secs(3), || finished.get());
    assert!(finished.get() && begun.elapsed() < Duration::from_secs(2));

    // A capture that's gone counts as settled.
    let flag = Rc::new(Cell::new(false));
    let f = flag.clone();
    capture.wait_for_first_frames(
        HashSet::from([Source::Screen]),
        Duration::from_secs(5),
        move || f.set(true),
    );
    drop(capture);
    pump(Duration::from_secs(1), || flag.get());
    assert!(flag.get());
    println!("ok: waits time out and finish early");
}

fn preview_window_builds(mtm: MainThreadMarker) {
    // An empty session: no camera is opened. The window is built but never shown.
    let session = unsafe { AVCaptureSession::new() };
    for size in PreviewSize::ALL {
        let bubble = CameraBubble::new(mtm, &session, size, |s| s.to_uppercase());
        assert!(bubble.window_number() > 0);
    }
    println!("ok: preview window builds");
}

fn main() {
    let mtm = MainThreadMarker::new().expect("runs on the main thread");
    let _app = NSApplication::sharedApplication(mtm);
    unpermitted_sources_fail_before_starting(mtm);
    waits_time_out_and_finish_early(mtm);
    preview_window_builds(mtm);
}
