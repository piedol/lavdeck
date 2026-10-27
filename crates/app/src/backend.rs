//! The seam between the interface and the hardware, the recorder and the take library.
//!
//! The views and [`crate::store`] only ever call a [`Backend`]: the real one,
//! [`crate::live::Live`] over `lavdeck-kit` (receiver, recorder, take library, plugins) and
//! `lavdeck-capture` (screen and camera), or the [`crate::demo`] one (made-up mics and takes,
//! like the Swift app's `--demo`). Backends report back through the `store::*` callbacks named
//! on each method.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model::plugins::{JsonValue, Launch, Plugin};
use crate::model::settings::{RecordingFormat, Settings};
use crate::model::status::ChannelMode;
use crate::model::takes::{Take, TranscriptDocument};

/// Called once with the outcome of a receiver write or a start, in words for the deck.
pub type Done = Box<dyn FnOnce(Result<(), String>)>;

/// Scans a recordings root on a background thread.
pub type Scanner = fn(&Path, &BTreeMap<String, String>) -> Vec<Take>;

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

    /// A scan that can run off the main thread (the take library reads audio durations), or
    /// `None` to scan with [`Backend::scan_takes`] on the main thread.
    fn scanner(&self) -> Option<Scanner> {
        None
    }

    fn load_transcript(&self, take: &Take) -> Option<TranscriptDocument>;

    /// Moves every file of a take to the Trash.
    fn trash(&self, take: &Take) -> Result<(), String>;

    /// Installed plugins and the problems found loading the others.
    fn discover_plugins(&self) -> (Vec<Plugin>, Vec<String>);

    /// Plugins' switches and their settings (defaults overlaid with the user's), as saved.
    fn plugin_state(
        &self,
        _plugins: &[Plugin],
    ) -> (
        BTreeMap<String, bool>,
        BTreeMap<String, BTreeMap<String, JsonValue>>,
    ) {
        Default::default()
    }

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
