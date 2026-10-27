//! The seam between the interface and the hardware, the recorder and the take library.
//!
//! The views and [`crate::store`] only ever call a [`Backend`]. Phase 1 has two: the
//! [`crate::demo`] backend (made-up mics and takes, like the Swift app's `--demo`) and
//! [`Offline`] (no receiver, no takes) for a normal launch until the kit and capture crates
//! are wired in. Phase 2 adds the real one over `lavdeck-kit` (mic controller, recorder, take
//! library, plugin runner) and `lavdeck-capture` (screen and camera), reporting back through
//! the `store::*` callbacks named on each method.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model::plugins::{Launch, Plugin};
use crate::model::settings::{RecordingFormat, Settings};
use crate::model::status::ChannelMode;
use crate::model::takes::{Take, TranscriptDocument};

/// Called once with the outcome of a receiver write.
pub type Done = Box<dyn FnOnce(Result<(), String>)>;

/// What a take needs to start. The backend picks file names (`<root>/<yyyy-MM-dd>/<HH.mm.ss>
/// …`) and does the capture-then-audio start order of `AppModel.startRecording`.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordRequest {
    pub root: PathBuf,
    pub format: RecordingFormat,
    pub split_tracks: bool,
    /// Mics that record; one means that mic alone (from a stereo take).
    pub armed: Vec<u8>,
    pub mode: Option<ChannelMode>,
    pub settings: Settings,
}

pub trait Backend {
    /// Starts watching for the receiver. Status and link changes arrive through
    /// `store::receiver_status` and `store::receiver_connected`; a press of a transmitter's
    /// link button (when `tapToRecord` is on) through `store::toggle_recording`.
    fn start(&self);

    /// Switches the receiver's channel mode; `done` runs once it's confirmed or failed.
    fn set_channel(&self, mode: ChannelMode, done: Done);

    /// Sets the receiver's gain (−12, −6, 0, +6, +12 dB).
    fn set_gain(&self, db: i32, done: Done);

    /// Starts a take. `done(Ok)` when audio is running (capture sources report through
    /// `store::capture_started`); the take ends through `store::recording_finished`.
    fn start_recording(&self, request: RecordRequest, done: Done);

    /// Stops the take; files are finished and reported through `store::recording_finished`.
    fn stop_recording(&self);

    /// Per-channel dBFS of the running take (polled ~15 times a second).
    fn channel_levels(&self) -> Vec<f32>;

    /// Every take under `root`, newest first.
    fn scan_takes(&self, root: &Path, mic_names: &BTreeMap<String, String>) -> Vec<Take>;

    fn load_transcript(&self, take: &Take) -> Option<TranscriptDocument>;

    /// Moves every file of a take to the Trash.
    fn trash(&self, take: &Take) -> Result<(), String>;

    /// Installed plugins and the problems found loading the others.
    fn discover_plugins(&self) -> (Vec<Plugin>, Vec<String>);

    fn install_plugin(&self, folder: &Path) -> Result<(), String>;

    fn plugins_folder(&self) -> PathBuf;

    /// Runs a job; progress and the end come back through `store::job_progress` and
    /// `store::job_finished`.
    fn launch(&self, launch: Launch, take: Take);

    /// Stops a running job (it then finishes as cancelled).
    fn cancel(&self, job_id: u64);

    /// Opens a take in an app plugin.
    fn open_in_app(&self, take: &Take, plugin: &Plugin) -> Result<(), String>;

    /// Persists the app's settings (no-op in the demo).
    fn save_settings(&self, settings: &Settings);

    /// Persists plugins' switches and settings (no-op in the demo).
    fn save_plugins(
        &self,
        enabled: &BTreeMap<String, bool>,
        plugin_id: Option<&str>,
        values: Option<&BTreeMap<String, crate::model::plugins::JsonValue>>,
    );

    /// Whether permission prompts may be shown (never in the demo).
    fn may_prompt(&self) -> bool;
}

/// A normal launch before the kit and capture crates are wired in: no receiver, no takes.
pub struct Offline;

impl Backend for Offline {
    fn start(&self) {}

    fn set_channel(&self, _mode: ChannelMode, done: Done) {
        done(Err("no receiver".into()));
    }

    fn set_gain(&self, _db: i32, done: Done) {
        done(Err("no receiver".into()));
    }

    fn start_recording(&self, _request: RecordRequest, done: Done) {
        done(Err("no receiver".into()));
    }

    fn stop_recording(&self) {}

    fn channel_levels(&self) -> Vec<f32> {
        Vec::new()
    }

    fn scan_takes(&self, _root: &Path, _names: &BTreeMap<String, String>) -> Vec<Take> {
        Vec::new()
    }

    fn load_transcript(&self, take: &Take) -> Option<TranscriptDocument> {
        TranscriptDocument::load(&take.transcript_path()).ok()
    }

    fn trash(&self, _take: &Take) -> Result<(), String> {
        Err("not available yet".into())
    }

    fn discover_plugins(&self) -> (Vec<Plugin>, Vec<String>) {
        (Vec::new(), Vec::new())
    }

    fn install_plugin(&self, _folder: &Path) -> Result<(), String> {
        Err("not available yet".into())
    }

    fn plugins_folder(&self) -> PathBuf {
        crate::util::home().join("Library/Application Support/Lavdeck/Plugins")
    }

    fn launch(&self, launch: Launch, _take: Take) {
        let id = launch.job_id;
        crate::util::main_async(move || {
            crate::store::job_finished(
                id,
                crate::model::plugins::JobState::Failed("not available yet".into()),
            )
        });
    }

    fn cancel(&self, _job_id: u64) {}

    fn open_in_app(&self, _take: &Take, _plugin: &Plugin) -> Result<(), String> {
        Err("not available yet".into())
    }

    fn save_settings(&self, settings: &Settings) {
        crate::prefs::save(settings);
    }

    fn save_plugins(
        &self,
        _e: &BTreeMap<String, bool>,
        _id: Option<&str>,
        _v: Option<&BTreeMap<String, crate::model::plugins::JsonValue>>,
    ) {
    }

    fn may_prompt(&self) -> bool {
        true
    }
}
