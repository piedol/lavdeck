//! The live view model and the backend behind it. Views read with [`with`], act through the
//! functions here, and are told to redraw through [`observe`] (coalesced: one pass per run-loop
//! turn, however many changes). The backend reports back through the callbacks at the end.
//!
//! This follows `AppModel.swift` for what the deck needs; the device and recording
//! orchestration itself is the backend's (see [`crate::backend`]).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2_app_kit::NSBeep;

use crate::backend::{Backend, RecordRequest};
use crate::format;
use crate::i18n::{tr, trf};
use crate::model::AppState;
use crate::model::plugins::{CaptureToggle, JobState, JsonValue, Launch, VIDEO_JOB_ID, Work};
use crate::model::settings::Settings;
use crate::model::status::{ChannelMode, MicStatus};
use crate::model::takes::Take;
use crate::util;

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
    static BACKEND: RefCell<Option<Rc<dyn Backend>>> = const { RefCell::new(None) };
    static OBSERVERS: RefCell<Vec<Rc<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    static SCHEDULED: Cell<bool> = const { Cell::new(false) };
    /// Takes for queued jobs, handed to the backend when the job starts.
    static JOB_TAKES: RefCell<HashMap<u64, Take>> = RefCell::new(HashMap::new());
    /// Bumped to cancel the level poll and the identify timeout.
    static LEVEL_RUN: Cell<u64> = const { Cell::new(0) };
    static IDENTIFY_RUN: Cell<u64> = const { Cell::new(0) };
}

pub fn install(state: AppState, backend: Rc<dyn Backend>) {
    STATE.with(|s| *s.borrow_mut() = Some(state));
    BACKEND.with(|b| *b.borrow_mut() = Some(backend));
}

pub fn backend() -> Rc<dyn Backend> {
    BACKEND.with(|b| b.borrow().clone().expect("store installed"))
}

pub fn with<R>(f: impl FnOnce(&AppState) -> R) -> R {
    STATE.with(|s| f(s.borrow().as_ref().expect("store installed")))
}

/// Changes the state and schedules a redraw.
pub fn update<R>(f: impl FnOnce(&mut AppState) -> R) -> R {
    let r = STATE.with(|s| f(s.borrow_mut().as_mut().expect("store installed")));
    changed();
    r
}

/// Calls `f` after state changes (coalesced, on the main queue).
pub fn observe(f: impl Fn() + 'static) {
    OBSERVERS.with(|o| o.borrow_mut().push(Rc::new(f)));
}

fn changed() {
    if SCHEDULED.with(|s| s.replace(true)) {
        return;
    }
    util::main_async(notify_now);
}

/// Runs the observers now (snapshots draw right after a change).
pub fn notify_now() {
    SCHEDULED.with(|s| s.set(false));
    let observers = OBSERVERS.with(|o| o.borrow().clone());
    for o in observers {
        o();
    }
}

// MARK: Controls

pub fn toggle_arm(index: u8) {
    update(|s| {
        if s.is_recording {
            return;
        }
        if !s.settings.armed.remove(&index) {
            s.settings.armed.insert(index);
        }
    });
    save_settings();
}

pub fn rename(index: u8, name: &str) {
    let demo = with(|s| s.demo);
    let changed = update(|s| {
        let Some((id, value)) = s.renamed(index, name) else {
            return false;
        };
        let names = if s.demo {
            &mut s.demo_names
        } else {
            &mut s.settings.mic_names
        };
        match value {
            Some(v) => names.insert(id, v),
            None => names.remove(&id),
        };
        true
    });
    if changed && !demo {
        save_settings();
    }
}

pub fn set_channel(mode: ChannelMode) {
    let go = update(|s| {
        if s.is_recording || s.pending_channel.is_some() || s.channel_mode() == Some(mode) {
            return false;
        }
        s.pending_channel = Some(mode);
        s.last_error = None;
        true
    });
    if !go {
        return;
    }
    backend().set_channel(
        mode,
        Box::new(move |r| {
            update(|s| {
                if let Err(e) = r {
                    s.last_error = Some(trf(
                        "Couldn’t switch to {mode}: {error}",
                        &[("mode", &mode_name(mode)), ("error", &e)],
                    ));
                }
                s.pending_channel = None;
            })
        }),
    );
}

/// "mono", "stereo", in a sentence.
pub fn mode_name(mode: ChannelMode) -> String {
    match mode {
        ChannelMode::Mono => tr("mono"),
        ChannelMode::Stereo => tr("stereo"),
        ChannelMode::Other(v) => trf("mode {value}", &[("value", &v.to_string())]),
    }
}

pub fn set_gain(db: i32) {
    let go = update(|s| {
        if s.pending_gain.is_some() || s.status.receiver.gain_db == Some(db) {
            return false;
        }
        s.pending_gain = Some(db);
        s.last_error = None;
        true
    });
    if !go {
        return;
    }
    backend().set_gain(
        db,
        Box::new(move |r| {
            update(|s| {
                if let Err(e) = r {
                    s.last_error = Some(trf(
                        "Couldn’t set gain to {db} dB: {error}",
                        &[("db", &db.to_string()), ("error", &e)],
                    ));
                }
                s.pending_gain = None;
            })
        }),
    );
}

// MARK: Recording

pub fn toggle_recording() {
    if with(|s| s.is_recording) {
        stop_recording();
    } else {
        start_recording();
    }
}

pub fn start_recording() {
    let (busy, none_armed) =
        with(|s| (s.is_recording || s.is_starting, s.settings.armed.is_empty()));
    if busy {
        return;
    }
    if none_armed {
        // The hot key and mic taps work with the deck closed, so say why nothing happened.
        NSBeep();
        update(|s| {
            s.last_error = Some(tr(
                "Both mics are switched off. Click OFF under a mic to arm it, then record.",
            ))
        });
        return;
    }
    update(|s| {
        s.last_error = None;
        // A new take makes the last take's processing failure old news.
        while let Some(id) = s.plugins.automatic_failure().map(|j| j.id) {
            s.plugins.dismiss(id);
        }
        s.is_starting = true;
    });
    if with(|s| s.identifying) {
        end_identify();
    }
    let (solo, mode) = with(|s| (s.solo_mic(), s.channel_mode()));
    if solo.is_some() && mode != Some(ChannelMode::Stereo) {
        // One mic alone needs the receiver in stereo (TX1 left, TX2 right); mono mixes both.
        backend().set_channel(
            ChannelMode::Stereo,
            Box::new(|r| match r {
                Ok(()) => begin_take(),
                Err(e) => update(|s| {
                    s.is_starting = false;
                    s.last_error = Some(trf(
                        "Couldn’t switch to stereo to record one mic: {error}",
                        &[("error", &e)],
                    ));
                }),
            }),
        );
    } else {
        begin_take();
    }
}

fn begin_take() {
    let request = with(|s| RecordRequest {
        root: s.settings.recordings_root.clone(),
        format: s.settings.format,
        split_tracks: s.settings.split_tracks,
        armed: s.settings.armed.iter().copied().collect(),
        mode: s.channel_mode(),
        settings: s.settings.clone(),
    });
    update(|s| {
        s.capture_sources.clear();
    });
    backend().start_recording(
        request,
        Box::new(|r| {
            update(|s| {
                s.is_starting = false;
                match r {
                    Ok(()) => {
                        s.recording_mode = s.channel_mode();
                        s.recording_started = Some(format::now());
                        s.is_recording = true;
                    }
                    Err(e) => {
                        s.last_error =
                            Some(trf("Couldn’t start recording: {error}", &[("error", &e)]))
                    }
                }
            });
            if with(|s| s.is_recording) {
                poll_levels(LEVEL_RUN.with(|r| r.get() + 1));
            }
        }),
    );
}

fn poll_levels(run: u64) {
    LEVEL_RUN.with(|r| r.set(run));
    let levels = backend().channel_levels();
    update(|s| s.recording_levels = levels);
    util::later(1.0 / 15.0, move || {
        if LEVEL_RUN.with(|r| r.get()) == run && with(|s| s.is_recording) {
            poll_levels(run);
        }
    });
}

pub fn stop_recording() {
    if with(|s| s.is_recording) {
        backend().stop_recording();
    }
}

// MARK: Identify

pub fn start_identify() {
    let go = update(|s| {
        if s.is_recording || s.identifying || !s.is_connected {
            return None;
        }
        s.identifying = true;
        s.talking_mic = None;
        let mode = s.channel_mode();
        (mode != Some(ChannelMode::Stereo)).then_some(mode)
    });
    let Some(restore) = go else {
        if with(|s| s.identifying) {
            schedule_identify_timeout();
        }
        return;
    };
    update(|s| s.identify_restore = restore);
    set_channel(ChannelMode::Stereo);
    schedule_identify_timeout();
}

fn schedule_identify_timeout() {
    let run = IDENTIFY_RUN.with(|r| {
        r.set(r.get() + 1);
        r.get()
    });
    util::later(45.0, move || {
        if IDENTIFY_RUN.with(|r| r.get()) == run {
            stop_identify();
        }
    });
}

pub fn stop_identify() {
    if with(|s| s.identifying) {
        end_identify();
    }
}

/// Leaves identify mode and, once any in-flight switch has settled, restores the mode.
fn end_identify() {
    IDENTIFY_RUN.with(|r| r.set(r.get() + 1));
    let restore = update(|s| {
        s.identifying = false;
        s.talking_mic = None;
        s.identify_restore.take()
    });
    if let Some(restore) = restore {
        restore_mode(restore, 40);
    }
}

fn restore_mode(mode: ChannelMode, tries: u32) {
    if with(|s| s.pending_channel.is_some()) && tries > 0 {
        util::later(0.1, move || restore_mode(mode, tries - 1));
        return;
    }
    if with(|s| s.channel_mode() == Some(mode)) {
        return;
    }
    update(|s| s.pending_channel = Some(mode));
    backend().set_channel(
        mode,
        Box::new(move |r| {
            update(|s| {
                if let Err(e) = r {
                    s.last_error = Some(trf(
                        "Couldn’t switch back to {mode}: {error}",
                        &[("mode", &mode_name(mode)), ("error", &e)],
                    ));
                }
                s.pending_channel = None;
            })
        }),
    );
}

// MARK: Capture switches and settings

/// Switches Lavdeck's own screen or camera recording for the next take, asking for its
/// permission now rather than at Record.
pub fn set_video_source(source: &str, on: bool) {
    update(|s| match source {
        "screen" => s.settings.video.screen = on,
        "camera" => s.settings.video.camera = on,
        _ => {}
    });
    save_settings();
    if on && let Some(p) = crate::permissions::for_source(source) {
        crate::permissions::request(&[p]);
    }
}

pub fn set_capture(toggle: &CaptureToggle, on: bool) {
    update(|s| s.plugins.set_on(toggle, on));
    save_plugins(Some(&toggle.plugin_id));
    if on && let Some(p) = crate::permissions::for_source(&toggle.source) {
        crate::permissions::request(&[p]);
    }
}

/// Changes settings and saves them.
pub fn set_settings(f: impl FnOnce(&mut Settings)) {
    update(|s| f(&mut s.settings));
    save_settings();
}

fn save_settings() {
    let settings = with(|s| s.settings.clone());
    backend().save_settings(&settings);
}

fn save_plugins(plugin: Option<&str>) {
    let (enabled, values) = with(|s| {
        (
            s.plugins.enabled.clone(),
            plugin.map(|id| s.plugins.stored.get(id).cloned().unwrap_or_default()),
        )
    });
    backend().save_plugins(&enabled, plugin, values.as_ref());
}

pub fn set_launch_at_login(on: bool) {
    match crate::login::set(on) {
        Ok(()) => update(|s| s.launch_at_login = crate::login::enabled()),
        Err(e) => update(|s| s.last_error = Some(trf("Login item: {error}", &[("error", &e)]))),
    }
}

// MARK: Plugins

pub fn reload_plugins() {
    let (plugins, problems) = backend().discover_plugins();
    update(|s| {
        s.plugins.plugins = plugins;
        s.plugins.problems = problems;
    });
}

pub fn set_plugin_enabled(id: &str, on: bool) {
    update(|s| s.plugins.set_enabled(id, on));
    save_plugins(None);
    if on {
        let needed = with(|s| {
            s.plugins
                .plugin(id)
                .map(|p| p.manifest.permissions.clone())
                .unwrap_or_default()
        });
        let needed: Vec<&str> = needed.iter().map(String::as_str).collect();
        crate::permissions::request(&needed);
    }
}

pub fn set_plugin_value(id: &str, key: &str, value: JsonValue) {
    update(|s| s.plugins.set_value(id, key, value));
    save_plugins(Some(id));
}

fn dispatch(launch: Option<Launch>, take: Option<Take>) {
    let Some(launch) = launch else {
        if let Some(t) = take {
            // Queued: remember the take for when it starts.
            let id = with(|s| s.plugins.jobs.last().map(|j| j.id));
            if let Some(id) = id {
                JOB_TAKES.with(|m| m.borrow_mut().insert(id, t));
            }
        }
        return;
    };
    let take = take.or_else(|| JOB_TAKES.with(|m| m.borrow_mut().remove(&launch.job_id)));
    if let Some(take) = take {
        backend().launch(launch, take);
    }
}

/// Runs a plugin action on each take (takes it's already busy with are skipped).
pub fn run_action(plugin_id: &str, action_id: &str, takes: &[Take]) {
    for take in takes {
        let (launch, queued) = update(|s| {
            let before = s.plugins.jobs.len();
            let l = s.plugins.run(plugin_id, action_id, &take.id);
            (l.clone(), l.is_none() && s.plugins.jobs.len() > before)
        });
        if launch.is_some() || queued {
            dispatch(launch, Some(take.clone()));
        }
    }
}

/// Makes (or remakes) each take's finished video, one at a time, as jobs.
pub fn make_video(takes: &[Take], automatic: bool) {
    for take in takes.iter().filter(|t| t.has("video")) {
        let (launch, queued) = update(|s| {
            let before = s.plugins.jobs.len();
            let l = s.plugins.run_built_in(
                VIDEO_JOB_ID,
                &tr("Video"),
                &tr("Making the video"),
                &take.id,
                automatic,
                Work::MakeVideo {
                    take_id: take.id.clone(),
                },
            );
            (l.clone(), l.is_none() && s.plugins.jobs.len() > before)
        });
        if launch.is_some() || queued {
            dispatch(launch, Some(take.clone()));
        }
    }
}

pub fn cancel_job(job_id: u64) {
    if update(|s| s.plugins.cancel(job_id)) {
        backend().cancel(job_id);
    } else {
        JOB_TAKES.with(|m| m.borrow_mut().remove(&job_id));
    }
}

pub fn cancel_jobs_for(take_ids: &[String]) {
    for id in update(|s| s.plugins.cancel_jobs_for(take_ids)) {
        backend().cancel(id);
    }
}

pub fn dismiss_job(job_id: u64) {
    update(|s| s.plugins.dismiss(job_id));
}

pub fn open_in_app(take: &Take, plugin_id: &str) {
    let Some(plugin) = with(|s| s.plugins.plugin(plugin_id).cloned()) else {
        return;
    };
    if let Err(e) = backend().open_in_app(take, &plugin) {
        update(|s| {
            s.plugins
                .problems
                .push(format!("{}: {e}", plugin.manifest.name))
        });
    }
}

pub fn note_library_changed() {
    update(|s| s.library_revision += 1);
}

pub fn reveal_recordings() {
    let folder = with(|s| {
        s.last_saved_folder
            .clone()
            .unwrap_or_else(|| s.settings.recordings_root.clone())
    });
    let _ = std::fs::create_dir_all(&folder);
    util::reveal(&[folder]);
}

// MARK: Backend callbacks

pub fn receiver_status(status: MicStatus) {
    update(|s| {
        if s.demo {
            return;
        }
        s.known_transmitters
            .extend(status.receiver.connected_transmitters.iter().copied());
        if !s.is_connected && status.receiver.channel_mode.is_some() {
            s.is_connected = true;
        }
        s.status = status;
    });
}

pub fn receiver_connected(connected: bool) {
    update(|s| s.is_connected = connected);
}

/// Identify mode's verdict on who is talking (from the kit's talking detector).
pub fn set_talking(mic: Option<u8>) {
    update(|s| {
        if s.identifying {
            s.talking_mic = mic;
        }
    });
}

pub fn capture_started(source: &str) {
    update(|s| {
        if !s.capture_sources.iter().any(|c| c == source) {
            s.capture_sources.push(source.to_string());
        }
    });
}

pub fn capture_failed(message: String) {
    update(|s| s.last_error = Some(message));
}

/// The take ended: `Ok(Some(take))` when its files and metadata are written, `Ok(None)` when
/// nothing was kept, `Err` when it stopped with an error.
pub fn recording_finished(
    result: Result<Option<Take>, String>,
    folder: Option<std::path::PathBuf>,
) {
    LEVEL_RUN.with(|r| r.set(r.get() + 1));
    update(|s| {
        s.is_recording = false;
        s.recording_levels.clear();
        s.recording_started = None;
        s.capture_sources.clear();
        if folder.is_some() {
            s.last_saved_folder = folder;
        }
        if let Err(e) = &result {
            s.last_error = Some(trf(
                "Recording stopped with an error: {error}",
                &[("error", e)],
            ));
        }
    });
    if let Ok(Some(take)) = result {
        let launches = update(|s| s.plugins.take_finished(&take));
        for l in launches {
            backend().launch(l, take.clone());
        }
        if with(|s| s.settings.video.make_video) {
            make_video(std::slice::from_ref(&take), true);
        }
        note_library_changed();
    }
}

pub fn job_progress(job_id: u64, fraction: Option<f64>, message: Option<String>) {
    update(|s| s.plugins.progress(job_id, fraction, message));
}

pub fn job_finished(job_id: u64, state: JobState) {
    let next = update(|s| s.plugins.finish(job_id, state));
    JOB_TAKES.with(|m| m.borrow_mut().remove(&job_id));
    dispatch(next, None);
}
