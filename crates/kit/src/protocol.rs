//! The Lavdeck plugin protocol (see `docs/PLUGINS.md`).
//!
//! A plugin is a folder containing `plugin.json` (a [`PluginManifest`]) and an executable. Lavdeck
//! starts the executable for each job, writes one [`PluginRequest`] as JSON to its standard input,
//! and reads newline-delimited [`PluginMessage`] JSON objects from its standard output until it
//! exits. Protocol 2 adds capture plugins ([`crate::capture_protocol`]); protocol 1 plugins keep
//! working unchanged.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::capture_protocol::CaptureSpec;
use crate::json;

/// Newest protocol this kit speaks.
pub const PROTOCOL_VERSION: i64 = 2;
/// Protocol versions Lavdeck accepts in a manifest.
pub const SUPPORTED_PROTOCOLS: std::ops::RangeInclusive<i64> = 1..=2;
/// Event sent when a recording has finished and its files (including split tracks) are written.
pub const TAKE_FINISHED: &str = "take.finished";

// MARK: - Manifest

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    /// Reverse-DNS identifier, e.g. `io.github.example.lavdeck.transcribe`.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Protocol version the plugin speaks.
    #[serde(deserialize_with = "json::int::deserialize")]
    pub protocol: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Path of the executable, relative to the plugin folder.
    pub executable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<String>>,
    /// Events the plugin runs on automatically while enabled, e.g. `take.finished`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<String>>,
    /// Actions a user can run on a take from the recordings manager.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<PluginAction>>,
    /// Settings Lavdeck renders in Settings › Plugins and passes back with every request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Vec<PluginSetting>>,
    /// Output kinds the plugin can produce, e.g. `transcript`, `audio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produces: Option<Vec<String>>,
    /// `post` (default), `capture` or `app`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// System permissions the plugin needs: `screen-recording`, `camera`, `microphone`, `network`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<String>>,
    /// Oldest Lavdeck version the plugin works with, e.g. `0.2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_app_version: Option<String>,
    /// How to start the plugin's capture process (protocol 2, `kind: capture`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<CaptureSpec>,
    /// The companion app to open (protocol 2, `kind: app`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppSpec>,
}

impl PluginManifest {
    /// A manifest with only the required fields set.
    pub fn new(id: &str, name: &str, version: &str, protocol: i64, executable: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            protocol,
            summary: None,
            executable: executable.into(),
            arguments: None,
            events: None,
            actions: None,
            settings: None,
            produces: None,
            kind: None,
            permissions: None,
            min_app_version: None,
            capture: None,
            app: None,
        }
    }

    pub fn is_capture(&self) -> bool {
        self.kind.as_deref() == Some("capture") && self.capture.is_some()
    }

    pub fn is_app(&self) -> bool {
        self.kind.as_deref() == Some("app") && self.app.is_some()
    }

    /// Reads `<folder>/plugin.json`.
    pub fn load(folder: &Path) -> Result<Self, ManifestError> {
        let data = std::fs::read(folder.join("plugin.json")).map_err(ManifestError::Read)?;
        serde_json::from_slice(&data).map_err(|e| ManifestError::Decode(e.to_string()))
    }
}

/// Why `plugin.json` couldn't be loaded. Displays as Foundation words the same failures.
#[derive(Debug)]
pub enum ManifestError {
    Read(std::io::Error),
    Decode(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(e) if e.kind() == std::io::ErrorKind::NotFound => {
                f.write_str("The file “plugin.json” couldn’t be opened because there is no such file.")
            }
            Self::Read(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                f.write_str("The file “plugin.json” couldn’t be opened because you don’t have permission to view it.")
            }
            Self::Read(_) => f.write_str("The file “plugin.json” couldn’t be opened."),
            // Swift reports a missing key or a null required value as "missing", anything else as format.
            Self::Decode(m) if m.starts_with("missing field") || m.starts_with("invalid type: null") => {
                f.write_str("The data couldn’t be read because it is missing.")
            }
            Self::Decode(_) => f.write_str("The data couldn’t be read because it isn’t in the correct format."),
        }
    }
}

impl std::error::Error for ManifestError {}

/// A companion app a plugin ships (protocol 2, `kind: app`), such as an editor. Lavdeck opens it on a take.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSpec {
    /// The `.app` bundle, relative to the plugin folder.
    pub bundle: String,
    /// URL to open a take with; `{folder}`, `{stem}` and `{id}` are filled in (percent-encoded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_take: Option<String>,
    /// Button title on a take, e.g. `Open in Studio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_title: Option<String>,
}

impl AppSpec {
    /// The URL that opens a take in the app, as a string for `NSURL`.
    pub fn url(&self, take: &TakeDescriptor) -> Option<String> {
        let template = self.open_take.as_ref()?;
        Some(
            template
                .replace("{folder}", &query_encode(&take.folder))
                .replace("{stem}", &query_encode(&take.stem))
                .replace("{id}", &query_encode(&take.id)),
        )
    }
}

/// Percent-encodes like `urlQueryAllowed` minus `&=+?#`, so a value can't break the query apart.
fn query_encode(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        let keep = b.is_ascii_alphanumeric() || b"-._~!$'()*,;:@/".contains(&b);
        if keep {
            out.push(b as char)
        } else {
            out.push_str(&format!("%{b:02X}"))
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginAction {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What a take needs for this action to make sense: `video`, `screen`, `camera`, `transcript`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Vec<String>>,
}

impl PluginAction {
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            summary: None,
            requires: None,
        }
    }

    /// True when `take` has everything in `requires`.
    pub fn applies(&self, take: &TakeDescriptor) -> bool {
        self.requires.iter().flatten().all(|r| take.has(r))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginSettingKind {
    Bool,
    String,
    Number,
    Choice,
    Path,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginSettingOption {
    pub value: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginSetting {
    pub key: String,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: PluginSettingKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<PluginSettingOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// A JSON value for plugin settings: no arrays or objects.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
}

impl JsonValue {
    pub fn as_bool(&self) -> Option<bool> {
        if let Self::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Self::String(s) = self {
            Some(s)
        } else {
            None
        }
    }
}

/// How a setting reads in `LAVDECK_SETTING_*`: whole numbers without a decimal point, null empty.
impl fmt::Display for JsonValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => f.write_str(s),
            Self::Number(n) if *n == n.round() && n.abs() < 9.2e18 => write!(f, "{}", *n as i64),
            Self::Number(n) => f.write_str(&json::swift_double(*n)),
            Self::Bool(b) => f.write_str(if *b { "true" } else { "false" }),
            Self::Null => Ok(()),
        }
    }
}

impl Serialize for JsonValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::String(v) => s.serialize_str(v),
            Self::Number(n) => s.serialize_f64(*n),
            Self::Bool(b) => s.serialize_bool(*b),
            Self::Null => s.serialize_unit(),
        }
    }
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Null => Ok(Self::Null),
            serde_json::Value::Bool(b) => Ok(Self::Bool(b)),
            serde_json::Value::Number(n) => Ok(Self::Number(n.as_f64().unwrap_or(0.0))),
            serde_json::Value::String(s) => Ok(Self::String(s)),
            _ => Err(serde::de::Error::custom(
                "expected a string, number, bool or null",
            )),
        }
    }
}

// MARK: - Takes

/// One recording ("take"): every file that shares a start-time stem, plus what Lavdeck knows about it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeDescriptor {
    /// Stable id: `<day folder>/<stem>`, e.g. `2026-10-11/12.34.54`.
    pub id: String,
    /// Folder the take's files live in.
    pub folder: String,
    /// Start-time stem shared by the take's files, e.g. `12.34.54`.
    pub stem: String,
    #[serde(
        default,
        with = "json::optional_date",
        skip_serializing_if = "Option::is_none"
    )]
    pub started: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// `mono` or `stereo` at the time of recording, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_mode: Option<String>,
    pub files: Vec<TakeFile>,
    /// Who spoke into which mic. Two entries with mono files means two-speaker mode.
    pub speakers: Vec<TakeSpeaker>,
}

impl TakeDescriptor {
    pub fn new(id: &str, folder: &str, stem: &str) -> Self {
        Self {
            id: id.into(),
            folder: folder.into(),
            stem: stem.into(),
            started: None,
            duration: None,
            channel_mode: None,
            files: Vec::new(),
            speakers: Vec::new(),
        }
    }

    /// True when each speaker has their own mono track (stereo with split tracks, both mics armed).
    pub fn is_two_speaker(&self) -> bool {
        self.speakers.iter().filter(|s| s.file.is_some()).count() >= 2
    }

    /// Whether the take has a kind of content, for [`PluginAction::requires`].
    pub fn has(&self, requirement: &str) -> bool {
        match requirement {
            "video" => self
                .files
                .iter()
                .any(|f| f.role == TakeRole::Capture && f.kind.as_deref() == Some("video")),
            "screen" | "camera" => self
                .files
                .iter()
                .any(|f| f.role == TakeRole::Capture && f.source.as_deref() == Some(requirement)),
            "transcript" => self.files.iter().any(|f| {
                f.kind.as_deref() == Some("transcript") && f.path.ends_with(".transcript.json")
            }),
            _ => true,
        }
    }

    /// The best single file holding everyone: stereo, then mix, then the first track.
    pub fn main_audio(&self) -> Option<&TakeFile> {
        [TakeRole::Stereo, TakeRole::Mix, TakeRole::Track]
            .into_iter()
            .find_map(|role| self.files.iter().find(|f| f.role == role))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TakeRole {
    /// Stereo take (TX1 left, TX2 right).
    Stereo,
    /// Mono take where the receiver mixed both mics.
    Mix,
    /// One mic's own mono track.
    Track,
    /// Produced by a plugin (transcript, converted voice, …).
    Derived,
    /// Lavdeck's own metadata sidecar.
    Metadata,
    /// Recorded by a capture plugin alongside the audio (screen, camera, system audio).
    Capture,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakeFile {
    pub path: String,
    pub role: TakeRole,
    /// Transmitter index (1 or 2) for `track` files.
    #[serde(
        default,
        deserialize_with = "json::optional_int::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub mic: Option<i64>,
    /// Free-form kind for derived and captured files, e.g. `transcript`, `audio`, `video`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// What a captured or composed video shows: `screen`, `camera`, `system-audio`, `pip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Seconds after the take's first audio sample that this file's first frame happened (negative
    /// when it started earlier). Set for captured media.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
}

impl TakeFile {
    pub fn new(path: &str, role: TakeRole) -> Self {
        Self {
            path: path.into(),
            role,
            mic: None,
            kind: None,
            source: None,
            offset: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakeSpeaker {
    /// `tx1` or `tx2`.
    pub id: String,
    #[serde(deserialize_with = "json::int::deserialize")]
    pub mic: i64,
    /// The user's name for that mic, or `TX1` / `TX2`.
    pub name: String,
    /// That mic's own mono track, when the take has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

// MARK: - Requests and messages

/// What Lavdeck sends a plugin on standard input (one JSON object, then EOF).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRequest {
    #[serde(deserialize_with = "json::int::deserialize")]
    pub protocol: i64,
    /// `event` or `action`.
    pub kind: String,
    /// Event name (e.g. `take.finished`) or action id.
    pub name: String,
    pub take: TakeDescriptor,
    pub settings: BTreeMap<String, JsonValue>,
    /// Where outputs should be written (the take's folder).
    pub output_directory: String,
    pub plugin_directory: String,
}

impl PluginRequest {
    pub fn new(
        kind: &str,
        name: &str,
        take: TakeDescriptor,
        settings: BTreeMap<String, JsonValue>,
        output_directory: &str,
        plugin_directory: &str,
    ) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            kind: kind.into(),
            name: name.into(),
            take,
            settings,
            output_directory: output_directory.into(),
            plugin_directory: plugin_directory.into(),
        }
    }

    /// The request as written to the plugin: compact JSON, ISO 8601 dates.
    pub fn to_json(&self) -> String {
        json::to_compact(&json::tree(self))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginMessageKind {
    Progress,
    Output,
    Log,
    Done,
    Error,
}

/// One line a plugin writes to standard output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginMessage {
    #[serde(rename = "type")]
    pub kind: PluginMessageKind,
    /// 0...1 for `progress`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// File written, for `output`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Output kind, e.g. `transcript`, for `output`. (`kind` in the JSON.)
    #[serde(rename = "kind", default, skip_serializing_if = "Option::is_none")]
    pub output_kind: Option<String>,
}

impl PluginMessage {
    pub fn new(kind: PluginMessageKind) -> Self {
        Self {
            kind,
            fraction: None,
            message: None,
            path: None,
            output_kind: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_plugins_fill_the_open_take_url() {
        let spec = AppSpec {
            bundle: "x.app".into(),
            open_take: Some("studio://open?folder={folder}&stem={stem}".into()),
            action_title: None,
        };
        let take = TakeDescriptor::new(
            "2026-10-11/20.11.43",
            "/Users/me/Recordings/Lavdeck/2026-10-11",
            "20.11.43",
        );
        assert_eq!(
            spec.url(&take).as_deref(),
            Some("studio://open?folder=/Users/me/Recordings/Lavdeck/2026-10-11&stem=20.11.43")
        );
        let spaced = TakeDescriptor::new("a/b", "/Users/me/My Takes & Co", "b");
        assert_eq!(
            spec.url(&spaced).as_deref(),
            Some("studio://open?folder=/Users/me/My%20Takes%20%26%20Co&stem=b")
        );
        let unicode = TakeDescriptor::new("a/b", "/Users/me/Café+1", "b");
        assert_eq!(
            spec.url(&unicode).as_deref(),
            Some("studio://open?folder=/Users/me/Caf%C3%A9%2B1&stem=b")
        );
    }

    #[test]
    fn actions_apply_only_to_takes_with_what_they_require() {
        let mut remake = PluginAction::new("pip", "Remake video");
        remake.requires = Some(vec!["video".into()]);
        let anything = PluginAction::new("info", "Take info");
        let mut audio = TakeDescriptor::new("d/1", "/d", "1");
        audio.files.push(TakeFile {
            mic: Some(1),
            ..TakeFile::new("/d/1 TX1.wav", TakeRole::Track)
        });
        let mut camera = audio.clone();
        camera.files.push(TakeFile {
            kind: Some("video".into()),
            source: Some("camera".into()),
            ..TakeFile::new("/d/1 Camera.mov", TakeRole::Capture)
        });
        // A finished video without its raw recordings can't be remade.
        let mut finished_only = audio.clone();
        finished_only.files.push(TakeFile {
            kind: Some("video".into()),
            source: Some("pip".into()),
            ..TakeFile::new("/d/1 PiP.mp4", TakeRole::Derived)
        });
        assert!(!remake.applies(&audio));
        assert!(remake.applies(&camera));
        assert!(!remake.applies(&finished_only));
        assert!(anything.applies(&audio));
        assert!(camera.has("camera") && !camera.has("screen"));
    }

    #[test]
    fn manifests_decode_like_swift() {
        let manifest: PluginManifest = serde_json::from_str(
            r#"{"id":"a","name":"A","version":"1","protocol":1.0,"executable":"x","unknown":[1],
                "settings":[{"key":"k","title":"K","type":"number","default":6}]}"#,
        )
        .unwrap();
        assert_eq!(manifest.protocol, 1);
        assert_eq!(
            manifest.settings.unwrap()[0].default,
            Some(JsonValue::Number(6.0))
        );
        let missing = serde_json::from_str::<PluginManifest>(
            r#"{"id":"a","name":"A","version":"1","executable":"x"}"#,
        );
        assert_eq!(
            ManifestError::Decode(missing.unwrap_err().to_string()).to_string(),
            "The data couldn’t be read because it is missing."
        );
        let array_default = serde_json::from_str::<PluginSetting>(
            r#"{"key":"k","title":"K","type":"bool","default":[]}"#,
        );
        assert!(array_default.is_err());
    }

    #[test]
    fn setting_values_read_like_swift_descriptions() {
        assert_eq!(JsonValue::Number(6.0).to_string(), "6");
        assert_eq!(JsonValue::Number(0.5).to_string(), "0.5");
        assert_eq!(JsonValue::Bool(true).to_string(), "true");
        assert_eq!(JsonValue::Null.to_string(), "");
        assert_eq!(JsonValue::String("small".into()).to_string(), "small");
    }

    #[test]
    fn requests_encode_compactly_with_iso_dates() {
        let mut take = TakeDescriptor::new("d/s", "/tmp", "s");
        take.started = Some(json::date_from_seconds(1791738000.0));
        take.duration = Some(44.0);
        let request = PluginRequest::new(
            "action",
            "go",
            take,
            BTreeMap::from([("model".into(), JsonValue::String("small".into()))]),
            "/tmp",
            "/tmp",
        );
        assert_eq!(
            request.to_json(),
            r#"{"protocol":2,"kind":"action","name":"go","take":{"id":"d\/s","folder":"\/tmp","stem":"s","started":"2026-10-11T17:00:00Z","duration":44,"files":[],"speakers":[]},"settings":{"model":"small"},"outputDirectory":"\/tmp","pluginDirectory":"\/tmp"}"#
        );
        let back: PluginRequest = serde_json::from_str(&request.to_json()).unwrap();
        assert_eq!(back, request);
    }

    #[test]
    fn requests_match_swifts_encoder_key_for_key() {
        let mut take = TakeDescriptor::new("d/s", "/tmp", "s");
        take.started = Some(json::date_from_seconds(1791738000.0));
        take.duration = Some(44.0);
        take.files.push(TakeFile {
            mic: Some(1),
            ..TakeFile::new("/tmp/s TX1.wav", TakeRole::Track)
        });
        take.speakers.push(TakeSpeaker {
            id: "tx1".into(),
            mic: 1,
            name: "Host".into(),
            file: Some("/tmp/s TX1.wav".into()),
        });
        let settings = BTreeMap::from([
            ("model".into(), JsonValue::String("small".into())),
            ("n".into(), JsonValue::Number(6.0)),
            ("b".into(), JsonValue::Bool(true)),
            ("z".into(), JsonValue::Null),
        ]);
        let request = PluginRequest::new("action", "go", take, settings, "/tmp", "/tmp");
        // JSONEncoder with `.sortedKeys` and `.iso8601`, from LavdeckPluginKit.
        let swift = r#"{"kind":"action","name":"go","outputDirectory":"\/tmp","pluginDirectory":"\/tmp","protocol":2,"settings":{"b":true,"model":"small","n":6,"z":null},"take":{"duration":44,"files":[{"mic":1,"path":"\/tmp\/s TX1.wav","role":"track"}],"folder":"\/tmp","id":"d\/s","speakers":[{"file":"\/tmp\/s TX1.wav","id":"tx1","mic":1,"name":"Host"}],"started":"2026-10-11T17:00:00Z","stem":"s"}}"#;
        let ours: serde_json::Value = serde_json::from_str(&request.to_json()).unwrap();
        assert_eq!(
            ours,
            serde_json::from_str::<serde_json::Value>(swift).unwrap()
        );
    }
}
