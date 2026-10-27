//! The app's settings, with the Swift app's defaults and keys (`io.github.piedol.lavdeck`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingFormat {
    Wav,
    M4a,
}

impl RecordingFormat {
    /// The file extension and the `format` defaults value.
    pub fn key(self) -> &'static str {
        match self {
            RecordingFormat::Wav => "wav",
            RecordingFormat::M4a => "m4a",
        }
    }

    pub fn from_key(key: &str) -> Option<RecordingFormat> {
        match key {
            "wav" => Some(RecordingFormat::Wav),
            "m4a" => Some(RecordingFormat::M4a),
            _ => None,
        }
    }
}

/// How Lavdeck records video with the mics and makes each take's finished video. Stored as
/// JSON under `video.settings`, with the Swift field names. Mirrors the capture crate's type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoSettings {
    pub screen: bool,
    pub camera: bool,
    pub camera_preview: bool,
    /// `bubble`, `large` or `full`.
    pub preview_size: String,
    pub system_audio: bool,
    #[serde(rename = "screenFPS")]
    pub screen_fps: i64,
    /// `hevc` or `h264`.
    pub codec: String,
    pub make_video: bool,
    /// `bottom-right`, `bottom-left`, `top-right`, `top-left`.
    pub bubble_corner: String,
    /// `small`, `medium`, `large`.
    pub bubble_size: String,
    /// `circle` or `rounded`.
    pub bubble_shape: String,
}

impl Default for VideoSettings {
    fn default() -> Self {
        VideoSettings {
            screen: false,
            camera: false,
            camera_preview: true,
            preview_size: "bubble".into(),
            system_audio: false,
            screen_fps: 30,
            codec: "hevc".into(),
            make_video: true,
            bubble_corner: "bottom-right".into(),
            bubble_size: "medium".into(),
            bubble_shape: "circle".into(),
        }
    }
}

impl VideoSettings {
    pub fn records_video(&self) -> bool {
        self.screen || self.camera
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// `recordingsRoot`: where dated take folders go.
    pub recordings_root: PathBuf,
    /// `format`.
    pub format: RecordingFormat,
    /// `splitTracks`: TX1 and TX2 files next to a stereo WAV take.
    pub split_tracks: bool,
    /// `tapToRecord`: the transmitters' link button starts and stops a take.
    pub tap_to_record: bool,
    /// `armed`: transmitters that record. One armed mic records alone; none disables Record.
    pub armed: BTreeSet<u8>,
    /// `video.settings`.
    pub video: VideoSettings,
    /// `micNames`: names keyed by Bluetooth ID, so a name follows the physical mic.
    pub mic_names: BTreeMap<String, String>,
    /// `checkUpdates`: ask GitHub once a day whether there's a newer version.
    pub check_updates: bool,
}

impl Settings {
    /// The Swift app's defaults, for a first launch.
    pub fn defaults(home: &std::path::Path) -> Settings {
        Settings {
            recordings_root: home.join("Recordings/Lavdeck"),
            format: RecordingFormat::Wav,
            split_tracks: true,
            tap_to_record: true,
            armed: [1, 2].into(),
            video: VideoSettings::default(),
            mic_names: BTreeMap::new(),
            check_updates: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_settings_round_trip_with_swift_keys() {
        let json = r#"{"screen":true,"camera":false,"cameraPreview":true,"previewSize":"large",
            "systemAudio":true,"screenFPS":60,"codec":"h264","makeVideo":false,
            "bubbleCorner":"top-left","bubbleSize":"small","bubbleShape":"rounded"}"#;
        let v: VideoSettings = serde_json::from_str(json).unwrap();
        assert!(v.screen && v.system_audio && !v.make_video);
        assert_eq!(
            (v.screen_fps, v.codec.as_str(), v.bubble_corner.as_str()),
            (60, "h264", "top-left")
        );
        let back = serde_json::to_string(&v).unwrap();
        assert!(back.contains("\"screenFPS\":60") && back.contains("\"cameraPreview\":true"));
    }

    #[test]
    fn missing_video_keys_take_defaults() {
        let v: VideoSettings = serde_json::from_str(r#"{"camera":true}"#).unwrap();
        assert!(v.camera && !v.screen && v.make_video);
        assert_eq!(v.screen_fps, 30);
    }
}
