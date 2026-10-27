//! The standard transcript format. Transcription plugins write it as `<stem>.transcript.json` next
//! to the take; the recordings manager reads it for viewing, search and playback.

use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::json;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptDocument {
    #[serde(deserialize_with = "json::int::deserialize")]
    pub version: i64,
    /// Take id, e.g. `2026-10-11/12.34.54`.
    pub take: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(with = "json::date")]
    pub created: SystemTime,
    pub speakers: Vec<TranscriptSpeaker>,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSpeaker {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<TranscriptWord>>,
}

impl TranscriptSegment {
    /// Stable within a transcript: `<speaker>@<start>`, e.g. `tx1@1.16`.
    pub fn id(&self) -> String {
        format!("{}@{}", self.speaker, json::swift_double(self.start))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Why a transcript couldn't be read.
#[derive(Debug)]
pub enum TranscriptError {
    Read(std::io::Error),
    Decode(serde_json::Error),
}

impl std::fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(e) => write!(f, "{e}"),
            Self::Decode(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TranscriptError {}

impl TranscriptDocument {
    /// The speaker's display name, or the id when the transcript doesn't name it.
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

    pub fn load(path: &Path) -> Result<Self, TranscriptError> {
        let data = std::fs::read(path).map_err(TranscriptError::Read)?;
        serde_json::from_slice(&data).map_err(TranscriptError::Decode)
    }

    /// `HH:MM:SS`, rounding down.
    pub fn timestamp(seconds: f64) -> String {
        let s = seconds.floor() as i64;
        format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_documented_shape() {
        let doc: TranscriptDocument = serde_json::from_str(
            r#"{"version":1,"take":"2026-10-11/13.00.00","model":"m","language":"en","created":"2026-10-11T17:01:02Z",
                "speakers":[{"id":"tx1","name":"Host"},{"id":"tx2","name":"Guest"}],
                "segments":[{"start":0.0,"end":2.86,"speaker":"tx1","text":"All right,",
                             "words":[{"start":0.0,"end":0.32,"text":"All"}]},
                            {"start":3,"end":4,"speaker":"speaker","text":"this is it."}]}"#,
        )
        .unwrap();
        assert_eq!(doc.speaker_name("tx2"), "Guest");
        assert_eq!(doc.speaker_name("speaker"), "speaker");
        assert_eq!(doc.plain_text(), "All right, this is it.");
        assert_eq!(doc.segments[0].id(), "tx1@0.0");
        assert_eq!(TranscriptDocument::timestamp(3725.9), "01:02:05");
    }
}
