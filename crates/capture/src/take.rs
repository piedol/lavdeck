//! The parts of the kit's take and capture types this crate reads and returns: a take's files with their
//! media offsets (`TakeDescriptor`, `TakeFile`) and the files a recording wrote (`CaptureOutput`). Field
//! for field a subset of `LavdeckPluginKit`/`LavdeckKit`, to be replaced by the kit crate's types.

use std::path::PathBuf;

/// What a file in a take is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TakeRole {
    /// Stereo take (TX1 left, TX2 right).
    Stereo,
    /// Mono take where the receiver mixed both mics.
    Mix,
    /// One mic's own mono track.
    Track,
    /// Produced by a plugin (transcript, converted voice, the finished video).
    Derived,
    /// Lavdeck's own metadata sidecar.
    Metadata,
    /// Recorded alongside the audio (screen, camera, system audio). See `source` and `offset`.
    Capture,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TakeFile {
    pub path: PathBuf,
    pub role: TakeRole,
    /// Transmitter index (1 or 2) for `Track` files.
    pub mic: Option<u8>,
    /// Free-form kind for derived and captured files, e.g. `transcript`, `audio`, `video`.
    pub kind: Option<String>,
    /// What a captured or composed video shows: `screen`, `camera`, `system-audio`, `pip`.
    pub source: Option<String>,
    /// Seconds after the take's first audio sample that this file's first frame happened (negative when
    /// it started earlier). Set for captured media from `.take.json`.
    pub offset: Option<f64>,
}

impl TakeFile {
    pub fn new(path: impl Into<PathBuf>, role: TakeRole) -> Self {
        TakeFile {
            path: path.into(),
            role,
            mic: None,
            kind: None,
            source: None,
            offset: None,
        }
    }

    /// A captured video, e.g. `<stem> Screen.mov` with source `screen`.
    pub fn captured(path: impl Into<PathBuf>, source: &str, offset: Option<f64>) -> Self {
        TakeFile {
            kind: Some("video".into()),
            source: Some(source.into()),
            offset,
            ..Self::new(path, TakeRole::Capture)
        }
    }
}

/// One recording: every file that shares a start-time stem.
#[derive(Clone, Debug, PartialEq)]
pub struct TakeDescriptor {
    /// Folder the take's files live in.
    pub folder: PathBuf,
    /// Start-time stem shared by the take's files, e.g. `12.34.54`.
    pub stem: String,
    pub files: Vec<TakeFile>,
}

impl TakeDescriptor {
    /// The best single file holding everyone: stereo, then mix, then the first track.
    pub fn main_audio(&self) -> Option<&TakeFile> {
        let first = |role| self.files.iter().find(|f| f.role == role);
        first(TakeRole::Stereo)
            .or_else(|| first(TakeRole::Mix))
            .or_else(|| first(TakeRole::Track))
    }

    /// The first captured file from `source` (`screen`, `camera`).
    pub fn capture(&self, source: &str) -> Option<&TakeFile> {
        self.files
            .iter()
            .find(|f| f.role == TakeRole::Capture && f.source.as_deref() == Some(source))
    }

    /// Whether the take has a captured video, so it can have a finished video (`has("video")`).
    pub fn has_video(&self) -> bool {
        self.files
            .iter()
            .any(|f| f.role == TakeRole::Capture && f.kind.as_deref() == Some("video"))
    }
}

/// One file recorded for a take, as the capture host reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureOutput {
    /// `lavdeck` for Lavdeck's own recording.
    pub plugin_id: String,
    pub path: PathBuf,
    pub source: String,
    pub kind: String,
    /// Host-clock seconds of the file's first frame.
    pub start_host_time: Option<f64>,
}

impl CaptureOutput {
    /// The file's offset in `.take.json` (`media[].offset`): seconds after the audio's first sample that
    /// the file's first frame happened. Unknown when either start is.
    pub fn offset(&self, audio_start_host_time: Option<f64>) -> Option<f64> {
        media_offset(self.start_host_time, audio_start_host_time)
    }
}

/// `zip(output.startHostTime, audioStart).map { $0 - $1 }`.
pub fn media_offset(
    start_host_time: Option<f64>,
    audio_start_host_time: Option<f64>,
) -> Option<f64> {
    Some(start_host_time? - audio_start_host_time?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_relative_to_the_first_audio_sample() {
        let out = |start| CaptureOutput {
            plugin_id: "lavdeck".into(),
            path: "/d/09.00.00 Camera.mov".into(),
            source: "camera".into(),
            kind: "video".into(),
            start_host_time: start,
        };
        // Video starts before the audio (it's started first), so offsets are usually negative.
        assert_eq!(out(Some(1000.25)).offset(Some(1000.75)), Some(-0.5));
        assert_eq!(out(Some(1001.0)).offset(Some(1000.75)), Some(0.25));
        assert_eq!(out(None).offset(Some(1.0)), None);
        assert_eq!(out(Some(1.0)).offset(None), None);
    }

    #[test]
    fn main_audio_prefers_stereo_then_mix_then_a_track() {
        let mut take = TakeDescriptor {
            folder: "/d".into(),
            stem: "09.00.00".into(),
            files: vec![
                TakeFile {
                    mic: Some(1),
                    ..TakeFile::new("/d/09.00.00 TX1.wav", TakeRole::Track)
                },
                TakeFile::new("/d/09.00.00 Mix.wav", TakeRole::Mix),
            ],
        };
        assert_eq!(take.main_audio().unwrap().role, TakeRole::Mix);
        take.files
            .push(TakeFile::new("/d/09.00.00 Stereo.wav", TakeRole::Stereo));
        assert_eq!(take.main_audio().unwrap().role, TakeRole::Stereo);
        assert!(!take.has_video());
        take.files.push(TakeFile::captured(
            "/d/09.00.00 Camera.mov",
            "camera",
            Some(-0.4),
        ));
        assert!(take.has_video());
        assert!(take.capture("camera").is_some() && take.capture("screen").is_none());
    }
}
