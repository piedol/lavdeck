//! Takes on disk and their transcripts, as Recordings shows them. These mirror the plugin
//! SDK's `TakeDescriptor` and `TranscriptDocument`; the kit crate's take library produces
//! them once it's wired in (phase 2).

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// What a file in a take is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Stereo take (TX1 left, TX2 right).
    Stereo,
    /// Mono take where the receiver mixed both mics.
    Mix,
    /// One mic's own mono track.
    Track,
    /// Produced by a plugin or Lavdeck (transcript, finished video).
    Derived,
    /// Lavdeck's `.take.json`.
    Metadata,
    /// Recorded alongside the audio (screen, camera, system audio).
    Capture,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TakeFile {
    pub path: PathBuf,
    pub role: Role,
    pub mic: Option<u8>,
    /// `transcript`, `audio`, `video`…
    pub kind: Option<String>,
    /// `screen`, `camera`, `system-audio`, `pip`.
    pub source: Option<String>,
    /// Seconds after the take's first audio sample that this file starts.
    pub offset: Option<f64>,
}

impl TakeFile {
    pub fn new(path: impl Into<PathBuf>, role: Role) -> TakeFile {
        TakeFile {
            path: path.into(),
            role,
            mic: None,
            kind: None,
            source: None,
            offset: None,
        }
    }

    pub fn name(&self) -> String {
        file_name(&self.path)
    }

    fn is_capture_video(&self, source: Option<&str>) -> bool {
        self.role == Role::Capture
            && self.kind.as_deref() == Some("video")
            && source.is_none_or(|s| self.source.as_deref() == Some(s))
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
pub struct TakeSpeaker {
    /// `tx1` or `tx2`.
    pub id: String,
    pub mic: u8,
    /// The user's name for that mic, or `TX1` / `TX2`.
    pub name: String,
    /// That mic's own mono track, when the take has one.
    pub file: Option<PathBuf>,
}

/// One recording: every file sharing a start-time stem.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    /// `<day folder>/<stem>`, e.g. `2026-10-26/11.05.09`.
    pub id: String,
    pub folder: PathBuf,
    pub stem: String,
    /// Seconds since 1970.
    pub started: Option<f64>,
    pub duration: Option<f64>,
    pub channel_mode: Option<String>,
    pub files: Vec<TakeFile>,
    pub speakers: Vec<TakeSpeaker>,
}

impl Take {
    /// True when each speaker has their own mono track.
    pub fn is_two_speaker(&self) -> bool {
        self.speakers.iter().filter(|s| s.file.is_some()).count() >= 2
    }

    /// Whether the take has a kind of content, for a plugin action's `requires`.
    pub fn has(&self, requirement: &str) -> bool {
        match requirement {
            "video" => self.files.iter().any(|f| f.is_capture_video(None)),
            "screen" | "camera" => self
                .files
                .iter()
                .any(|f| f.role == Role::Capture && f.source.as_deref() == Some(requirement)),
            "transcript" => self.files.iter().any(|f| {
                f.kind.as_deref() == Some("transcript")
                    && f.path.to_string_lossy().ends_with(".transcript.json")
            }),
            _ => true,
        }
    }

    pub fn has_transcript(&self) -> bool {
        self.has("transcript")
    }

    pub fn transcript_path(&self) -> PathBuf {
        self.folder.join(format!("{}.transcript.json", self.stem))
    }

    /// The best single file holding everyone: stereo, then mix, then the first track.
    pub fn main_audio(&self) -> Option<&TakeFile> {
        let first = |role| self.files.iter().find(|f| f.role == role);
        first(Role::Stereo)
            .or_else(|| first(Role::Mix))
            .or_else(|| first(Role::Track))
    }

    pub fn finished_video(&self) -> Option<&TakeFile> {
        self.files
            .iter()
            .find(|f| f.source.as_deref() == Some("pip"))
    }

    pub fn all_paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|f| f.path.clone()).collect()
    }

    /// The file to share, open or reveal: the finished video, else the main audio.
    pub fn share_path(&self) -> Option<PathBuf> {
        self.finished_video()
            .or_else(|| self.main_audio())
            .map(|f| f.path.clone())
    }

    pub fn speaker_names(&self) -> String {
        self.speakers
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// For the Video column and its sort: 3 finished video, 2 screen and camera, 1 one raw
    /// capture, 0 audio only.
    pub fn media_rank(&self) -> u8 {
        if self.finished_video().is_some() {
            3
        } else if self
            .files
            .iter()
            .filter(|f| f.is_capture_video(None))
            .count()
            >= 2
        {
            2
        } else if self.has("video") {
            1
        } else {
            0
        }
    }

    pub fn has_capture(&self, source: &str) -> bool {
        self.files
            .iter()
            .any(|f| f.role == Role::Capture && f.source.as_deref() == Some(source))
    }
}

/// Something in a take that plays: the finished video, a raw capture, the audio, a mic's track.
#[derive(Clone, Debug, PartialEq)]
pub struct TakeMedia {
    pub kind: MediaKind,
    pub file: TakeFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Screen,
    Camera,
    Audio,
    Track(u8),
}

impl TakeMedia {
    pub fn is_video(&self) -> bool {
        self.file.kind.as_deref() == Some("video")
    }

    /// What a take offers, best first: the finished video, raw captures, the main audio,
    /// then each mic's own track in a two-speaker take.
    pub fn all(take: &Take) -> Vec<TakeMedia> {
        let mut out = Vec::new();
        let mut push = |kind, file: &TakeFile| {
            out.push(TakeMedia {
                kind,
                file: file.clone(),
            })
        };
        if let Some(pip) = take.finished_video() {
            push(MediaKind::Video, pip);
        }
        if let Some(f) = take
            .files
            .iter()
            .find(|f| f.is_capture_video(Some("screen")))
        {
            push(MediaKind::Screen, f);
        }
        if let Some(f) = take
            .files
            .iter()
            .find(|f| f.is_capture_video(Some("camera")))
        {
            push(MediaKind::Camera, f);
        }
        let main = take.main_audio();
        if let Some(main) = main {
            push(MediaKind::Audio, main);
        }
        if take.is_two_speaker() {
            for speaker in &take.speakers {
                let Some(path) = &speaker.file else { continue };
                if let Some(file) = take.files.iter().find(|f| &f.path == path)
                    && main.is_none_or(|m| m.path != file.path)
                {
                    push(MediaKind::Track(speaker.mic), file);
                }
            }
        }
        out
    }
}

/// The standard transcript format plugins write as `<stem>.transcript.json`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TranscriptDocument {
    #[serde(default)]
    pub version: i64,
    #[serde(default)]
    pub take: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub speakers: Vec<Speaker>,
    #[serde(default)]
    pub segments: Vec<Segment>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Speaker {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
    pub text: String,
}

impl Segment {
    /// Stable within a transcript: speaker and start time, as in the Swift app.
    pub fn id(&self) -> String {
        format!("{}@{}", self.speaker, self.start)
    }
}

impl TranscriptDocument {
    pub fn parse(json: &str) -> Result<TranscriptDocument, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    pub fn load(path: &Path) -> Result<TranscriptDocument, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        TranscriptDocument::parse(&text)
    }

    pub fn speaker_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.speakers
            .iter()
            .find(|s| s.id == id)
            .map_or(id, |s| s.name.as_str())
    }

    pub fn plain_text(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The line under the playhead: the last segment that started at or before `time`
    /// and hasn't ended.
    pub fn segment_at(&self, time: f64) -> Option<&Segment> {
        self.segments
            .iter()
            .rev()
            .find(|s| s.start <= time && time < s.end)
    }
}
