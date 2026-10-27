//! Records the screen and camera for one take, alongside Lavdeck's audio, with the camera preview.
//!
//! Video starts first: a camera takes about a second to deliver its first frame, so starting it after the
//! audio left the first second of every video black. Start the capture, wait (up to
//! [`FIRST_FRAME_TIMEOUT`]) for each source's first frame, then start the audio; each file's first-frame
//! host time, minus the audio's, is its offset in `.take.json` ([`CaptureOutput::offset`]).
//!
//! Everything here runs on the main thread. Framework callbacks hop back to it by capture id, so no
//! main-thread state ever crosses threads.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;

use crate::bubble::CameraBubble;
use crate::camera::CameraRecorder;
use crate::error::CaptureError;
use crate::screen::{RecordedFile, ScreenRecorder};
use crate::settings::VideoSettings;
use crate::take::CaptureOutput;
use crate::util::{main_after, main_async};

/// How long Record waits for each source's first frame before starting the audio anyway.
pub const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// The permission names Lavdeck's permission center uses.
pub const PERMISSION_CAMERA: &str = "camera";
pub const PERMISSION_SCREEN_RECORDING: &str = "screen-recording";

/// What Lavdeck records video from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    Screen,
    Camera,
}

impl Source {
    /// The name in capture events and `.take.json` (`screen`, `camera`).
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Screen => "screen",
            Source::Camera => "camera",
        }
    }
}

/// What a take's video recording left behind.
#[derive(Debug, Default)]
pub struct StopResult {
    /// The files written, screen first, as the capture host reports them (plugin id `lavdeck`).
    pub outputs: Vec<CaptureOutput>,
    /// Sources that recorded nothing or couldn't finish their file. The app reports each as a problem
    /// from "Video".
    pub problems: Vec<CaptureError>,
}

struct Stopping {
    screen: Option<Result<RecordedFile, CaptureError>>,
    camera: Option<Result<RecordedFile, CaptureError>>,
    waiting: usize,
    done: Box<dyn FnOnce(StopResult)>,
}

struct Inner {
    id: u64,
    started: Vec<Source>,
    failed: BTreeMap<Source, CaptureError>,
    screen: Option<ScreenRecorder>,
    camera: Option<CameraRecorder>,
    bubble: Option<CameraBubble>,
    /// The screen recorder is still starting on its thread.
    screen_starting: bool,
    start_done: Option<Box<dyn FnOnce()>>,
    stopping: Option<Stopping>,
    finished: bool,
    on_event: Rc<dyn Fn()>,
    translate: fn(&str) -> String,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Live captures by id, for callbacks arriving from framework queues.
    static LIVE: RefCell<HashMap<u64, Weak<RefCell<Inner>>>> = RefCell::new(HashMap::new());
    /// Captures with a stop in flight, kept until their files are handed over even if the app has let
    /// go of them (as Swift's stop task kept its capture).
    static STOPPING: RefCell<HashMap<u64, Rc<RefCell<Inner>>>> = RefCell::new(HashMap::new());
}

fn live(id: u64) -> Option<Rc<RefCell<Inner>>> {
    LIVE.with(|live| live.borrow().get(&id).and_then(Weak::upgrade))
}

/// Runs `f` on the main thread with the capture `id`, if it still exists.
fn on_main(id: u64, f: impl FnOnce(&Rc<RefCell<Inner>>) + Send + 'static) {
    main_async(move || {
        if let Some(inner) = live(id) {
            f(&inner);
        }
    });
}

/// Records the screen and camera for one take. Main thread only.
pub struct VideoCapture {
    id: u64,
    inner: Rc<RefCell<Inner>>,
}

impl VideoCapture {
    /// `on_event` runs whenever a source starts or fails, e.g. to update the deck; `translate` words the
    /// camera preview's hints (the app's `tr`).
    pub fn new(
        _mtm: MainThreadMarker,
        translate: fn(&str) -> String,
        on_event: impl Fn() + 'static,
    ) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let inner = Rc::new(RefCell::new(Inner {
            id,
            started: Vec::new(),
            failed: BTreeMap::new(),
            screen: None,
            camera: None,
            bubble: None,
            screen_starting: false,
            start_done: None,
            stopping: None,
            finished: false,
            on_event: Rc::new(on_event),
            translate,
        }));
        LIVE.with(|live| live.borrow_mut().insert(id, Rc::downgrade(&inner)));
        VideoCapture { id, inner }
    }

    /// The sources `settings` will try to record, given the permissions Lavdeck holds.
    pub fn expected(settings: &VideoSettings, granted: &HashSet<String>) -> HashSet<Source> {
        let mut out = HashSet::new();
        if settings.camera && granted.contains(PERMISSION_CAMERA) {
            out.insert(Source::Camera);
        }
        if settings.screen && granted.contains(PERMISSION_SCREEN_RECORDING) {
            out.insert(Source::Screen);
        }
        out
    }

    /// Sources that have written their first frame, in the order they did.
    pub fn started(&self) -> Vec<Source> {
        self.inner.borrow().started.clone()
    }

    /// Sources that couldn't start, with why.
    pub fn failed(&self) -> Vec<(Source, CaptureError)> {
        self.inner
            .borrow()
            .failed
            .iter()
            .map(|(s, e)| (*s, e.clone()))
            .collect()
    }

    /// Whether every source in `expected` has started or failed.
    pub fn is_settled(&self, expected: &HashSet<Source>) -> bool {
        settled(&self.inner.borrow(), expected)
    }

    /// Starts the camera (and its preview) and the screen in `folder` as `<stem> Camera.mov` and
    /// `<stem> Screen.mov`. `done` runs once each has started or failed to; use [`Self::started`] or
    /// [`Self::wait_for_first_frames`] for first frames.
    pub fn start(
        &self,
        folder: &Path,
        stem: &str,
        settings: &VideoSettings,
        granted: &HashSet<String>,
        done: impl FnOnce() + 'static,
    ) {
        let mtm = MainThreadMarker::new().expect("VideoCapture runs on the main thread");
        let camera_on = settings.camera && granted.contains(PERMISSION_CAMERA);
        let screen_on = settings.screen && granted.contains(PERMISSION_SCREEN_RECORDING);
        if settings.camera && !camera_on {
            self.fail(Source::Camera, CaptureError::NotGranted(Source::Camera));
        }
        if settings.screen && !screen_on {
            self.fail(Source::Screen, CaptureError::NotGranted(Source::Screen));
        }
        if camera_on {
            let path = folder.join(format!("{stem} Camera.mov"));
            let id = self.id;
            let made = CameraRecorder::make(&path, settings.hevc(), move |_| {
                on_main(id, |inner| did_start(inner, Source::Camera));
            });
            match made {
                Ok(recorder) => {
                    if settings.camera_preview {
                        let translate = self.inner.borrow().translate;
                        let bubble = CameraBubble::new(
                            mtm,
                            &recorder.session,
                            settings.preview_size,
                            translate,
                        );
                        bubble.show();
                        self.inner.borrow_mut().bubble = Some(bubble);
                    }
                    recorder.start();
                    self.inner.borrow_mut().camera = Some(recorder);
                }
                Err(error) => self.fail(Source::Camera, error),
            }
        }
        if !screen_on {
            return done();
        }
        let path = folder.join(format!("{stem} Screen.mov"));
        let excluding: Vec<isize> = self
            .inner
            .borrow()
            .bubble
            .iter()
            .map(CameraBubble::window_number)
            .collect();
        let (fps, hevc, system_audio) =
            (settings.screen_fps, settings.hevc(), settings.system_audio);
        let id = self.id;
        {
            let mut inner = self.inner.borrow_mut();
            inner.screen_starting = true;
            inner.start_done = Some(Box::new(done));
        }
        std::thread::spawn(move || {
            let result =
                ScreenRecorder::start(&path, fps, hevc, system_audio, &excluding, move |_| {
                    on_main(id, |inner| did_start(inner, Source::Screen));
                });
            main_async(move || {
                let Some(inner) = live(id) else {
                    // The capture is gone: stop what started, keep nothing.
                    if let Ok(recorder) = result {
                        recorder.stop(|_| {});
                    }
                    return;
                };
                screen_started(id, &inner, result);
            });
        });
    }

    /// Calls `done` once every source in `expected` has written its first frame or failed, or after
    /// `timeout` ([`FIRST_FRAME_TIMEOUT`] at Record).
    pub fn wait_for_first_frames(
        &self,
        expected: HashSet<Source>,
        timeout: Duration,
        done: impl FnOnce() + 'static,
    ) {
        let weak = Rc::downgrade(&self.inner);
        wait_until(
            timeout,
            move || {
                weak.upgrade()
                    .is_none_or(|inner| settled(&inner.borrow(), &expected))
            },
            done,
        );
    }

    /// Stops everything and hands over the files written. Safe to call more than once: later calls get
    /// nothing.
    pub fn stop(&self, done: impl FnOnce(StopResult) + 'static) {
        let mut inner = self.inner.borrow_mut();
        if inner.finished {
            drop(inner);
            return done(StopResult::default());
        }
        inner.finished = true;
        if let Some(bubble) = inner.bubble.take() {
            bubble.close();
        }
        let screen = inner.screen.take();
        let camera = inner.camera.take();
        let waiting =
            screen.is_some() as usize + camera.is_some() as usize + inner.screen_starting as usize;
        inner.stopping = Some(Stopping {
            screen: None,
            camera: None,
            waiting,
            done: Box::new(done),
        });
        drop(inner);
        if waiting > 0 {
            STOPPING.with(|s| s.borrow_mut().insert(self.id, self.inner.clone()));
        }
        if let Some(screen) = screen {
            self.stop_screen(screen);
        }
        if let Some(camera) = camera {
            let id = self.id;
            camera.stop(move |result| {
                on_main(id, move |inner| {
                    stopped(inner, Source::Camera, Some(result))
                })
            });
        }
        if waiting == 0 {
            finish_stop(&self.inner);
        }
    }

    fn stop_screen(&self, screen: ScreenRecorder) {
        stop_screen(self.id, screen);
    }

    fn fail(&self, source: Source, error: CaptureError) {
        fail(&self.inner, source, error);
    }
}

impl Drop for VideoCapture {
    fn drop(&mut self) {
        // A stop in flight still needs its callbacks; finish_stop forgets the capture after.
        if !STOPPING.with(|s| s.borrow().contains_key(&self.id)) {
            LIVE.with(|live| live.borrow_mut().remove(&self.id));
        }
    }
}

fn settled(inner: &Inner, expected: &HashSet<Source>) -> bool {
    expected
        .iter()
        .all(|s| inner.started.contains(s) || inner.failed.contains_key(s))
}

fn notify(inner: &Rc<RefCell<Inner>>) {
    let on_event = inner.borrow().on_event.clone();
    on_event();
}

fn did_start(inner: &Rc<RefCell<Inner>>, source: Source) {
    {
        let mut i = inner.borrow_mut();
        if !i.started.contains(&source) {
            i.started.push(source);
        }
    }
    notify(inner);
}

fn fail(inner: &Rc<RefCell<Inner>>, source: Source, error: CaptureError) {
    inner.borrow_mut().failed.insert(source, error);
    notify(inner);
}

fn screen_started(
    id: u64,
    inner: &Rc<RefCell<Inner>>,
    result: Result<ScreenRecorder, CaptureError>,
) {
    let (done, stopping) = {
        let mut i = inner.borrow_mut();
        i.screen_starting = false;
        (i.start_done.take(), i.stopping.is_some())
    };
    match result {
        // Stopped while starting: finish it now, so its file is reported with the rest.
        Ok(recorder) if stopping => stop_screen(id, recorder),
        Ok(recorder) => inner.borrow_mut().screen = Some(recorder),
        Err(error) => {
            fail(inner, Source::Screen, error);
            // It never started, so the stop has no screen file to wait for; the failure is reported
            // above, as when it fails before a stop.
            if stopping {
                stopped(inner, Source::Screen, None);
            }
        }
    }
    if let Some(done) = done {
        done();
    }
}

fn stop_screen(id: u64, screen: ScreenRecorder) {
    screen.stop(move |result| {
        on_main(id, move |inner| {
            stopped(inner, Source::Screen, Some(result))
        })
    });
}

/// One source has finished (`None`: it never started).
fn stopped(
    inner: &Rc<RefCell<Inner>>,
    source: Source,
    result: Option<Result<RecordedFile, CaptureError>>,
) {
    let ready = {
        let mut i = inner.borrow_mut();
        let Some(stopping) = i.stopping.as_mut() else {
            return;
        };
        match source {
            Source::Screen => stopping.screen = result,
            Source::Camera => stopping.camera = result,
        }
        stopping.waiting = stopping.waiting.saturating_sub(1);
        stopping.waiting == 0
    };
    if ready {
        finish_stop(inner);
    }
}

fn finish_stop(inner: &Rc<RefCell<Inner>>) {
    let Some(stopping) = inner.borrow_mut().stopping.take() else {
        return;
    };
    let mut result = StopResult::default();
    for file in [stopping.screen, stopping.camera].into_iter().flatten() {
        match file {
            Ok(file) => result.outputs.push(CaptureOutput {
                plugin_id: "lavdeck".into(),
                path: file.path,
                source: file.source.into(),
                kind: "video".into(),
                start_host_time: file.start_host_time,
            }),
            Err(error) => result.problems.push(error),
        }
    }
    (stopping.done)(result);
    // Nothing calls back once the files are handed over.
    let id = inner.borrow().id;
    LIVE.with(|live| live.borrow_mut().remove(&id));
    STOPPING.with(|s| s.borrow_mut().remove(&id));
}

struct Waiter {
    deadline: Instant,
    ready: Box<dyn Fn() -> bool>,
    done: Box<dyn FnOnce()>,
}

thread_local! {
    static WAITERS: RefCell<HashMap<u64, Waiter>> = RefCell::new(HashMap::new());
}

/// Calls `done` on the main thread once `ready()` holds (checked every 20 ms) or `timeout` passes. For
/// waiting on Lavdeck's sources and capture plugins' together before starting the audio.
pub fn wait_until(
    timeout: Duration,
    ready: impl Fn() -> bool + 'static,
    done: impl FnOnce() + 'static,
) {
    if ready() {
        return done();
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let waiter = Waiter {
        deadline: Instant::now() + timeout,
        ready: Box::new(ready),
        done: Box::new(done),
    };
    WAITERS.with(|w| w.borrow_mut().insert(id, waiter));
    poll_later(id);
}

fn poll_later(id: u64) {
    main_after(Duration::from_millis(20), move || {
        let Some(waiter) = WAITERS.with(|w| w.borrow_mut().remove(&id)) else {
            return;
        };
        if Instant::now() >= waiter.deadline || (waiter.ready)() {
            (waiter.done)();
        } else {
            WAITERS.with(|w| w.borrow_mut().insert(id, waiter));
            poll_later(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expects_only_sources_with_permission() {
        let settings = VideoSettings {
            screen: true,
            camera: true,
            ..VideoSettings::default()
        };
        let granted = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<HashSet<_>>();
        assert_eq!(
            VideoCapture::expected(&settings, &granted(&["camera", "screen-recording"])),
            HashSet::from([Source::Screen, Source::Camera])
        );
        assert_eq!(
            VideoCapture::expected(&settings, &granted(&["camera"])),
            HashSet::from([Source::Camera])
        );
        let off = VideoSettings {
            screen: false,
            ..settings
        };
        assert!(VideoCapture::expected(&off, &granted(&["screen-recording"])).is_empty());
    }
}
