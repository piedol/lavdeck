//! The real backend: the receiver, recorder, take library and plugins from `lavdeck-kit`, and
//! screen and camera recording from `lavdeck-capture`. It ports the device and recording
//! orchestration of `AppModel.swift` and the job running of `PluginHost.swift`.
//!
//! Kit calls that block (receiver writes, the microphone prompt, splitting tracks, stopping
//! capture plugins, plugin jobs, making the finished video) run on background threads, and
//! their results come back to the main thread, where all of this lives.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use lavdeck_capture::{FIRST_FRAME_TIMEOUT, VideoCapture, VideoComposer};
use lavdeck_kit::{
    self as kit, ButtonMonitor, CaptureEventKind, CaptureHost, CaptureResult, CaptureTake,
    MicButton, MicController, MicRecorder, PluginMessageKind, PluginStore, TakeMetadata, TakeMic,
    TalkingDetector,
};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSURL};

use crate::backend::{Backend, Done, RecordRequest};
use crate::i18n::{tr, trf, trn};
use crate::model::plugins::{CANCELLED, JobState, JsonValue, Launch, Plugin, Work};
use crate::model::settings::Settings;
use crate::model::status::ChannelMode;
use crate::model::takes::{Take, TranscriptDocument};
use crate::util::{self, ns};
use crate::{convert, errors, permissions, store};

/// Plugins Lavdeck has absorbed: a copy still installed is ignored, so it can't run twice.
const BUILT_IN: &[&str] = &["io.github.piedol.lavdeck.capture"];
/// How long capture plugins get to finish writing after Stop.
const CAPTURE_STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// A running job: a plugin process, or Lavdeck's own video export.
enum JobHandle {
    Plugin(kit::PluginJob),
    Video(Arc<AtomicBool>),
}

/// The take in progress, from Record until its files are saved.
struct Running {
    folder: PathBuf,
    stem: String,
    /// Where the audio goes: the take's file, or a scratch stereo file for one mic alone.
    target: PathBuf,
    solo: Option<u8>,
    mode: Option<ChannelMode>,
    format: crate::model::settings::RecordingFormat,
    split: bool,
    started: SystemTime,
    native: Option<VideoCapture>,
    host: Option<Arc<CaptureHost>>,
    /// Sources that reported an error before their first frame, so Record doesn't wait on them.
    failed: HashSet<String>,
    /// What capture handed over at Stop, once it has; `None` while it's still stopping.
    captured: Rc<RefCell<Option<CaptureResult>>>,
    stopping: bool,
}

pub struct Live {
    controller: MicController,
    recorder: MicRecorder,
    store: PluginStore,
    plugins: RefCell<Vec<kit::InstalledPlugin>>,
    buttons: RefCell<Option<ButtonMonitor>>,
    tap_to_record: Cell<Option<bool>>,
    jobs: RefCell<HashMap<u64, JobHandle>>,
    take: RefCell<Option<Running>>,
    /// Record's caller, until the audio starts or the start fails.
    starting: RefCell<Option<Done>>,
    detector: RefCell<TalkingDetector>,
    was_identifying: Cell<bool>,
}

thread_local! {
    static LIVE: RefCell<Option<Rc<Live>>> = const { RefCell::new(None) };
}

fn with_live(f: impl FnOnce(&Rc<Live>)) {
    if let Some(live) = LIVE.with(|l| l.borrow().clone()) {
        f(&live);
    }
}

impl Live {
    pub fn new() -> Rc<Live> {
        let live = Rc::new(Live {
            controller: MicController::new(),
            recorder: MicRecorder::new(),
            store: PluginStore::new(),
            plugins: RefCell::new(Vec::new()),
            buttons: RefCell::new(None),
            tap_to_record: Cell::new(None),
            jobs: RefCell::new(HashMap::new()),
            take: RefCell::new(None),
            starting: RefCell::new(None),
            detector: RefCell::new(TalkingDetector::new()),
            was_identifying: Cell::new(false),
        });
        LIVE.with(|l| *l.borrow_mut() = Some(live.clone()));
        live.controller.set_on_status(|status| {
            util::main_async(move || with_live(|l| l.status(&status)));
        });
        live.controller.set_on_connection_change(|connected| {
            util::main_async(move || store::receiver_connected(connected));
        });
        live.recorder.set_on_finish(|path, error| {
            util::main_async(move || with_live(|l| l.audio_finished(path, error)));
        });
        live
    }

    /// A status push: identify's talking detector, then the model.
    fn status(&self, status: &kit::MicStatus) {
        let s = convert::status(status);
        let (identifying, stereo) =
            store::with(|m| (m.identifying, m.channel_mode() == Some(ChannelMode::Stereo)));
        if identifying && !self.was_identifying.get() {
            self.detector.borrow_mut().reset();
        }
        self.was_identifying.set(identifying);
        store::receiver_status(s.clone(), status.last_push.is_some());
        if identifying && s.receiver.channel_mode == Some(ChannelMode::Stereo) && stereo {
            let meter = |i: u8| s.transmitters.get(&i).and_then(|t| t.meter_level);
            let talking = self.detector.borrow_mut().update(meter(1), meter(2));
            store::set_talking(talking.and_then(|t| u8::try_from(t).ok()));
        }
    }

    /// Starts or stops watching the transmitters' link button (tap to record).
    fn restart_buttons(&self, on: bool) {
        if self.tap_to_record.replace(Some(on)) == Some(on) {
            return;
        }
        if let Some(mut b) = self.buttons.borrow_mut().take() {
            b.stop();
        }
        if !on {
            return;
        }
        // Seizing the button stops it changing the Mac's volume while it records instead.
        let mut monitor = ButtonMonitor::new(true);
        monitor.set_on_event(|event| {
            if event.is_down && event.button == MicButton::VolumeUp {
                util::main_async(store::toggle_recording);
            }
        });
        let result = monitor.start();
        if result != 0 {
            util::log(format!("buttons: IOHIDManagerOpen returned {result:#x}"));
        }
        *self.buttons.borrow_mut() = Some(monitor);
    }

    fn plugin(&self, id: &str) -> Option<kit::InstalledPlugin> {
        self.plugins.borrow().iter().find(|p| p.id() == id).cloned()
    }

    // MARK: Starting a take

    fn fail_start(&self, message: String) {
        self.stop_capture();
        self.take.borrow_mut().take();
        if let Some(done) = self.starting.borrow_mut().take() {
            done(Err(message));
        }
    }

    /// After the microphone question: folders and names, then capture, then audio.
    fn begin(self: &Rc<Self>, request: RecordRequest, allowed: bool) {
        if !allowed {
            return self.fail_start(tr(
                "Microphone access is off. Allow it in System Settings › Privacy & Security › Microphone.",
            ));
        }
        let now = SystemTime::now();
        let (day, stem) = kit::takes::day_and_stem(now);
        let folder = request.root.join(&day);
        if let Err(e) = std::fs::create_dir_all(&folder) {
            return self.fail_start(trf(
                "Couldn’t start recording: {error}",
                &[("error", &e.to_string())],
            ));
        }
        let ext = request.format.key();
        let solo = (request.armed.len() == 1).then(|| request.armed[0]);
        let target = if solo.is_some() {
            // Record stereo to a scratch file, keep only the armed mic's channel at the end.
            let id = objc2_foundation::NSUUID::new().UUIDString().to_string();
            std::env::temp_dir().join(format!("lavdeck-{id}.{ext}"))
        } else {
            // Mono mixes both mics into L and R, so the take is named for what it is.
            let kind = if request.mode == Some(ChannelMode::Stereo) {
                "Stereo"
            } else {
                "Mix"
            };
            folder.join(format!("{stem} {kind}.{ext}"))
        };
        *self.take.borrow_mut() = Some(Running {
            folder: folder.clone(),
            stem: stem.clone(),
            target,
            solo,
            mode: request.mode,
            format: request.format,
            split: request.split_tracks,
            started: now,
            native: None,
            host: None,
            failed: HashSet::new(),
            captured: Rc::new(RefCell::new(None)),
            stopping: false,
        });
        // Video first: a camera takes about a second to deliver its first frame, so starting it
        // after the audio left the first second of every video black. Start capture, wait for
        // each source's first frame, then start the audio.
        let this = self.clone();
        self.start_capture(&request.settings, &folder, &stem, move |expected| {
            this.wait_for_capture(expected, Instant::now() + FIRST_FRAME_TIMEOUT);
        });
    }

    /// Lavdeck's own screen and camera, then capture plugins; `then` gets the sources expected
    /// to start (switched on, permission granted).
    fn start_capture(
        self: &Rc<Self>,
        settings: &Settings,
        folder: &Path,
        stem: &str,
        then: impl FnOnce(HashSet<String>) + 'static,
    ) {
        let video = settings.video.clone();
        let this = self.clone();
        let (f, s) = (folder.to_path_buf(), stem.to_string());
        let after_native =
            move |expected: HashSet<String>| this.start_plugins(&f, &s, expected, then);
        if !video.records_video() {
            return after_native(HashSet::new());
        }
        let needed: Vec<&'static str> = [
            video.screen.then_some(permissions::SCREEN),
            video.camera.then_some(permissions::CAMERA),
        ]
        .into_iter()
        .flatten()
        .collect();
        let this = self.clone();
        let (folder, stem) = (folder.to_path_buf(), stem.to_string());
        let asked = needed.clone();
        permissions::request_then(&asked, move || {
            let granted: HashSet<String> = permissions::granted(&needed).into_iter().collect();
            if let Some(missing) = needed.iter().find(|p| !granted.contains(**p)) {
                store::capture_failed(permission_message(missing));
            }
            let capture_settings = convert::video(&video);
            let native = VideoCapture::new(util::mtm(), errors::preview_hint, || {
                util::later(0.0, || with_live(|l| l.native_changed()))
            });
            let expected: HashSet<String> = VideoCapture::expected(&capture_settings, &granted)
                .into_iter()
                .map(|s| s.as_str().to_string())
                .collect();
            // Kept with the take first, so waiting for first frames sees it capturing.
            let mut take = this.take.borrow_mut();
            let Some(r) = take.as_mut() else { return };
            let native = r.native.insert(native);
            native.start(&folder, &stem, &capture_settings, &granted, move || {
                util::later(0.0, move || after_native(expected))
            });
        });
    }

    /// Capture plugins, under Lavdeck's grants (asked for once, for every plugin).
    fn start_plugins(
        self: &Rc<Self>,
        folder: &Path,
        stem: &str,
        mut expected: HashSet<String>,
        then: impl FnOnce(HashSet<String>) + 'static,
    ) {
        let (enabled, toggles_on, needed) = store::with(|m| {
            let toggles = m.plugins.capture_toggles();
            let enabled: Vec<String> = m
                .plugins
                .plugins
                .iter()
                .filter(|p| m.plugins.is_enabled(p.id()) && p.manifest.is_capture())
                .filter(|p| {
                    let own: Vec<_> = toggles.iter().filter(|t| t.plugin_id == p.id()).collect();
                    own.is_empty() || own.iter().any(|t| m.plugins.is_on(t))
                })
                .map(|p| p.id().to_string())
                .collect();
            let on: Vec<(String, String, bool)> = toggles
                .iter()
                .map(|t| (t.plugin_id.clone(), t.source.clone(), m.plugins.is_on(t)))
                .collect();
            // A source switched off doesn't need its permission.
            let mut needed: BTreeSet<String> = BTreeSet::new();
            for id in &enabled {
                let Some(p) = m.plugins.plugin(id) else {
                    continue;
                };
                let off: Vec<&str> = on
                    .iter()
                    .filter(|(pid, _, is_on)| pid == id && !is_on)
                    .filter_map(|(_, src, _)| permissions::for_source(src))
                    .collect();
                needed.extend(
                    p.manifest
                        .permissions
                        .iter()
                        .filter(|x| !off.contains(&x.as_str()))
                        .cloned(),
                );
            }
            (enabled, on, needed)
        });
        if enabled.is_empty() {
            return then(expected);
        }
        let this = self.clone();
        let (folder, stem) = (folder.to_path_buf(), stem.to_string());
        let needed: Vec<String> = needed.into_iter().collect();
        let refs: Vec<&str> = needed.iter().map(String::as_str).collect();
        let needed2 = needed.clone();
        permissions::request_then(&refs, move || {
            let refs: Vec<&str> = needed2.iter().map(String::as_str).collect();
            let granted: BTreeSet<String> = permissions::granted(&refs).into_iter().collect();
            if let Some(missing) = refs.iter().find(|p| !granted.contains(**p)) {
                store::capture_failed(permission_message(missing));
            }
            let host = Arc::new(CaptureHost::new(|plugin, event| {
                let (name, event) = (plugin.manifest.name.clone(), event.clone());
                util::main_async(move || with_live(|l| l.capture_event(&name, &event)));
            }));
            let plugins: Vec<(kit::InstalledPlugin, BTreeMap<String, kit::JsonValue>)> = enabled
                .iter()
                .filter_map(|id| this.plugin(id))
                .map(|p| {
                    let settings = this.store.settings(&p);
                    (p, settings)
                })
                .collect();
            let take = CaptureTake {
                folder: folder.to_string_lossy().into_owned(),
                stem: stem.clone(),
            };
            let problems = host.start(&plugins, &take, &granted);
            if let Some(p) = problems.first() {
                store::capture_failed(errors::capture_problem(p));
            }
            if host.is_running() {
                for (p, _) in &plugins {
                    let own: Vec<&(String, String, bool)> = toggles_on
                        .iter()
                        .filter(|(pid, _, _)| pid == p.id())
                        .collect();
                    let sources: Vec<String> = if own.is_empty() {
                        p.manifest
                            .capture
                            .as_ref()
                            .and_then(|c| c.sources.clone())
                            .unwrap_or_default()
                    } else {
                        own.iter()
                            .filter(|(_, _, on)| *on)
                            .map(|(_, s, _)| s.clone())
                            .collect()
                    };
                    expected.extend(sources.into_iter().filter(|s| {
                        permissions::for_source(s).is_none_or(|perm| granted.contains(perm))
                    }));
                }
                if let Some(r) = this.take.borrow_mut().as_mut() {
                    r.host = Some(host);
                }
            }
            then(expected);
        });
    }

    /// Mirrors Lavdeck's own capture's started and failed sources into the deck.
    fn native_changed(&self) {
        let (started, failed) = {
            let take = self.take.borrow();
            let Some(native) = take.as_ref().and_then(|r| r.native.as_ref()) else {
                return;
            };
            (native.started(), native.failed())
        };
        for source in started {
            store::capture_started(source.as_str());
        }
        for (source, error) in failed {
            let fresh = self
                .take
                .borrow_mut()
                .as_mut()
                .is_some_and(|r| r.failed.insert(source.as_str().to_string()));
            if fresh {
                store::capture_failed(errors::capture(&error));
            }
        }
    }

    fn capture_event(&self, plugin: &str, event: &kit::CaptureEvent) {
        match event.kind {
            CaptureEventKind::Started => {
                if let Some(source) = &event.source {
                    store::capture_started(source);
                }
            }
            CaptureEventKind::Error => {
                if let Some(source) = &event.source
                    && let Some(r) = self.take.borrow_mut().as_mut()
                {
                    r.failed.insert(source.clone());
                }
                let message = event.message.clone().unwrap_or_else(|| tr("error"));
                store::capture_failed(trf(
                    "{plugin}: {message}",
                    &[("plugin", plugin), ("message", &message)],
                ));
            }
            _ => {}
        }
    }

    /// Waits until every expected source has written its first frame (or failed), at most until
    /// `deadline`, then starts the audio.
    fn wait_for_capture(self: Rc<Self>, expected: HashSet<String>, deadline: Instant) {
        let settled = {
            let started: HashSet<String> =
                store::with(|m| m.capture_sources.iter().cloned().collect());
            let take = self.take.borrow();
            let Some(r) = take.as_ref() else { return };
            let capturing = r.native.is_some() || r.host.is_some();
            !capturing
                || expected
                    .iter()
                    .all(|s| started.contains(s) || r.failed.contains(s))
        };
        if settled || Instant::now() >= deadline || expected.is_empty() {
            return self.start_audio();
        }
        util::later(0.02, move || self.wait_for_capture(expected, deadline));
    }

    fn start_audio(&self) {
        let (target, format) = {
            let take = self.take.borrow();
            let Some(r) = take.as_ref() else { return };
            (r.target.clone(), r.format)
        };
        match self.recorder.start(&target, convert::format(format)) {
            Ok(()) => {
                // The take's start is its audio's, after capture warmed up.
                if let Some(r) = self.take.borrow_mut().as_mut() {
                    r.started = SystemTime::now();
                }
                if let Some(done) = self.starting.borrow_mut().take() {
                    done(Ok(()));
                }
            }
            Err(e) => self.fail_start(trf(
                "Couldn’t start recording: {error}",
                &[("error", &errors::recorder(&e))],
            )),
        }
    }

    // MARK: Stopping

    /// Stops capture (once) and keeps what it hands over for the take's metadata.
    fn stop_capture(&self) {
        let (native, host, captured) = {
            let mut take = self.take.borrow_mut();
            let Some(r) = take.as_mut() else { return };
            if r.stopping || (r.native.is_none() && r.host.is_none()) {
                if !r.stopping && r.captured.borrow().is_none() {
                    *r.captured.borrow_mut() = Some(CaptureResult::default());
                }
                return;
            }
            r.stopping = true;
            (r.native.take(), r.host.take(), r.captured.clone())
        };
        // Lavdeck's own files first, then plugins', as the Swift app lists them.
        let from_lavdeck: Rc<RefCell<Option<CaptureResult>>> =
            Rc::new(RefCell::new(native.is_none().then(CaptureResult::default)));
        let from_plugins: Rc<RefCell<Option<CaptureResult>>> =
            Rc::new(RefCell::new(host.is_none().then(CaptureResult::default)));
        let combine = {
            let (a, b, out) = (from_lavdeck.clone(), from_plugins.clone(), captured.clone());
            move || {
                let (Some(a), Some(b)) = (a.borrow().clone(), b.borrow().clone()) else {
                    return;
                };
                let mut all = a;
                all.outputs.extend(b.outputs);
                all.problems.extend(b.problems);
                *out.borrow_mut() = Some(all);
            }
        };
        let combine: Rc<dyn Fn()> = Rc::new(combine);
        if let Some(native) = native {
            let (slot, combine) = (from_lavdeck.clone(), combine.clone());
            native.stop(move |result| {
                *slot.borrow_mut() = Some(result);
                combine();
            });
            // The capture finishes its files after this returns; keep it until then.
            self.keep_native(native);
        }
        if let Some(host) = host {
            let token = NEXT_PENDING.with(|n| {
                n.set(n.get() + 1);
                n.get()
            });
            PENDING.with(|p| {
                p.borrow_mut()
                    .insert(token, (from_plugins.clone(), combine.clone()))
            });
            std::thread::spawn(move || {
                let result = host.stop(CAPTURE_STOP_TIMEOUT);
                util::main_async(move || {
                    if let Some((slot, combine)) = PENDING.with(|p| p.borrow_mut().remove(&token)) {
                        *slot.borrow_mut() = Some(result);
                        combine();
                    }
                });
            });
        }
        combine();
    }

    fn keep_native(&self, native: VideoCapture) {
        KEPT.with(|k| k.borrow_mut().push(native));
        // Long after any stop has finished.
        util::later(120.0, || {
            KEPT.with(|k| {
                k.borrow_mut().remove(0);
            })
        });
    }

    /// The audio file is done: the deck stops at once; tracks, capture and metadata follow.
    fn audio_finished(self: &Rc<Self>, path: PathBuf, error: Option<String>) {
        let Some(folder) = self.take.borrow().as_ref().map(|r| r.folder.clone()) else {
            store::recording_stopped(error, path.parent().map(Path::to_path_buf));
            return store::take_saved(None);
        };
        let audio_start = self.recorder.start_host_time();
        // The audio can end on its own (an error, the receiver unplugged).
        self.stop_capture();
        store::recording_stopped(error.clone(), Some(folder));
        if error.is_some() {
            return self.when_captured(move |this, _| {
                this.take.borrow_mut().take();
                store::take_saved(None);
            });
        }
        // Who was on which mic: the names now, which can change later.
        let mics: Vec<TakeMic> = store::with(|m| {
            [1u8, 2]
                .iter()
                .map(|&i| TakeMic {
                    index: i64::from(i),
                    name: m.display_name(i),
                    bluetooth_id: m
                        .transmitter(i)
                        .and_then(|t| t.identity.bluetooth_id.clone()),
                    armed: m.is_armed(i),
                })
                .collect()
        });
        let (solo, mode, format, split, target, stem, folder, started) = {
            let take = self.take.borrow();
            let r = take.as_ref().expect("a take");
            (
                r.solo,
                r.mode,
                r.format,
                r.split,
                r.target.clone(),
                r.stem.clone(),
                r.folder.clone(),
                r.started,
            )
        };
        // Keep one mic's channel, or split stereo into tracks, off the main thread.
        let stem_path = folder.join(&stem);
        std::thread::spawn(move || {
            let ext = format.key();
            let failure: Option<String> = if let Some(solo) = solo {
                let out = PathBuf::from(format!("{} TX{solo}.{ext}", stem_path.display()));
                let r = kit::audio::extract(&target, usize::from(solo - 1), &out);
                // The scratch stereo take.
                let _ = std::fs::remove_file(&target);
                r.err().map(|e| {
                    trf(
                        "Couldn’t save TX{index}’s track: {error}",
                        &[("index", &solo.to_string()), ("error", &errors::split(&e))],
                    )
                })
            } else if split
                && mode == Some(ChannelMode::Stereo)
                && format == crate::model::settings::RecordingFormat::Wav
            {
                let left = PathBuf::from(format!("{} TX1.wav", stem_path.display()));
                let right = PathBuf::from(format!("{} TX2.wav", stem_path.display()));
                kit::audio::split(&target, &left, &right).err().map(|e| {
                    trf(
                        "Couldn’t split tracks: {error}",
                        &[("error", &errors::split(&e))],
                    )
                })
            } else {
                None
            };
            util::main_async(move || {
                with_live(|l| {
                    l.when_captured(move |this, captured| {
                        this.save_take(
                            captured,
                            failure,
                            mics,
                            solo,
                            mode,
                            format,
                            started,
                            audio_start,
                        );
                    })
                })
            });
        });
    }

    /// Runs `f` once capture has handed over its files.
    fn when_captured(self: &Rc<Self>, f: impl FnOnce(&Rc<Live>, CaptureResult) + 'static) {
        let captured = self.take.borrow().as_ref().map(|r| r.captured.clone());
        let Some(captured) = captured else {
            return f(self, CaptureResult::default());
        };
        if let Some(result) = captured.borrow().clone() {
            return f(self, result);
        }
        let this = self.clone();
        util::later(0.05, move || this.when_captured(f));
    }

    #[allow(clippy::too_many_arguments)]
    fn save_take(
        self: &Rc<Self>,
        captured: CaptureResult,
        failure: Option<String>,
        mut mics: Vec<TakeMic>,
        solo: Option<u8>,
        mode: Option<ChannelMode>,
        format: crate::model::settings::RecordingFormat,
        started: SystemTime,
        audio_start: Option<f64>,
    ) {
        let Some(r) = self.take.borrow_mut().take() else {
            return store::take_saved(None);
        };
        if let Some(f) = failure {
            store::capture_failed(f);
        }
        if let Some(p) = captured.problems.first() {
            store::capture_failed(errors::capture_problem(p));
        }
        if solo.is_some() {
            for m in &mut mics {
                m.armed = Some(m.index) == solo.map(i64::from);
            }
        }
        let media: Vec<kit::TakeMedia> = captured
            .outputs
            .iter()
            .map(|o| kit::TakeMedia {
                file: Path::new(&o.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                source: o.source.clone(),
                kind: o.kind.clone(),
                offset: o.offset(audio_start),
                plugin: Some(o.plugin_id.clone()),
            })
            .collect();
        let duration = started.elapsed().map(|d| d.as_secs_f64()).ok();
        let mut metadata =
            TakeMetadata::new(started, duration, mode.map(|m| m.key()), format.key(), mics);
        metadata.audio_start_host_time = audio_start;
        metadata.media = (!media.is_empty()).then_some(media);
        let _ = metadata.write(&r.folder.join(format!("{}.take.json", r.stem)));
        let names: HashMap<String, String> =
            store::with(|m| m.settings.mic_names.clone().into_iter().collect());
        let take = kit::takes::take(&r.folder, &r.stem, None, &names)
            .map(|t| convert::take(&t.descriptor));
        store::take_saved(take);
    }
}

thread_local! {
    /// Capture plugins still stopping, by token: where their result goes.
    #[allow(clippy::type_complexity)]
    static PENDING: RefCell<HashMap<u64, (Rc<RefCell<Option<CaptureResult>>>, Rc<dyn Fn()>)>> = RefCell::new(HashMap::new());
    static NEXT_PENDING: Cell<u64> = const { Cell::new(0) };
    /// Lavdeck's own captures finishing their files after Stop.
    static KEPT: RefCell<Vec<VideoCapture>> = const { RefCell::new(Vec::new()) };
}

/// Opens the receiver link only, without the button monitor (snapshots of the live receiver).
pub fn start_link_only() {
    with_live(|l| l.controller.start());
}

/// What to do about a permission a capture needs and doesn't have.
fn permission_message(missing: &str) -> String {
    if missing == permissions::SCREEN {
        tr(
            "Screen recording isn’t on for this Lavdeck yet. Turn on Lavdeck in System Settings › Privacy & Security › Screen & System Audio Recording, then quit and reopen Lavdeck (macOS applies it on relaunch).",
        )
    } else {
        trf(
            "{permission} access is off for Lavdeck. Turn it on in System Settings › Privacy & Security.",
            &[("permission", &permissions::name(missing))],
        )
    }
}

/// Every take under `root`, newest first (off the main thread: it reads audio durations).
pub fn scan(root: &Path, names: &BTreeMap<String, String>) -> Vec<Take> {
    let names: HashMap<String, String> = names.clone().into_iter().collect();
    kit::takes::scan(root, &names)
        .iter()
        .map(|t| convert::take(&t.descriptor))
        .collect()
}

/// A finished job's note: how many files the plugin wrote.
fn finished_note(outputs: usize) -> Option<String> {
    (outputs > 0).then(|| trn(outputs as u64, "{n} file", "{n} files"))
}

impl Backend for Live {
    fn start(&self) {
        self.controller.start();
        let tap = store::with(|m| m.settings.tap_to_record);
        self.restart_buttons(tap);
    }

    fn set_channel(&self, mode: ChannelMode, done: Done) {
        let controller = self.controller.clone();
        let done = util::MainOnly::new(done);
        std::thread::spawn(move || {
            let result = controller
                .set_channel(convert::kit_mode(mode))
                .map_err(|e| errors::mic(&e));
            util::main_async(move || (done.into_inner())(result));
        });
    }

    fn set_gain(&self, db: i32, done: Done) {
        let controller = self.controller.clone();
        let done = util::MainOnly::new(done);
        std::thread::spawn(move || {
            let result = controller
                .set_receiver_gain(db)
                .map_err(|e| errors::mic(&e));
            util::main_async(move || (done.into_inner())(result));
        });
    }

    fn start_recording(&self, request: RecordRequest, done: Done) {
        *self.starting.borrow_mut() = Some(done);
        // The microphone prompt blocks until answered.
        std::thread::spawn(move || {
            let allowed = kit::recorder::request_microphone_permission();
            util::main_async(move || with_live(|l| l.begin(request, allowed)));
        });
    }

    fn stop_recording(&self) {
        self.stop_capture();
        self.recorder.stop();
    }

    fn channel_levels(&self) -> Vec<f32> {
        self.recorder.channel_levels()
    }

    fn scan_takes(&self, root: &Path, names: &BTreeMap<String, String>) -> Vec<Take> {
        scan(root, names)
    }

    fn scanner(&self) -> Option<crate::backend::Scanner> {
        Some(scan)
    }

    fn load_transcript(&self, take: &Take) -> Option<TranscriptDocument> {
        TranscriptDocument::load(&take.transcript_path()).ok()
    }

    fn trash(&self, take: &Take) -> Result<(), String> {
        kit::takes::trash(&kit::Take {
            descriptor: convert::descriptor(take),
            metadata: None,
        })
    }

    fn discover_plugins(&self) -> (Vec<Plugin>, Vec<String>) {
        let (found, problems) = self.store.discover();
        let found: Vec<kit::InstalledPlugin> = found
            .into_iter()
            .filter(|p| !BUILT_IN.contains(&p.id()))
            .collect();
        let plugins = found.iter().map(convert::plugin).collect();
        *self.plugins.borrow_mut() = found;
        (
            plugins,
            problems.iter().map(errors::plugin_problem).collect(),
        )
    }

    fn plugin_state(
        &self,
        plugins: &[Plugin],
    ) -> (
        BTreeMap<String, bool>,
        BTreeMap<String, BTreeMap<String, JsonValue>>,
    ) {
        let mut enabled = BTreeMap::new();
        let mut values = BTreeMap::new();
        for p in plugins {
            enabled.insert(p.id().to_string(), self.store.is_enabled(p.id()));
            if let Some(k) = self.plugin(p.id()) {
                values.insert(
                    p.id().to_string(),
                    convert::values(&self.store.settings(&k)),
                );
            }
        }
        (enabled, values)
    }

    fn install_plugin(&self, folder: &Path) -> Result<(), String> {
        self.store
            .install(folder)
            .map(|_| ())
            .map_err(|e| errors::install(&e))
    }

    fn plugins_folder(&self) -> PathBuf {
        PluginStore::plugins_folder()
    }

    fn launch(&self, launch: Launch, take: Take) {
        let id = launch.job_id;
        match launch.work {
            Work::Plugin {
                plugin_id,
                kind,
                name,
                ..
            } => {
                let Some(plugin) = self.plugin(&plugin_id) else {
                    return util::later(0.0, move || {
                        store::job_finished(
                            id,
                            JobState::Failed(tr("The plugin isn’t installed any more.")),
                        )
                    });
                };
                let settings = self.store.settings(&plugin);
                let request = plugin.request(&kind, &name, &convert::descriptor(&take), settings);
                let job = kit::runner::spawn(
                    plugin,
                    request,
                    move |message| {
                        let message = message.clone();
                        util::main_async(move || match message.kind {
                            PluginMessageKind::Progress => {
                                store::job_progress(id, message.fraction, message.message)
                            }
                            PluginMessageKind::Output => store::job_output(id),
                            PluginMessageKind::Done => {
                                store::job_progress(id, None, message.message)
                            }
                            _ => {}
                        });
                    },
                    move |result| {
                        let state = match result {
                            Ok(r) => JobState::Finished(finished_note(r.outputs.len())),
                            Err(kit::PluginRunError::Cancelled) => {
                                JobState::Failed(CANCELLED.into())
                            }
                            Err(e) => JobState::Failed(errors::plugin_run(&e)),
                        };
                        util::main_async(move || {
                            with_live(|l| {
                                l.jobs.borrow_mut().remove(&id);
                            });
                            store::job_finished(id, state);
                        });
                    },
                );
                self.jobs.borrow_mut().insert(id, JobHandle::Plugin(job));
            }
            Work::MakeVideo { .. } => {
                let settings = store::with(|m| convert::video(&m.settings.video));
                let descriptor = convert::descriptor(&take);
                let cancelled = Arc::new(AtomicBool::new(false));
                self.jobs
                    .borrow_mut()
                    .insert(id, JobHandle::Video(cancelled.clone()));
                std::thread::spawn(move || {
                    let result = VideoComposer::make(&descriptor, &settings, |fraction| {
                        util::main_async(move || store::job_progress(id, Some(fraction), None));
                    });
                    let state = if cancelled.load(Ordering::SeqCst) {
                        JobState::Failed(CANCELLED.into())
                    } else {
                        match result {
                            Ok(path) => {
                                let file = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                JobState::Finished(Some(trf("Saved {file}", &[("file", &file)])))
                            }
                            Err(e) => JobState::Failed(errors::capture(&e)),
                        }
                    };
                    util::main_async(move || {
                        with_live(|l| {
                            l.jobs.borrow_mut().remove(&id);
                        });
                        store::job_finished(id, state);
                    });
                });
            }
        }
    }

    fn cancel(&self, job_id: u64) {
        match self.jobs.borrow().get(&job_id) {
            Some(JobHandle::Plugin(job)) => job.cancel(),
            Some(JobHandle::Video(flag)) => flag.store(true, Ordering::SeqCst),
            None => {}
        }
    }

    fn open_in_app(&self, take: &Take, plugin: &Plugin) -> Result<(), String> {
        let Some(kit_plugin) = self.plugin(plugin.id()) else {
            return Ok(());
        };
        let Some(spec) = kit_plugin.manifest.app.clone() else {
            return Ok(());
        };
        let Some(url) = spec
            .url(&convert::descriptor(take))
            .and_then(|u| NSURL::URLWithString(&ns(&u)))
        else {
            return Ok(());
        };
        let bundle = util::file_url(&kit_plugin.folder.join(&spec.bundle));
        let config = objc2_app_kit::NSWorkspaceOpenConfiguration::configuration();
        config.setActivates(true);
        let name = plugin.manifest.name.clone();
        let handler = block2::RcBlock::new(
            move |_app: *mut objc2_app_kit::NSRunningApplication,
                  error: *mut objc2_foundation::NSError| {
                // SAFETY: AppKit passes a valid error or null.
                if let Some(error) = unsafe { error.as_ref() } {
                    let message = format!("{name}: {}", error.localizedDescription());
                    util::main_async(move || store::update(|s| s.plugins.problems.push(message)));
                }
            },
        );
        NSWorkspace::sharedWorkspace()
            .openURLs_withApplicationAtURL_configuration_completionHandler(
                &NSArray::from_retained_slice(&[url]),
                &bundle,
                &config,
                Some(&handler),
            );
        Ok(())
    }

    fn save_settings(&self, settings: &Settings) {
        crate::prefs::save(settings);
        self.restart_buttons(settings.tap_to_record);
    }

    fn save_plugins(
        &self,
        enabled: &BTreeMap<String, bool>,
        plugin_id: Option<&str>,
        values: Option<&BTreeMap<String, JsonValue>>,
    ) {
        for (id, on) in enabled {
            if self.store.is_enabled(id) != *on {
                self.store.set_enabled(id, *on);
            }
        }
        if let (Some(id), Some(values)) = (plugin_id, values) {
            for (key, value) in values {
                self.store.set_setting(&convert::kit_value(value), key, id);
            }
        }
    }

    fn may_prompt(&self) -> bool {
        true
    }
}
