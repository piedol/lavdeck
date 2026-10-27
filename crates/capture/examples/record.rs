//! Records a short take's video live, the way Record does, then makes its finished video:
//!
//! ```text
//! cargo run -p lavdeck-capture --example record -- [--screen] [--camera] [--seconds 5] [--out DIR]
//!     [--system-audio] [--fps 60] [--h264] [--no-preview] [--preview large|full]
//! ```
//!
//! Starts the camera (with its preview) and the screen, waits up to 5 s for their first frames, takes that
//! moment as the audio's first sample, records for `--seconds`, stops, prints each file's offset, writes a
//! silent `<stem> Stereo.wav` and the kit's `<stem>.take.json` in its place, reads the take back with the
//! kit's take library and composes `<stem> PiP.mp4`.
//!
//! It never asks for permissions: a source whose permission this process (or the terminal it runs in)
//! doesn't already hold is reported and skipped. Run it from a signed build that has Camera and Screen &
//! System Audio Recording access.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use lavdeck_capture::{
    Codec, FIRST_FRAME_TIMEOUT, PERMISSION_CAMERA, PERMISSION_SCREEN_RECORDING, PreviewSize,
    VideoCapture, VideoComposer, VideoSettings, wait_until,
};
use lavdeck_kit::{CaptureResult, HostClock, TakeMedia, TakeMetadata, takes};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeVideo};
use objc2_core_graphics::CGPreflightScreenCaptureAccess;

struct Options {
    settings: VideoSettings,
    seconds: f64,
    out: PathBuf,
}

fn options() -> Options {
    let mut settings = VideoSettings {
        make_video: true,
        ..VideoSettings::default()
    };
    let mut seconds = 5.0;
    let mut out = std::env::temp_dir().join("lavdeck-record");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--screen" => settings.screen = true,
            "--camera" => settings.camera = true,
            "--system-audio" => settings.system_audio = true,
            "--h264" => settings.codec = Codec::H264,
            "--no-preview" => settings.camera_preview = false,
            "--fps" => {
                settings.screen_fps = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--fps 30|60")
            }
            "--preview" => {
                settings.preview_size = args
                    .next()
                    .and_then(|v| PreviewSize::from_raw(&v))
                    .expect("--preview")
            }
            "--seconds" => {
                seconds = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--seconds N")
            }
            "--out" => out = PathBuf::from(args.next().expect("--out DIR")),
            other => panic!("unknown option {other}"),
        }
    }
    Options {
        settings,
        seconds,
        out,
    }
}

/// What this process already holds; checking never prompts.
fn granted() -> HashSet<String> {
    let mut granted = HashSet::new();
    if CGPreflightScreenCaptureAccess() {
        granted.insert(PERMISSION_SCREEN_RECORDING.to_string());
    }
    let video = unsafe { AVMediaTypeVideo }.expect("AVMediaTypeVideo");
    if unsafe { AVCaptureDevice::authorizationStatusForMediaType(video) }
        == AVAuthorizationStatus::Authorized
    {
        granted.insert(PERMISSION_CAMERA.to_string());
    }
    granted
}

fn write_silent_wav(path: &Path, seconds: f64) {
    let rate = 48_000u32;
    let bytes = (seconds * rate as f64) as u32 * 4;
    let mut wav = Vec::with_capacity(44 + bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    for v in [
        16u32.to_le_bytes().as_slice(),
        &1u16.to_le_bytes(),
        &2u16.to_le_bytes(),
        &rate.to_le_bytes(),
    ] {
        wav.extend_from_slice(v);
    }
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    wav.resize(44 + bytes as usize, 0);
    std::fs::write(path, wav).expect("write the WAV");
}

fn finished(options: &Options, stem: &str, audio_start: f64, result: CaptureResult) {
    for problem in &result.problems {
        println!("problem: {problem}");
    }
    write_silent_wav(
        &options.out.join(format!("{stem} Stereo.wav")),
        options.seconds,
    );
    // The take's metadata, as Record writes it once the audio is saved.
    let mut metadata = TakeMetadata::new(
        SystemTime::now(),
        Some(options.seconds),
        Some("stereo".into()),
        "wav",
        Vec::new(),
    );
    metadata.audio_start_host_time = Some(audio_start);
    let media: Vec<TakeMedia> = result
        .outputs
        .iter()
        .map(|output| {
            let offset = output.offset(Some(audio_start));
            println!("{} → {} (offset {offset:?} s)", output.source, output.path);
            TakeMedia {
                file: Path::new(&output.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                source: output.source.clone(),
                kind: output.kind.clone(),
                offset,
                plugin: Some(output.plugin_id.clone()),
            }
        })
        .collect();
    metadata.media = (!media.is_empty()).then_some(media);
    metadata
        .write(&options.out.join(format!("{stem}.take.json")))
        .expect("write the take's metadata");
    let take = takes::take(&options.out, stem, None, &HashMap::new()).expect("the take");
    if !take.descriptor.has("video") {
        println!("Nothing recorded.");
        std::process::exit(0);
    }
    let settings = options.settings.clone();
    std::thread::spawn(move || {
        match VideoComposer::make(&take.descriptor, &settings, |p| {
            println!("composing {:.0}%", p * 100.0)
        }) {
            Ok(path) => println!("Saved {}", path.display()),
            Err(error) => println!("Couldn’t make the video: {error}"),
        }
        std::process::exit(0);
    });
}

fn main() {
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let options = Rc::new(options());
    std::fs::create_dir_all(&options.out).expect("output folder");
    let granted = granted();
    println!("granted: {granted:?}");
    let (_, stem) = takes::day_and_stem(SystemTime::now());

    let capture: Rc<RefCell<Option<VideoCapture>>> = Rc::new(RefCell::new(None));
    let events = capture.clone();
    let video = VideoCapture::new(
        mtm,
        |s| s.to_string(),
        move || {
            if let Some(c) = events.try_borrow().ok().as_ref().and_then(|c| c.as_ref()) {
                println!("started {:?}, failed {:?}", c.started(), c.failed());
            }
        },
    );
    *capture.borrow_mut() = Some(video);
    let expected = VideoCapture::expected(&options.settings, &granted);
    let (c, o, s) = (capture.clone(), options.clone(), stem.clone());
    capture.borrow().as_ref().unwrap().start(
        &options.out,
        &stem,
        &options.settings,
        &granted,
        move || {
            let (c2, o2, s2) = (c.clone(), o.clone(), s.clone());
            c.borrow().as_ref().unwrap().wait_for_first_frames(
                expected,
                FIRST_FRAME_TIMEOUT,
                move || {
                    // Stands in for the mic recorder's first sample.
                    let audio_start = HostClock::now();
                    println!("audio starts at host time {audio_start:.3}");
                    let (c3, o3, s3) = (c2.clone(), o2.clone(), s2.clone());
                    wait_until(
                        Duration::from_secs_f64(o2.seconds),
                        || false,
                        move || {
                            c3.borrow()
                                .as_ref()
                                .unwrap()
                                .stop(move |result| finished(&o3, &s3, audio_start, result));
                        },
                    );
                },
            );
        },
    );
    app.run();
}
