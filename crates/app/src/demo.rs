//! `--demo`: made-up mics, takes and plugins, for screenshots and for trying the interface
//! without the receiver. Like the Swift app's demo it never opens the receiver, claims the
//! shortcut, prompts for permissions or touches saved settings and names.
//!
//! The takes live in a scratch folder (`$TMPDIR/lavdeck-demo`) with a generated 75-second
//! two-voice clip behind every audio file, so the player, transcript follow and drag-out work.
//! Deleting a demo take only drops it from the list.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::backend::{Backend, Done, RecordRequest};
use crate::format;
use crate::i18n::trn;
use crate::model::AppState;
use crate::model::plugins::{
    AppSpec, JobState, JsonValue, Launch, Manifest, Plugin, PluginAction, PluginSetting,
    SettingKind, Work,
};
use crate::model::settings::{RecordingFormat, Settings};
use crate::model::status::{ChannelMode, DeviceIdentity, MicStatus, TransmitterStatus};
use crate::model::takes::{Role, Take, TakeFile, TakeSpeaker, TranscriptDocument};
use crate::{store, util};

const CLIP_SECONDS: f64 = 75.0;
const RATE: u32 = 16_000;

/// The demo's receiver, takes and jobs.
pub struct Demo {
    root: PathBuf,
    takes: RefCell<Vec<Take>>,
    /// Interactive runs move the meters; snapshots keep them still.
    live: bool,
    started: Cell<f64>,
    cancelled: RefCell<Vec<u64>>,
}

impl Demo {
    pub fn new(live: bool) -> Demo {
        let root = std::env::temp_dir().join("lavdeck-demo");
        let takes = library(&root);
        Demo {
            root,
            takes: RefCell::new(takes),
            live,
            started: Cell::new(0.0),
            cancelled: RefCell::new(Vec::new()),
        }
    }

    pub fn recordings_root(&self) -> PathBuf {
        self.root.join("Recordings")
    }

    /// Settings as on a first launch, with the demo's recordings folder.
    pub fn settings(&self) -> Settings {
        let mut s = Settings::defaults(&util::home());
        s.recordings_root = self.recordings_root();
        s
    }

    /// Starts the meters moving (interactive runs only).
    pub fn animate(&self) {
        if self.live {
            tick(0, Talk::default());
        }
    }
}

/// Two linked mics named Host and Guest, in stereo at 0 dB with noise cancelling on High; as
/// `AppModel.loadDemo`.
pub fn load(s: &mut AppState, recording: bool) {
    s.demo = true;
    let mut st = MicStatus::default();
    st.receiver.identity = DeviceIdentity {
        product_name: Some("DJI Mic Mini 2".into()),
        firmware: Some("02.03.11.00".into()),
        ..Default::default()
    };
    st.receiver.channel_mode = Some(ChannelMode::Stereo);
    st.receiver.gain_db = Some(0);
    st.receiver.noise_cancelling = Some(true);
    st.receiver.noise_cancelling_level = Some(1);
    st.receiver.connected_transmitters = [1, 2].into();
    for (index, id, meter, charging) in [(1u8, "a1b2c3", 58, false), (2, "d4e5f6", 41, !recording)]
    {
        let mut tx = TransmitterStatus::new(index);
        tx.identity = DeviceIdentity {
            product_name: Some("DJI Mic Mini".into()),
            bluetooth_id: Some(id.into()),
            firmware: Some("02.03.17.00".into()),
            ..Default::default()
        };
        tx.battery_level = Some(1);
        tx.is_charging = Some(charging);
        tx.meter_level = Some(meter);
        st.transmitters.insert(index, tx);
    }
    s.demo_names = [
        ("a1b2c3".to_string(), "Host".to_string()),
        ("d4e5f6".to_string(), "Guest".to_string()),
    ]
    .into();
    s.status = st;
    s.is_connected = true;
    s.known_transmitters = [1, 2].into();
    s.settings.armed = [1, 2].into();
    s.identifying = false;
    s.talking_mic = None;
    s.is_starting = false;
    if recording {
        s.is_recording = true;
        s.recording_mode = Some(ChannelMode::Stereo);
        s.recording_started = Some(format::now() - 767.0);
        s.recording_levels = vec![-14.0, -21.0];
        let v = &s.settings.video;
        s.capture_sources = [v.screen.then_some("screen"), v.camera.then_some("camera")]
            .into_iter()
            .flatten()
            .map(String::from)
            .collect();
    } else {
        s.is_recording = false;
        s.recording_started = None;
        s.recording_levels.clear();
        s.capture_sources.clear();
    }
}

/// Identify mode with Host's strip lit, as `AppModel.loadDemoIdentify`.
pub fn load_identify(s: &mut AppState) {
    load(s, false);
    s.identifying = true;
    s.talking_mic = Some(1);
}

/// The demo's plugins: a transcriber that runs after each take, and a companion app.
pub fn plugins() -> Vec<Plugin> {
    let setting =
        |key: &str, title: &str, kind, default, options: &[(&str, &str)], help: Option<&str>| {
            PluginSetting {
                key: key.into(),
                title: title.into(),
                kind,
                default,
                options: options
                    .iter()
                    .map(|(v, t)| (v.to_string(), t.to_string()))
                    .collect(),
                help: help.map(String::from),
            }
        };
    let transcribe = Manifest {
        id: "io.github.piedol.lavdeck.transcribe".into(),
        name: "Transcription".into(),
        version: "0.1.0".into(),
        summary: Some("Timestamped transcripts with Whisper large-v3-turbo, per speaker when each person has their own mic.".into()),
        events: vec!["take.finished".into()],
        actions: vec![PluginAction { id: "transcribe".into(), title: "Generate transcript".into(), requires: vec![] }],
        settings: vec![
            setting(
                "language",
                "Language",
                SettingKind::Choice,
                JsonValue::String("auto".into()),
                &[("auto", "Detect automatically"), ("en", "English"), ("es", "Spanish"), ("fr", "French"), ("de", "German")],
                None,
            ),
            setting(
                "bleed_db",
                "Crosstalk threshold (dB)",
                SettingKind::Number,
                JsonValue::Number(6.0),
                &[],
                Some("In two-speaker takes, a word is dropped from a mic's track when that mic is this much quieter than the other at the same moment."),
            ),
            setting("subtitles", "Also write SRT and VTT subtitles", SettingKind::Bool, JsonValue::Bool(true), &[], None),
            setting("model", "Whisper model folder (MLX)", SettingKind::Path, JsonValue::String("~/Models/whisper-large-v3-turbo-mlx".into()), &[], None),
        ],
        produces: vec!["transcript".into()],
        kind: None,
        permissions: vec![],
        capture_sources: None,
        app: None,
    };
    let studio = Manifest {
        id: "io.github.piedol.lavdeck.studio".into(),
        name: "Studio".into(),
        version: "0.1.0".into(),
        summary: Some("Timeline editor for your takes: trim, split, move, speed, gain and fades, camera bubble layout, export.".into()),
        events: vec![],
        actions: vec![],
        settings: vec![],
        produces: vec![],
        kind: Some("app".into()),
        permissions: vec![],
        capture_sources: None,
        app: Some(AppSpec {
            bundle: "build/Lavdeck Studio.app".into(),
            open_take: Some("lavdeck-studio://open?folder={folder}&stem={stem}".into()),
            action_title: Some("Open in Studio".into()),
        }),
    };
    vec![
        Plugin {
            folder: PathBuf::from("/demo/lavdeck-studio"),
            manifest: studio,
        },
        Plugin {
            folder: PathBuf::from("/demo/lavdeck-plugin-transcribe"),
            manifest: transcribe,
        },
    ]
}

// MARK: Live meters

#[derive(Default, Clone, Copy)]
struct Talk {
    talking: Option<u8>,
    evidence: f64,
}

/// A slow conversation: each mic takes a turn, syllables at about 4 Hz.
fn speech(index: u8, t: f64) -> f64 {
    let turn = ((t / 4.0).floor() as i64 % 2) as u8 + 1;
    let syllable =
        (0.5 + 0.5 * (t * 2.0 * std::f64::consts::PI * 3.7 + f64::from(index)).sin()).powf(1.5);
    let breath = if (t * 0.9).sin() > 0.85 { 0.2 } else { 1.0 };
    if turn == index {
        0.35 + 0.55 * syllable * breath
    } else {
        0.08 + 0.05 * syllable
    }
}

fn tick(n: u64, mut talk: Talk) {
    util::later(0.1, move || {
        let t = n as f64 * 0.1;
        let (identifying, recording) = store::with(|s| (s.identifying, s.is_recording));
        let raw = |i: u8| (25.0 + speech(i, t) * 60.0).round() as i32;
        let (a, b) = (raw(1), raw(2));
        store::update(|s| {
            for (i, v) in [(1u8, a), (2, b)] {
                if let Some(tx) = s.status.transmitters.get_mut(&i) {
                    tx.meter_level = Some(v);
                }
            }
        });
        if identifying {
            // The kit's talking detector: the louder mic leads by 12 and is speech-loud.
            let now = format::now();
            if a.max(b) >= 52 && (a - b).abs() >= 12 {
                talk.talking = Some(if a > b { 1 } else { 2 });
                talk.evidence = now;
            } else if now - talk.evidence > 1.5 {
                talk.talking = None;
            }
            store::set_talking(talk.talking);
        }
        let _ = recording;
        tick(n + 1, talk);
    });
}

impl Backend for Demo {
    fn start(&self) {}

    fn set_channel(&self, mode: ChannelMode, done: Done) {
        util::later(0.25, move || {
            store::update(|s| s.status.receiver.channel_mode = Some(mode));
            done(Ok(()));
        });
    }

    fn set_gain(&self, db: i32, done: Done) {
        util::later(0.2, move || {
            store::update(|s| s.status.receiver.gain_db = Some(db));
            done(Ok(()));
        });
    }

    fn start_recording(&self, request: RecordRequest, done: Done) {
        // Capture sources warm up first, then the audio starts.
        let video = request.settings.video.clone();
        util::later(if video.records_video() { 0.9 } else { 0.3 }, move || {
            for (on, source) in [(video.screen, "screen"), (video.camera, "camera")] {
                if on {
                    store::capture_started(source);
                }
            }
            with_demo(|d| d.started.set(format::now()));
            done(Ok(()));
        });
    }

    fn stop_recording(&self) {
        let (started, mode, armed, format_, split, names, video) = store::with(|s| {
            (
                s.recording_started.unwrap_or_else(format::now),
                s.recording_mode,
                s.settings.armed.clone(),
                s.settings.format,
                s.settings.split_tracks,
                (s.display_name(1), s.display_name(2)),
                s.settings.video.clone(),
            )
        });
        let take = new_take(
            &self.root,
            started,
            format::now() - started,
            mode,
            &armed,
            format_,
            split,
            names,
            &video,
        );
        self.takes.borrow_mut().insert(0, take.clone());
        let folder = take.folder.clone();
        util::later(0.3, move || {
            store::recording_finished(Ok(Some(take)), Some(folder))
        });
    }

    fn channel_levels(&self) -> Vec<f32> {
        let t = format::now() - self.started.get();
        let db = |i: u8| {
            if self.live {
                (-60.0 + speech(i, t) * 54.0) as f32
            } else {
                [-14.0, -21.0][usize::from(i - 1)]
            }
        };
        vec![db(1), db(2)]
    }

    fn scan_takes(&self, _root: &Path, _names: &BTreeMap<String, String>) -> Vec<Take> {
        let mut t = self.takes.borrow().clone();
        t.sort_by(|a, b| {
            b.started
                .unwrap_or(0.0)
                .total_cmp(&a.started.unwrap_or(0.0))
        });
        t
    }

    fn load_transcript(&self, take: &Take) -> Option<TranscriptDocument> {
        TranscriptDocument::load(&take.transcript_path()).ok()
    }

    fn trash(&self, take: &Take) -> Result<(), String> {
        // Only the list: the demo's files are shared by every take.
        self.takes.borrow_mut().retain(|t| t.id != take.id);
        Ok(())
    }

    fn discover_plugins(&self) -> (Vec<Plugin>, Vec<String>) {
        (plugins(), Vec::new())
    }

    fn install_plugin(&self, _folder: &Path) -> Result<(), String> {
        Err(crate::i18n::tr("The demo doesn’t install plugins."))
    }

    fn plugins_folder(&self) -> PathBuf {
        self.root.join("Plugins")
    }

    fn launch(&self, launch: Launch, take: Take) {
        let id = launch.job_id;
        let transcribe = matches!(launch.work, Work::Plugin { .. });
        let takes_root = self.root.clone();
        let steps = if transcribe { 24 } else { 16 };
        fn step(id: u64, i: u32, steps: u32, transcribe: bool, take: Take, root: PathBuf) {
            util::later(0.15, move || {
                let cancelled = demo_cancelled(id);
                if cancelled {
                    store::job_finished(
                        id,
                        JobState::Failed(crate::model::plugins::CANCELLED.into()),
                    );
                    return;
                }
                if i < steps {
                    let message = if transcribe {
                        crate::i18n::tr("Transcribing")
                    } else {
                        crate::i18n::tr("Making the video")
                    };
                    store::job_progress(id, Some(f64::from(i) / f64::from(steps)), Some(message));
                    step(id, i + 1, steps, transcribe, take, root);
                    return;
                }
                let note = if transcribe {
                    write_transcript(&take);
                    with_demo(|d| d.add_file(&take.id, transcript_file(&take)));
                    Some(trn(1, "{n} file", "{n} files"))
                } else {
                    let _ = &root;
                    Some(crate::i18n::tr("Saved"))
                };
                store::job_finished(id, JobState::Finished(note));
            });
        }
        step(id, 0, steps, transcribe, take, takes_root);
    }

    fn cancel(&self, job_id: u64) {
        self.cancelled.borrow_mut().push(job_id);
    }

    fn open_in_app(&self, take: &Take, plugin: &Plugin) -> Result<(), String> {
        util::log(format!(
            "demo: would open {} in {}",
            take.id, plugin.manifest.name
        ));
        Ok(())
    }

    fn save_settings(&self, _settings: &Settings) {}

    fn save_plugins(
        &self,
        _e: &BTreeMap<String, bool>,
        _id: Option<&str>,
        _v: Option<&BTreeMap<String, JsonValue>>,
    ) {
    }

    fn may_prompt(&self) -> bool {
        false
    }
}

thread_local! {
    static DEMO: RefCell<Option<std::rc::Rc<Demo>>> = const { RefCell::new(None) };
}

/// Installs the demo as the store's backend.
pub fn install(demo: std::rc::Rc<Demo>, state: AppState) {
    DEMO.with(|d| *d.borrow_mut() = Some(demo.clone()));
    store::install(state, demo);
}

fn with_demo(f: impl FnOnce(&Demo)) {
    DEMO.with(|d| {
        if let Some(d) = d.borrow().as_ref() {
            f(d)
        }
    });
}

fn demo_cancelled(id: u64) -> bool {
    DEMO.with(|d| {
        d.borrow()
            .as_ref()
            .is_some_and(|d| d.cancelled.borrow().contains(&id))
    })
}

impl Demo {
    fn add_file(&self, take_id: &str, file: TakeFile) {
        for t in self.takes.borrow_mut().iter_mut() {
            if t.id == take_id && !t.files.iter().any(|f| f.path == file.path) {
                t.files.push(file.clone());
                t.files.sort_by(|a, b| a.path.cmp(&b.path));
            }
        }
    }
}

// MARK: The library

/// Seconds since 1970 for a local date and time.
fn local(year: i32, month: i32, day: i32, hour: i32, minute: i32, second: i32) -> f64 {
    // SAFETY: mktime reads a fully initialized tm (zeroed, then filled) and normalises it.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = year - 1900;
        tm.tm_mon = month - 1;
        tm.tm_mday = day;
        tm.tm_hour = hour;
        tm.tm_min = minute;
        tm.tm_sec = second;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm) as f64
    }
}

fn local_parts(t: f64) -> (String, String) {
    // SAFETY: localtime_r fills the zeroed tm from a valid time.
    unsafe {
        let secs = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        (
            format!(
                "{:04}-{:02}-{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday
            ),
            format!("{:02}.{:02}.{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec),
        )
    }
}

/// What kind of take a library entry is.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Stereo with each mic's track.
    TwoSpeaker,
    /// One mic alone.
    Solo,
    /// Stereo without split tracks.
    Stereo,
}

struct Entry {
    when: (i32, i32, i32, i32, i32, i32),
    length: f64,
    kind: Kind,
    names: (&'static str, &'static str),
    /// `pip`, `screen`, `camera`.
    video: &'static [&'static str],
    transcript: Option<&'static [(&'static str, &'static str)]>,
}

const FIRST: &[(&str, &str)] = &[
    (
        "tx1",
        "Welcome back. Today it's all about recording conversations away from a studio.",
    ),
    (
        "tx2",
        "Thanks for having me. I record most of my interviews in ordinary rooms.",
    ),
    ("tx1", "So where should a lav go?"),
    (
        "tx2",
        "A hand's width below the chin, clipped to the shirt, cable tucked away so it doesn't rub.",
    ),
    ("tx1", "And what about the room?"),
    (
        "tx2",
        "Soft furnishings help. A sofa, curtains, a rug. Bare walls and hard floors make it ring.",
    ),
    ("tx1", "How loud should we be recording?"),
    (
        "tx2",
        "Peaks around minus twelve leave room for laughter without clipping.",
    ),
    (
        "tx1",
        "Most of it comes down to where the mic sits, not which mic you buy.",
    ),
    (
        "tx2",
        "Exactly. Move the mic before you reach for an equaliser.",
    ),
];

fn entries() -> Vec<Entry> {
    let e = |when, length: f64, kind, names, video, transcript| Entry {
        when,
        length,
        kind,
        names,
        video,
        transcript,
    };
    let hg = ("Host", "Guest");
    let tx = ("TX1", "TX2");
    let host = ("Host", "");
    vec![
        e(
            (2026, 10, 26, 11, 5, 9),
            842.0,
            Kind::TwoSpeaker,
            hg,
            &[][..],
            Some(FIRST),
        ),
        e(
            (2026, 10, 25, 18, 22, 40),
            95.0,
            Kind::Solo,
            host,
            &[],
            Some(
                &[(
                    "tx1",
                    "Quick note for the edit: trim the first ten seconds, the door was open.",
                )][..],
            ),
        ),
        e(
            (2026, 10, 25, 9, 15, 2),
            2650.0,
            Kind::TwoSpeaker,
            hg,
            &["pip", "screen", "camera"],
            Some(
                &[
                    (
                        "tx1",
                        "Let's walk through the export settings one at a time.",
                    ),
                    ("tx2", "Starting with the frame rate."),
                ][..],
            ),
        ),
        e(
            (2026, 10, 24, 15, 40, 11),
            487.0,
            Kind::Stereo,
            tx,
            &[],
            None,
        ),
        e(
            (2026, 10, 23, 10, 2, 45),
            1912.0,
            Kind::TwoSpeaker,
            hg,
            &["pip", "screen", "camera"],
            Some(
                &[(
                    "tx1",
                    "This is the tutorial for setting up two mics on one receiver.",
                )][..],
            ),
        ),
        e(
            (2026, 10, 22, 16, 48, 30),
            1338.0,
            Kind::TwoSpeaker,
            hg,
            &["camera"],
            None,
        ),
        e(
            (2026, 10, 21, 14, 30, 5),
            1795.0,
            Kind::TwoSpeaker,
            hg,
            &[],
            Some(
                &[(
                    "tx1",
                    "Today we're answering your questions about wireless range.",
                )][..],
            ),
        ),
        e(
            (2026, 10, 20, 19, 12, 18),
            212.0,
            Kind::Solo,
            host,
            &[],
            Some(&[("tx1", "Voice-over for the product page, take three.")][..]),
        ),
        e(
            (2026, 10, 19, 11, 48, 0),
            3121.0,
            Kind::TwoSpeaker,
            hg,
            &["pip", "screen", "camera"],
            Some(
                &[(
                    "tx1",
                    "Welcome to the long-form episode on field recording.",
                )][..],
            ),
        ),
        e(
            (2026, 10, 17, 8, 55, 33),
            640.0,
            Kind::Stereo,
            tx,
            &[],
            None,
        ),
        e(
            (2026, 10, 16, 13, 20, 7),
            1504.0,
            Kind::TwoSpeaker,
            hg,
            &[],
            Some(&[("tx1", "Episode twelve, and we finally talk about wind.")][..]),
        ),
        e(
            (2026, 10, 15, 17, 3, 52),
            921.0,
            Kind::TwoSpeaker,
            hg,
            &["pip", "camera"],
            Some(&[("tx1", "Quick camera check before the interview.")][..]),
        ),
        e(
            (2026, 10, 14, 10, 10, 10),
            2280.0,
            Kind::TwoSpeaker,
            hg,
            &[],
            Some(&[("tx1", "Episode eleven: the case for recording in mono.")][..]),
        ),
        e(
            (2026, 10, 13, 16, 5, 41),
            148.0,
            Kind::Solo,
            host,
            &[],
            Some(&[("tx1", "Pickup line for the intro.")][..]),
        ),
        e(
            (2026, 10, 12, 9, 41, 26),
            1677.0,
            Kind::TwoSpeaker,
            hg,
            &["pip", "screen", "camera"],
            Some(&[("tx1", "Today's tutorial covers gain staging.")][..]),
        ),
        e(
            (2026, 10, 11, 15, 27, 3),
            1203.0,
            Kind::TwoSpeaker,
            hg,
            &[],
            None,
        ),
        e(
            (2026, 10, 10, 12, 2, 59),
            2954.0,
            Kind::TwoSpeaker,
            hg,
            &[],
            Some(&[("tx1", "Welcome to the show. Two guests today.")][..]),
        ),
        e(
            (2026, 10, 9, 18, 36, 14),
            377.0,
            Kind::Stereo,
            tx,
            &[],
            None,
        ),
    ]
}

/// The clip files every demo take's audio points at.
struct Clips {
    stereo: PathBuf,
    tx1: PathBuf,
    tx2: PathBuf,
}

fn clips(root: &Path) -> Clips {
    let dir = root.join("clips");
    let c = Clips {
        stereo: dir.join("stereo.wav"),
        tx1: dir.join("tx1.wav"),
        tx2: dir.join("tx2.wav"),
    };
    if !(c.stereo.exists() && c.tx1.exists() && c.tx2.exists()) {
        let _ = std::fs::create_dir_all(&dir);
        let (l, r) = voices();
        let _ = std::fs::write(&c.tx1, wav(&[&l]));
        let _ = std::fs::write(&c.tx2, wav(&[&r]));
        let _ = std::fs::write(&c.stereo, wav(&[&l, &r]));
    }
    c
}

/// Two voice-like tones taking turns along the first take's transcript.
fn voices() -> (Vec<i16>, Vec<i16>) {
    let n = (CLIP_SECONDS * f64::from(RATE)) as usize;
    let segments = segments(FIRST);
    let mut l = vec![0i16; n];
    let mut r = vec![0i16; n];
    for (i, (ls, rs)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
        let t = i as f64 / f64::from(RATE);
        let Some((_, end_, who)) = segments.iter().find(|(s, e, _)| *s <= t && t < *e) else {
            continue;
        };
        let (pitch, own, other) = if who == "tx1" {
            (130.0, &mut *ls, &mut *rs)
        } else {
            (205.0, &mut *rs, &mut *ls)
        };
        // Syllables, a little vibrato, three harmonics; a quiet bleed into the other mic.
        let syl = (0.5 - 0.5 * (t * 2.0 * std::f64::consts::PI * 4.1).cos()).powi(2);
        let fade = ((end_ - t) * 8.0).clamp(0.0, 1.0);
        let f = pitch * (1.0 + 0.02 * (t * 5.0).sin());
        let w = 2.0 * std::f64::consts::PI * f * t;
        let v = (w.sin() + 0.5 * (2.0 * w).sin() + 0.25 * (3.0 * w).sin()) * syl * fade * 0.16;
        *own = (v * 32767.0) as i16;
        *other = (v * 0.08 * 32767.0) as i16;
    }
    (l, r)
}

fn wav(channels: &[&[i16]]) -> Vec<u8> {
    let frames = channels[0].len();
    let ch = channels.len() as u16;
    let data_len = (frames * channels.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&ch.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * u32::from(ch) * 2).to_le_bytes());
    out.extend_from_slice(&(ch * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        for c in channels {
            out.extend_from_slice(&c[i].to_le_bytes());
        }
    }
    out
}

/// Start, end and speaker of each line: about seven seconds each.
fn segments(lines: &[(&str, &str)]) -> Vec<(f64, f64, String)> {
    let mut t = 0.0;
    lines
        .iter()
        .map(|(who, text)| {
            let len = (2.5 + text.split_whitespace().count() as f64 * 0.32).min(7.0);
            let s = (t, t + len, who.to_string());
            t += len + 0.6;
            s
        })
        .collect()
}

fn link(target: &Path, at: &Path) {
    if at.symlink_metadata().is_err() {
        let _ = std::os::unix::fs::symlink(target, at);
    }
}

fn transcript_file(take: &Take) -> TakeFile {
    let mut f = TakeFile::new(take.transcript_path(), Role::Derived);
    f.kind = Some("transcript".into());
    f
}

fn write_transcript(take: &Take) {
    let lines: Vec<(&str, &str)> = FIRST.to_vec();
    write_transcript_lines(take, &lines);
}

fn write_transcript_lines(take: &Take, lines: &[(&str, &str)]) {
    let names: BTreeMap<String, String> = take
        .speakers
        .iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();
    let speakers: Vec<serde_json::Value> = if take.speakers.is_empty() {
        vec![serde_json::json!({"id": "tx1", "name": "TX1"})]
    } else {
        take.speakers
            .iter()
            .map(|s| serde_json::json!({"id": s.id, "name": s.name}))
            .collect()
    };
    let segs: Vec<serde_json::Value> = segments(lines)
        .into_iter()
        .zip(lines)
        .map(|((start, end, who), (_, text))| {
            let who = if names.contains_key(&who) { who } else { names.keys().next().cloned().unwrap_or(who) };
            serde_json::json!({"start": (start * 100.0).round() / 100.0, "end": (end * 100.0).round() / 100.0, "speaker": who, "text": text})
        })
        .collect();
    let doc = serde_json::json!({
        "version": 1,
        "take": take.id,
        "model": "whisper-large-v3-turbo",
        "language": "en",
        "created": "2026-10-26T11:20:00Z",
        "speakers": speakers,
        "segments": segs,
    });
    let _ = std::fs::write(
        take.transcript_path(),
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    );
}

#[allow(clippy::too_many_arguments)]
fn build_take(
    root: &Path,
    started: f64,
    length: f64,
    kind: Kind,
    names: (&str, &str),
    video: &[&str],
    format: RecordingFormat,
) -> Take {
    let clips = clips(root);
    let (day, stem) = local_parts(started);
    let folder = root.join("Recordings").join(&day);
    let _ = std::fs::create_dir_all(&folder);
    let ext = format.key();
    let mut files = Vec::new();
    let mut add = |name: String, target: &Path, mut file: TakeFile| {
        let path = folder.join(name);
        link(target, &path);
        file.path = path;
        files.push(file);
    };
    let mut speakers = Vec::new();
    match kind {
        Kind::TwoSpeaker | Kind::Stereo => {
            add(
                format!("{stem} Stereo.{ext}"),
                &clips.stereo,
                TakeFile::new("", Role::Stereo),
            );
            for (i, name, clip) in [(1u8, names.0, &clips.tx1), (2, names.1, &clips.tx2)] {
                let mut track = None;
                if kind == Kind::TwoSpeaker {
                    let mut f = TakeFile::new("", Role::Track);
                    f.mic = Some(i);
                    add(format!("{stem} TX{i}.{ext}"), clip, f);
                    track = Some(folder.join(format!("{stem} TX{i}.{ext}")));
                }
                speakers.push(TakeSpeaker {
                    id: format!("tx{i}"),
                    mic: i,
                    name: name.to_string(),
                    file: track,
                });
            }
        }
        Kind::Solo => {
            let mut f = TakeFile::new("", Role::Track);
            f.mic = Some(1);
            add(format!("{stem} TX1.{ext}"), &clips.tx1, f);
            speakers.push(TakeSpeaker {
                id: "tx1".into(),
                mic: 1,
                name: names.0.to_string(),
                file: Some(folder.join(format!("{stem} TX1.{ext}"))),
            });
        }
    }
    for source in video {
        let (name, mut f) = match *source {
            "pip" => (format!("{stem} PiP.mp4"), TakeFile::new("", Role::Derived)),
            "screen" => (
                format!("{stem} Screen.mov"),
                TakeFile::new("", Role::Capture),
            ),
            _ => (
                format!("{stem} Camera.mov"),
                TakeFile::new("", Role::Capture),
            ),
        };
        f.kind = Some("video".into());
        f.source = Some(source.to_string());
        f.offset = (*source != "pip").then_some(0.0);
        // No demo video is generated: these show as missing files.
        f.path = folder.join(name);
        files.push(f);
    }
    // The take's metadata, as Lavdeck writes it, so any reader of the folder sees the same take.
    let meta_path = folder.join(format!("{stem}.take.json"));
    let mics: Vec<serde_json::Value> = [(1u8, names.0, "a1b2c3"), (2, names.1, "d4e5f6")]
        .iter()
        .map(|(i, name, id)| {
            let armed = kind != Kind::Solo || *i == 1;
            let name = if name.is_empty() {
                format!("TX{i}")
            } else {
                name.to_string()
            };
            serde_json::json!({"index": i, "name": name, "bluetoothID": id, "armed": armed})
        })
        .collect();
    let media: Vec<serde_json::Value> = files
        .iter()
        .filter(|f| f.role == Role::Capture)
        .map(|f| serde_json::json!({"file": f.name(), "source": f.source, "kind": f.kind, "offset": f.offset}))
        .collect();
    let mut meta = serde_json::json!({
        "version": 1,
        "started": iso8601(started),
        "duration": length,
        "channelMode": "stereo",
        "format": format.key(),
        "mics": mics,
    });
    if !media.is_empty() {
        meta["media"] = serde_json::Value::Array(media);
    }
    let _ = std::fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    );
    files.push(TakeFile::new(meta_path, Role::Metadata));
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Take {
        id: format!("{day}/{stem}"),
        folder,
        stem,
        started: Some(started),
        duration: Some(length),
        channel_mode: Some("stereo".into()),
        files,
        speakers,
    }
}

/// `2026-10-26T15:05:09Z`, as Foundation's ISO 8601 encoder writes dates.
fn iso8601(t: f64) -> String {
    // SAFETY: gmtime_r fills the zeroed tm from a valid time.
    unsafe {
        let secs = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::gmtime_r(&secs, &mut tm);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

fn library(root: &Path) -> Vec<Take> {
    entries()
        .into_iter()
        .map(|e| {
            let (y, mo, d, h, mi, s) = e.when;
            let mut take = build_take(
                root,
                local(y, mo, d, h, mi, s),
                e.length,
                e.kind,
                e.names,
                e.video,
                RecordingFormat::Wav,
            );
            if let Some(lines) = e.transcript {
                write_transcript_lines(&take, lines);
                take.files.push(transcript_file(&take));
                take.files.sort_by(|a, b| a.path.cmp(&b.path));
            }
            take
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn new_take(
    root: &Path,
    started: f64,
    length: f64,
    mode: Option<ChannelMode>,
    armed: &std::collections::BTreeSet<u8>,
    format: RecordingFormat,
    split: bool,
    names: (String, String),
    video: &crate::model::settings::VideoSettings,
) -> Take {
    let kind = if armed.len() == 1 {
        Kind::Solo
    } else if mode == Some(ChannelMode::Stereo) && split && format == RecordingFormat::Wav {
        Kind::TwoSpeaker
    } else {
        Kind::Stereo
    };
    let sources: Vec<&str> = [
        video.screen.then_some("screen"),
        video.camera.then_some("camera"),
    ]
    .into_iter()
    .flatten()
    .collect();
    build_take(
        root,
        started,
        length.max(1.0),
        kind,
        (&names.0, &names.1),
        &sources,
        format,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header() {
        let w = wav(&[&[0, 1, -1], &[2, 3, 4]]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize,
            w.len() - 8
        );
        assert_eq!(u16::from_le_bytes(w[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 12);
    }

    #[test]
    fn segments_follow_each_other() {
        let s = segments(FIRST);
        assert_eq!(s.len(), FIRST.len());
        assert!(s.windows(2).all(|w| w[0].1 < w[1].0));
        assert!(
            s.last().unwrap().1 < CLIP_SECONDS,
            "the clip covers the first transcript"
        );
    }
}
