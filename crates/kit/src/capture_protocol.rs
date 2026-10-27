//! Capture plugins (protocol 2).
//!
//! A capture plugin records something alongside Lavdeck's audio, such as the screen or a camera.
//! Lavdeck starts its capture process when a recording starts and talks to it with JSON lines both
//! ways:
//!
//! ```text
//! Lavdeck → plugin (stdin):  {"type":"start", ...}   then later   {"type":"stop"}
//! plugin → Lavdeck (stdout): {"type":"started", ...}, {"type":"output", ...}, {"type":"stopped"}
//! ```
//!
//! Timestamps are host-clock seconds ([`HostClock`]), the same clock AVFoundation and
//! ScreenCaptureKit stamp sample buffers with, so Lavdeck can line every file up with the audio.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::json;
use crate::protocol::{JsonValue, PROTOCOL_VERSION};

/// How Lavdeck starts a plugin's capture process.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CaptureSpec {
    /// Executable for capture, relative to the plugin folder. Defaults to the manifest's `executable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Arguments for the capture process, e.g. `["--capture"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<String>>,
    /// What the plugin can record, e.g. `["screen", "camera"]`, for display in Lavdeck.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
}

/// Seconds on the host clock (mach absolute time), shared by Lavdeck, AVFoundation and ScreenCaptureKit.
pub struct HostClock;

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut [u32; 2]) -> i32;
}

impl HostClock {
    pub fn seconds_from_ticks(ticks: u64) -> f64 {
        static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
        let (numer, denom) = *TIMEBASE.get_or_init(|| {
            let mut info = [0u32; 2];
            // SAFETY: mach_timebase_info fills a {numer, denom} pair of u32s.
            unsafe { mach_timebase_info(&mut info) };
            (info[0], info[1].max(1))
        });
        ticks as f64 * numer as f64 / denom as f64 / 1_000_000_000.0
    }

    pub fn now() -> f64 {
        // SAFETY: no preconditions.
        Self::seconds_from_ticks(unsafe { mach_absolute_time() })
    }
}

/// The take a capture belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureTake {
    /// Folder the take's files go in.
    pub folder: String,
    /// The take's stem, e.g. `14.02.11`. Name every file `<stem> <Something>.<ext>`.
    pub stem: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureCommandKind {
    Start,
    Stop,
}

/// A line Lavdeck writes to a capture process.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureCommand {
    #[serde(rename = "type")]
    pub kind: CaptureCommandKind,
    #[serde(
        default,
        deserialize_with = "json::optional_int::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub protocol: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take: Option<CaptureTake>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<BTreeMap<String, JsonValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_directory: Option<String>,
    /// Host-clock seconds when Lavdeck asked the recording to start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_time: Option<f64>,
    /// Permissions from the plugin's manifest that Lavdeck holds right now, e.g. `["camera"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted: Option<Vec<String>>,
}

impl CaptureCommand {
    /// `{"type":"stop","protocol":2}`.
    pub fn stop() -> Self {
        Self::new(CaptureCommandKind::Stop)
    }

    pub fn new(kind: CaptureCommandKind) -> Self {
        Self {
            kind,
            protocol: Some(PROTOCOL_VERSION),
            take: None,
            settings: None,
            plugin_directory: None,
            host_time: None,
            granted: None,
        }
    }

    /// One JSON line without the newline.
    pub fn to_json(&self) -> String {
        json::to_compact(&json::tree(self))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureEventKind {
    Started,
    Output,
    Log,
    Error,
    Stopped,
}

/// A line a capture process writes to Lavdeck.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureEvent {
    #[serde(rename = "type")]
    pub kind: CaptureEventKind,
    /// What this is about: `screen`, `camera`, `system-audio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// File written, for `output`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `video` or `audio`, for `output`. (`kind` in the JSON.)
    #[serde(rename = "kind", default, skip_serializing_if = "Option::is_none")]
    pub media_kind: Option<String>,
    /// Host-clock seconds of the file's first frame (for `started` and `output`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_host_time: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl CaptureEvent {
    pub fn new(kind: CaptureEventKind) -> Self {
        Self {
            kind,
            source: None,
            path: None,
            media_kind: None,
            start_host_time: None,
            message: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_encode_like_swift() {
        assert_eq!(
            CaptureCommand::stop().to_json(),
            r#"{"type":"stop","protocol":2}"#
        );
        let mut start = CaptureCommand::new(CaptureCommandKind::Start);
        start.take = Some(CaptureTake {
            folder: "/tmp".into(),
            stem: "14.02.11".into(),
        });
        start.settings = Some(BTreeMap::from([("camera".into(), JsonValue::Bool(true))]));
        start.plugin_directory = Some("/p".into());
        start.host_time = Some(12345.678);
        start.granted = Some(vec!["camera".into()]);
        assert_eq!(
            start.to_json(),
            r#"{"type":"start","protocol":2,"take":{"folder":"\/tmp","stem":"14.02.11"},"settings":{"camera":true},"pluginDirectory":"\/p","hostTime":12345.678,"granted":["camera"]}"#
        );
    }

    #[test]
    fn events_decode() {
        let event: CaptureEvent =
            serde_json::from_str(r#"{"type":"output","source":"screen","kind":"video","path":"/x","startHostTime":100.5}"#)
                .unwrap();
        assert_eq!(event.kind, CaptureEventKind::Output);
        assert_eq!(event.media_kind.as_deref(), Some("video"));
        assert_eq!(event.start_host_time, Some(100.5));
        assert!(serde_json::from_str::<CaptureEvent>(r#"{"type":"bogus"}"#).is_err());
    }

    #[test]
    fn host_clock_moves_forward() {
        let a = HostClock::now();
        let b = HostClock::now();
        assert!(a > 0.0 && b >= a);
    }
}
