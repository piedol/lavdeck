//! The finished video from synthetic takes: a screen-like and a camera-like clip (generated frames written
//! with AVAssetWriter) and a WAV, composed offline into `PiP.mp4` and checked for duration, tracks, size,
//! where the camera bubble sits and how the clips line up with the audio.
//!
//! Set `LAVDECK_COMPOSE_KEEP=<folder>` to keep the inputs and outputs there (for comparing with the Swift
//! composer).

mod synth;

use std::path::{Path, PathBuf};

use lavdeck_capture::{
    BubbleShape, BubbleSize, CaptureError, Codec, Corner, VideoComposer, VideoSettings,
};
use lavdeck_kit::{TakeDescriptor, TakeFile, TakeRole};

const STEM: &str = "09.00.00";
/// The screen started 0.1 s after the first audio sample, the camera 0.5 s before it.
const SCREEN_OFFSET: f64 = 0.1;
const CAMERA_OFFSET: f64 = -0.5;
const AUDIO_SECONDS: f64 = 3.0;

const BLUE: (u8, u8, u8) = (200, 80, 30); // (b, g, r) for the writer
const RED: (u8, u8, u8) = (30, 30, 210);
const YELLOW: (u8, u8, u8) = (30, 210, 210);
const GREEN: (u8, u8, u8) = (40, 200, 40);

struct Folder {
    path: PathBuf,
    keep: bool,
}

impl Folder {
    fn new(name: &str) -> Folder {
        let (path, keep) = match std::env::var_os("LAVDECK_COMPOSE_KEEP") {
            Some(dir) => (PathBuf::from(dir).join(name), true),
            None => (
                Path::new(env!("CARGO_TARGET_TMPDIR"))
                    .join("lavdeck-compose")
                    .join(name),
                false,
            ),
        };
        std::fs::create_dir_all(&path).unwrap();
        Folder { path, keep }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(format!("{STEM} {name}"))
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        // Only ever under target/tmp.
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// A 1280x800 "screen": blue, a band of bars, and a white box that moves.
fn screen_clip(path: &Path) {
    synth::write_clip(path, 1280, 800, 96, true, |frame, x, y| {
        let bx = (frame * 8) % 1200;
        if (600..680).contains(&y) && (bx..bx + 80).contains(&x) {
            (255, 255, 255)
        } else if (300..400).contains(&y) {
            let bar = (x / 160) as u8;
            (bar * 30, 255 - bar * 30, 128)
        } else {
            BLUE
        }
    });
}

/// A 640x480 "camera": a green disc in the middle, on red for its first second and yellow after.
fn camera_clip(path: &Path) {
    synth::write_clip(path, 640, 480, 108, false, |frame, x, y| {
        let (dx, dy) = (x as f64 - 320.0, y as f64 - 240.0);
        if dx * dx + dy * dy < 100.0 * 100.0 {
            GREEN
        } else if frame < 30 {
            RED
        } else {
            YELLOW
        }
    });
}

fn take(folder: &Folder, files: Vec<TakeFile>) -> TakeDescriptor {
    let path = folder.path.to_string_lossy();
    TakeDescriptor {
        files,
        ..TakeDescriptor::new(&format!("test/{STEM}"), &path, STEM)
    }
}

fn audio(path: PathBuf, role: TakeRole) -> TakeFile {
    TakeFile::new(&path.to_string_lossy(), role)
}

/// A captured video, as the take library lists it from `.take.json`.
fn captured(path: PathBuf, source: &str, offset: Option<f64>) -> TakeFile {
    TakeFile {
        kind: Some("video".into()),
        source: Some(source.into()),
        offset,
        ..audio(path, TakeRole::Capture)
    }
}

fn close(rgb: (u8, u8, u8), bgr: (u8, u8, u8)) -> bool {
    let near = |a: u8, b: u8| (a as i32 - b as i32).abs() < 45;
    near(rgb.0, bgr.2) && near(rgb.1, bgr.1) && near(rgb.2, bgr.0)
}

#[test]
fn screen_with_camera_bubble_and_mic_audio() {
    let folder = Folder::new("screen-and-camera");
    screen_clip(&folder.file("Screen.mov"));
    camera_clip(&folder.file("Camera.mov"));
    synth::write_wav(&folder.file("Stereo.wav"), AUDIO_SECONDS);
    let take = take(
        &folder,
        vec![
            audio(folder.file("Stereo.wav"), TakeRole::Stereo),
            captured(folder.file("Screen.mov"), "screen", Some(SCREEN_OFFSET)),
            captured(folder.file("Camera.mov"), "camera", Some(CAMERA_OFFSET)),
        ],
    );
    let mut reports = Vec::new();
    let out = VideoComposer::make(&take, &VideoSettings::default(), |p| reports.push(p)).unwrap();
    assert_eq!(out, folder.path.join(format!("{STEM} PiP.mp4")));
    assert!(reports.iter().all(|p| (0.0..=1.0).contains(p)));

    let info = synth::info(&out);
    // As long as the audio; the screen's size (under 1920 wide); one video track and the mic audio.
    assert!(
        (info.duration - AUDIO_SECONDS).abs() < 0.05,
        "duration {}",
        info.duration
    );
    assert_eq!(info.size, (1280.0, 800.0));
    assert_eq!((info.video_tracks, info.audio_tracks), (1, 1));
    assert!(
        (info.frame_rate - 30.0).abs() < 1.0,
        "frame rate {}",
        info.frame_rate
    );

    // Medium bubble at 800 high: 224 across, 28 from the bottom-right corner, so x 1028…1252, y 548…772
    // from the top.
    for (t, background) in [(0.25, RED), (0.75, YELLOW)] {
        let f = synth::frame(&out, t);
        assert_eq!((f.width, f.height), (1280, 800));
        assert!(
            close(f.at(1140, 660), GREEN),
            "bubble centre at {t}: {:?}",
            f.at(1140, 660)
        );
        // The camera started half a second early, so its first second ends at 0.5 s into the take.
        assert!(
            close(f.at(1140, 580), background),
            "bubble edge at {t}: {:?}",
            f.at(1140, 580)
        );
        // Outside the circle but inside its square: the screen.
        assert!(
            close(f.at(1034, 554), BLUE),
            "bubble corner at {t}: {:?}",
            f.at(1034, 554)
        );
        // The white ring.
        let ring = f.at(1140, 546);
        assert!(
            ring.0 > 200 && ring.1 > 200 && ring.2 > 200,
            "ring at {t}: {ring:?}"
        );
        assert!(
            close(f.at(100, 100), BLUE),
            "screen at {t}: {:?}",
            f.at(100, 100)
        );
    }

    // Remake Video replaces the file.
    let first = std::fs::metadata(&out).unwrap().modified().unwrap();
    let again = VideoComposer::make(&take, &VideoSettings::default(), |_| {}).unwrap();
    assert_eq!(again, out);
    assert!(std::fs::metadata(&out).unwrap().modified().unwrap() >= first);
}

#[test]
fn rounded_bubble_top_left_in_h264() {
    let folder = Folder::new("top-left-rounded");
    screen_clip(&folder.file("Screen.mov"));
    camera_clip(&folder.file("Camera.mov"));
    synth::write_wav(&folder.file("Stereo.wav"), AUDIO_SECONDS);
    let take = take(
        &folder,
        vec![
            audio(folder.file("Stereo.wav"), TakeRole::Stereo),
            captured(folder.file("Screen.mov"), "screen", Some(SCREEN_OFFSET)),
            captured(folder.file("Camera.mov"), "camera", Some(CAMERA_OFFSET)),
        ],
    );
    let settings = VideoSettings {
        codec: Codec::H264,
        bubble_corner: Corner::TopLeft,
        bubble_size: BubbleSize::Small,
        bubble_shape: BubbleShape::Rounded,
        ..VideoSettings::default()
    };
    let out = VideoComposer::make(&take, &settings, |_| {}).unwrap();
    let info = synth::info(&out);
    assert!(
        (info.duration - AUDIO_SECONDS).abs() < 0.05,
        "duration {}",
        info.duration
    );
    assert_eq!(info.size, (1280.0, 800.0));
    // Small bubble: 160 across, 28 from the top-left corner; corners rounded to 28.8.
    let f = synth::frame(&out, 1.0);
    assert!(close(f.at(108, 108), GREEN), "{:?}", f.at(108, 108));
    assert!(
        close(f.at(108, 32), YELLOW),
        "straight top edge: {:?}",
        f.at(108, 32)
    );
    assert!(
        close(f.at(31, 31), BLUE),
        "rounded corner: {:?}",
        f.at(31, 31)
    );
    assert!(
        close(f.at(1140, 660), BLUE),
        "nothing bottom right: {:?}",
        f.at(1140, 660)
    );
}

#[test]
fn camera_alone_with_two_mic_tracks() {
    let folder = Folder::new("camera-and-tracks");
    camera_clip(&folder.file("Camera.mov"));
    synth::write_wav(&folder.file("TX1.wav"), AUDIO_SECONDS);
    synth::write_wav(&folder.file("TX2.wav"), AUDIO_SECONDS);
    let track = |name: &str, mic| TakeFile {
        mic: Some(mic),
        ..audio(folder.file(name), TakeRole::Track)
    };
    let take = take(
        &folder,
        vec![
            track("TX1.wav", 1),
            track("TX2.wav", 2),
            captured(folder.file("Camera.mov"), "camera", Some(CAMERA_OFFSET)),
        ],
    );
    let out = VideoComposer::make(&take, &VideoSettings::default(), |_| {}).unwrap();
    let info = synth::info(&out);
    assert!(
        (info.duration - AUDIO_SECONDS).abs() < 0.05,
        "duration {}",
        info.duration
    );
    // The camera's own size, full frame.
    assert_eq!(info.size, (640.0, 480.0));
    assert_eq!(info.video_tracks, 1);
    assert!(info.audio_tracks >= 1);
    let f = synth::frame(&out, 1.0);
    assert!(close(f.at(320, 240), GREEN), "{:?}", f.at(320, 240));
    assert!(close(f.at(20, 20), YELLOW), "{:?}", f.at(20, 20));
}

#[test]
fn without_audio_the_video_sets_the_length() {
    let folder = Folder::new("camera-only");
    camera_clip(&folder.file("Camera.mov"));
    let take = take(
        &folder,
        vec![captured(
            folder.file("Camera.mov"),
            "camera",
            Some(CAMERA_OFFSET),
        )],
    );
    let out = VideoComposer::make(&take, &VideoSettings::default(), |_| {}).unwrap();
    let info = synth::info(&out);
    // 3.6 s of camera less the half second before the (absent) audio.
    assert!(
        (info.duration - 3.1).abs() < 0.05,
        "duration {}",
        info.duration
    );
    assert_eq!((info.video_tracks, info.audio_tracks), (1, 0));
}

#[test]
fn a_take_without_video_has_no_finished_video() {
    let folder = Folder::new("audio-only");
    synth::write_wav(&folder.file("Stereo.wav"), 1.0);
    let take = take(
        &folder,
        vec![audio(folder.file("Stereo.wav"), TakeRole::Stereo)],
    );
    let error = VideoComposer::make(&take, &VideoSettings::default(), |_| {}).unwrap_err();
    assert_eq!(
        error,
        CaptureError::NothingRecorded("screen or camera".into())
    );
    assert!(!VideoComposer::output_path(&take).exists());
}
