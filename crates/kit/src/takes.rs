//! Takes on disk: `<root>/<yyyy-MM-dd>/<HH.mm.ss> …` files grouped by their start-time stem, and
//! the `<stem>.take.json` metadata Lavdeck writes when a recording finishes.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use objc2_foundation::{NSFileManager, NSString, NSURL};
use serde::{Deserialize, Serialize};

use crate::audio;
use crate::json;
use crate::protocol::{TakeDescriptor, TakeFile, TakeRole, TakeSpeaker};

/// Lavdeck's per-take metadata, written as `<stem>.take.json` when a recording finishes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeMetadata {
    #[serde(default = "one", deserialize_with = "json::int::deserialize")]
    pub version: i64,
    #[serde(with = "json::date")]
    pub started: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_mode: Option<String>,
    #[serde(default = "wav")]
    pub format: String,
    #[serde(default)]
    pub mics: Vec<TakeMic>,
    /// Host-clock seconds of the audio's first sample, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_start_host_time: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Vec<TakeMedia>>,
}

fn one() -> i64 {
    1
}

fn wav() -> String {
    "wav".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeMic {
    #[serde(deserialize_with = "json::int::deserialize")]
    pub index: i64,
    pub name: String,
    #[serde(
        rename = "bluetoothID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bluetooth_id: Option<String>,
    pub armed: bool,
}

/// A file a capture plugin recorded with the take, and where it sits relative to the audio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakeMedia {
    /// File name in the take's folder.
    pub file: String,
    /// `screen`, `camera`, `system-audio`.
    pub source: String,
    /// `video` or `audio`.
    pub kind: String,
    /// Seconds after the first audio sample that the file's first frame happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
}

impl TakeMetadata {
    pub fn new(
        started: SystemTime,
        duration: Option<f64>,
        channel_mode: Option<String>,
        format: &str,
        mics: Vec<TakeMic>,
    ) -> Self {
        Self {
            version: 1,
            started,
            duration,
            channel_mode,
            format: format.into(),
            mics,
            audio_start_host_time: None,
            media: None,
        }
    }

    /// The file's text: pretty-printed, sorted keys, no trailing newline (as Foundation writes it).
    pub fn to_json(&self) -> String {
        json::to_pretty_sorted(&json::tree(self))
    }

    /// Writes the file atomically (a temporary file renamed over the target).
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        write_atomically(path, self.to_json().as_bytes())
    }

    /// Tolerant of missing optional keys, so a hand-written or older file still yields its mic names.
    pub fn load(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// A recording on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    pub descriptor: TakeDescriptor,
    pub metadata: Option<TakeMetadata>,
}

impl Take {
    pub fn id(&self) -> &str {
        &self.descriptor.id
    }

    pub fn transcript_path(&self) -> PathBuf {
        Path::new(&self.descriptor.folder).join(format!("{}.transcript.json", self.descriptor.stem))
    }

    /// From the scanned file list, so lists don't stat the disk on every render.
    pub fn has_transcript(&self) -> bool {
        self.descriptor.has("transcript")
    }

    pub fn audio_files(&self) -> Vec<&TakeFile> {
        let audio = [TakeRole::Stereo, TakeRole::Mix, TakeRole::Track];
        self.descriptor
            .files
            .iter()
            .filter(|f| audio.contains(&f.role))
            .collect()
    }

    pub fn video_files(&self) -> Vec<&TakeFile> {
        self.descriptor
            .files
            .iter()
            .filter(|f| f.kind.as_deref() == Some("video"))
            .collect()
    }

    pub fn all_paths(&self) -> Vec<PathBuf> {
        self.descriptor
            .files
            .iter()
            .map(|f| PathBuf::from(&f.path))
            .collect()
    }
}

/// Scans a recordings root into takes, newest first.
pub fn scan(root: &Path, mic_names: &HashMap<String, String>) -> Vec<Take> {
    let Ok(days) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut takes = Vec::new();
    for day in days.flatten() {
        if !day.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let day = day.path();
        let Ok(entries) = std::fs::read_dir(&day) else {
            continue;
        };
        let mut by_stem: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if let Some(stem) = stem_of(&name) {
                by_stem.entry(stem.to_string()).or_default().push(name);
            }
        }
        for (stem, files) in by_stem {
            if let Some(take) = take(&day, &stem, Some(&files), mic_names)
                && !take.audio_files().is_empty()
            {
                takes.push(take);
            }
        }
    }
    let key = |t: &Take| {
        t.descriptor
            .started
            .map(json::seconds_since_epoch)
            .unwrap_or(f64::MIN)
    };
    takes.sort_by(|a, b| key(b).total_cmp(&key(a)));
    takes
}

/// `HH.MM.SS` at the start of a file name (Lavdeck's take stem).
fn stem_of(name: &str) -> Option<&str> {
    let b = name.as_bytes();
    let digits = |i: usize| {
        b.get(i).is_some_and(u8::is_ascii_digit) && b.get(i + 1).is_some_and(u8::is_ascii_digit)
    };
    (digits(0) && b.get(2) == Some(&b'.') && digits(3) && b.get(5) == Some(&b'.') && digits(6))
        .then(|| &name[..8])
}

/// Builds one take from its folder and stem (used right after recording and by [`scan`]).
/// Without `file_names`, every file in `folder` starting with `stem` belongs to it.
pub fn take(
    folder: &Path,
    stem: &str,
    file_names: Option<&[String]>,
    mic_names: &HashMap<String, String>,
) -> Option<Take> {
    let mut names: Vec<String> = match file_names {
        Some(names) => names.to_vec(),
        None => std::fs::read_dir(folder)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.starts_with(stem))
                    .collect()
            })
            .unwrap_or_default(),
    };
    names.sort();
    let metadata = TakeMetadata::load(&folder.join(format!("{stem}.take.json")));
    let mut files = Vec::new();
    for name in &names {
        let path = folder.join(name).to_string_lossy().into_owned();
        let rest = trim_whitespace(name.get(stem.len()..).unwrap_or(""));
        let ext = path_extension(name).to_lowercase();
        let base = deleting_path_extension(rest);
        let file = |role: TakeRole| TakeFile::new(&path, role);
        let media = metadata
            .as_ref()
            .and_then(|m| m.media.as_ref())
            .and_then(|m| m.iter().find(|m| &m.file == name));
        if name.ends_with(".take.json") {
            files.push(file(TakeRole::Metadata));
        } else if let Some(media) = media {
            files.push(TakeFile {
                kind: Some(media.kind.clone()),
                source: Some(media.source.clone()),
                offset: media.offset,
                ..file(TakeRole::Capture)
            });
        } else if ["mov", "mp4", "m4v"].contains(&ext.as_str()) {
            let (role, source) = match base {
                "Screen" => (TakeRole::Capture, Some("screen")),
                "Camera" => (TakeRole::Capture, Some("camera")),
                "PiP" => (TakeRole::Derived, Some("pip")),
                _ => (TakeRole::Derived, None),
            };
            files.push(TakeFile {
                kind: Some("video".into()),
                source: source.map(Into::into),
                ..file(role)
            });
        } else if ["wav", "m4a", "caf", "aif", "aiff"].contains(&ext.as_str()) {
            files.push(match base {
                "Stereo" => file(TakeRole::Stereo),
                "Mix" => file(TakeRole::Mix),
                "TX1" => TakeFile {
                    mic: Some(1),
                    ..file(TakeRole::Track)
                },
                "TX2" => TakeFile {
                    mic: Some(2),
                    ..file(TakeRole::Track)
                },
                _ => {
                    let mic = if base.starts_with("TX2") {
                        Some(2)
                    } else if base.starts_with("TX1") {
                        Some(1)
                    } else {
                        None
                    };
                    TakeFile {
                        mic,
                        kind: Some("audio".into()),
                        ..file(TakeRole::Derived)
                    }
                }
            });
        } else if name.contains(".transcript.") {
            files.push(TakeFile {
                kind: Some("transcript".into()),
                ..file(TakeRole::Derived)
            });
        } else {
            files.push(TakeFile {
                kind: Some(ext),
                ..file(TakeRole::Derived)
            });
        }
    }
    if files.is_empty() {
        return None;
    }

    let day = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let started = metadata
        .as_ref()
        .map(|m| m.started)
        .or_else(|| start_date(&day, stem));
    let main = [TakeRole::Stereo, TakeRole::Mix, TakeRole::Track]
        .into_iter()
        .find_map(|role| files.iter().find(|f| f.role == role));
    let duration = metadata
        .as_ref()
        .and_then(|m| m.duration)
        .or_else(|| main.and_then(|f| audio::duration(Path::new(&f.path))));
    let has = |role: TakeRole| files.iter().any(|f| f.role == role);
    let mode = metadata
        .as_ref()
        .and_then(|m| m.channel_mode.clone())
        .or_else(|| {
            if has(TakeRole::Stereo) {
                Some("stereo".into())
            } else if has(TakeRole::Mix) {
                Some("mono".into())
            } else {
                None
            }
        });

    let mut speakers = Vec::new();
    for index in [1, 2] {
        let track = files
            .iter()
            .find(|f| f.role == TakeRole::Track && f.mic == Some(index));
        let meta = metadata
            .as_ref()
            .and_then(|m| m.mics.iter().find(|mic| mic.index == index));
        // A mic entry always carries a name; `mic_names` only fills in for one without.
        let name = meta
            .map(|m| m.name.clone())
            .or_else(|| {
                meta.and_then(|m| m.bluetooth_id.as_ref())
                    .and_then(|id| mic_names.get(id).cloned())
            })
            .unwrap_or_else(|| format!("TX{index}"));
        if track.is_some() || meta.is_some_and(|m| m.armed) {
            speakers.push(TakeSpeaker {
                id: format!("tx{index}"),
                mic: index,
                name,
                file: track.map(|t| t.path.clone()),
            });
        }
    }
    let descriptor = TakeDescriptor {
        id: format!("{day}/{stem}"),
        folder: folder.to_string_lossy().into_owned(),
        stem: stem.into(),
        started,
        duration,
        channel_mode: mode,
        files,
        speakers,
    };
    Some(Take {
        descriptor,
        metadata,
    })
}

/// Moves every file of a take to the Trash (recoverable).
pub fn trash(take: &Take) -> Result<(), String> {
    let manager = NSFileManager::defaultManager();
    for path in take.all_paths() {
        if !path.exists() {
            continue;
        }
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        manager
            .trashItemAtURL_resultingItemURL_error(&url, None)
            .map_err(|e| e.localizedDescription().to_string())?;
    }
    Ok(())
}

// MARK: Names

/// The day folder (`2026-10-11`) and stem (`19.49.23`) for a take starting at `date`, in local time
/// (UTC put evening takes in tomorrow's folder).
pub fn day_and_stem(date: SystemTime) -> (String, String) {
    let seconds = date
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    // SAFETY: localtime_r fills the tm we pass.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&seconds, &mut tm);
        tm
    };
    (
        format!(
            "{:04}-{:02}-{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday
        ),
        format!("{:02}.{:02}.{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec),
    )
}

/// The local time a take started, from its day folder and stem.
pub fn start_date(day: &str, stem: &str) -> Option<SystemTime> {
    let d = day.as_bytes();
    let s = stem.as_bytes();
    if d.len() != 10 || d[4] != b'-' || d[7] != b'-' || s.len() != 8 || s[2] != b'.' || s[5] != b'.'
    {
        return None;
    }
    let num = |b: &[u8]| -> Option<i32> {
        b.iter()
            .all(u8::is_ascii_digit)
            .then(|| b.iter().fold(0, |acc, c| acc * 10 + (c - b'0') as i32))
    };
    let (year, month, mday) = (num(&d[..4])?, num(&d[5..7])?, num(&d[8..10])?);
    let (hour, minute, second) = (num(&s[..2])?, num(&s[3..5])?, num(&s[6..8])?);
    let days_in_month = match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return None,
    };
    if mday < 1 || mday > days_in_month || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    // SAFETY: mktime reads and normalises the tm we pass.
    let t = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = year - 1900;
        tm.tm_mon = month - 1;
        tm.tm_mday = mday;
        tm.tm_hour = hour;
        tm.tm_min = minute;
        tm.tm_sec = second;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm)
    };
    Some(json::date_from_seconds(t as f64))
}

/// `NSString.pathExtension` for a file name: the part after the last dot, if any.
fn path_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[i + 1..],
        _ => "",
    }
}

/// `NSString.deletingPathExtension` for a file name.
fn deleting_path_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[..i],
        _ => name,
    }
}

/// Trims Foundation's `.whitespaces` (spaces and tabs, not newlines).
fn trim_whitespace(s: &str) -> &str {
    let line_break = |c: char| {
        matches!(
            c,
            '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        )
    };
    s.trim_matches(|c: char| c.is_whitespace() && !line_break(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch_folder;

    #[test]
    fn take_library_groups_files_by_stem() {
        let root = scratch_folder();
        let day = root.join("2026-10-11");
        std::fs::create_dir_all(&day).unwrap();
        for name in [
            "12.00.00 TX1.wav",
            "12.00.00 TX2.wav",
            "12.00.00.transcript.json",
            "12.00.00 TX1 · voice-ada.wav",
            "13.30.05 Mix.wav",
            "notes.txt",
        ] {
            std::fs::write(day.join(name), b"").unwrap();
        }
        let mics = vec![
            TakeMic {
                index: 1,
                name: "Host".into(),
                bluetooth_id: Some("aa".into()),
                armed: true,
            },
            TakeMic {
                index: 2,
                name: "Guest".into(),
                bluetooth_id: Some("bb".into()),
                armed: true,
            },
        ];
        TakeMetadata::new(
            json::date_from_seconds(0.0),
            Some(42.0),
            Some("stereo".into()),
            "wav",
            mics,
        )
        .write(&day.join("12.00.00.take.json"))
        .unwrap();

        let takes = scan(&root, &HashMap::new());
        let mut ids: Vec<&str> = takes.iter().map(Take::id).collect();
        ids.sort();
        assert_eq!(ids, ["2026-10-11/12.00.00", "2026-10-11/13.30.05"]);
        let duo = takes
            .iter()
            .find(|t| t.id() == "2026-10-11/12.00.00")
            .unwrap();
        assert!(duo.descriptor.is_two_speaker());
        assert_eq!(
            duo.descriptor
                .speakers
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["Host", "Guest"]
        );
        assert_eq!(duo.descriptor.duration, Some(42.0));
        assert!(duo.has_transcript());
        assert!(
            duo.descriptor
                .files
                .iter()
                .any(|f| f.role == TakeRole::Derived
                    && f.kind.as_deref() == Some("audio")
                    && f.mic == Some(1))
        );
        let mono = takes
            .iter()
            .find(|t| t.id() == "2026-10-11/13.30.05")
            .unwrap();
        assert_eq!(mono.descriptor.channel_mode.as_deref(), Some("mono"));
        assert!(mono.descriptor.speakers.is_empty());
        assert!(!mono.has_transcript());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn take_library_reads_captured_media_with_offsets() {
        let root = scratch_folder();
        let day = root.join("2026-10-11");
        std::fs::create_dir_all(&day).unwrap();
        for name in [
            "09.00.00 Stereo.wav",
            "09.00.00 Screen.mov",
            "09.00.00 Camera.mov",
            "09.00.00 PiP.mp4",
        ] {
            std::fs::write(day.join(name), b"").unwrap();
        }
        let mut metadata = TakeMetadata::new(
            SystemTime::now(),
            Some(10.0),
            Some("stereo".into()),
            "wav",
            vec![],
        );
        metadata.audio_start_host_time = Some(50.0);
        metadata.media = Some(vec![TakeMedia {
            file: "09.00.00 Screen.mov".into(),
            source: "screen".into(),
            kind: "video".into(),
            offset: Some(0.42),
            plugin: Some("p".into()),
        }]);
        metadata.write(&day.join("09.00.00.take.json")).unwrap();
        let takes = scan(&root, &HashMap::new());
        let take = &takes[0];
        let screen = take
            .descriptor
            .files
            .iter()
            .find(|f| f.source.as_deref() == Some("screen"))
            .unwrap();
        assert!(screen.role == TakeRole::Capture && screen.offset == Some(0.42));
        let camera = take
            .descriptor
            .files
            .iter()
            .find(|f| f.source.as_deref() == Some("camera"))
            .unwrap();
        assert!(camera.role == TakeRole::Capture && camera.offset.is_none());
        assert_eq!(
            take.descriptor
                .files
                .iter()
                .find(|f| f.source.as_deref() == Some("pip"))
                .unwrap()
                .role,
            TakeRole::Derived
        );
        assert_eq!(take.video_files().len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn metadata_is_written_as_foundation_writes_it() {
        let mics = vec![
            TakeMic {
                index: 1,
                name: "TX1".into(),
                bluetooth_id: Some("x".into()),
                armed: true,
            },
            TakeMic {
                index: 2,
                name: "TX2".into(),
                bluetooth_id: None,
                armed: false,
            },
        ];
        let mut metadata = TakeMetadata::new(
            json::parse_iso8601("2026-10-12T16:43:54Z").unwrap(),
            Some(125.2405298948288),
            Some("stereo".into()),
            "wav",
            mics,
        );
        metadata.audio_start_host_time = Some(2406254.0812708335);
        metadata.media = Some(vec![TakeMedia {
            file: "12.43.53 Screen.mov".into(),
            source: "screen".into(),
            kind: "video".into(),
            offset: Some(-0.29029862536117435),
            plugin: Some("io.github.piedol.lavdeck.capture".into()),
        }]);
        let expected = r#"{
  "audioStartHostTime" : 2406254.0812708335,
  "channelMode" : "stereo",
  "duration" : 125.2405298948288,
  "format" : "wav",
  "media" : [
    {
      "file" : "12.43.53 Screen.mov",
      "kind" : "video",
      "offset" : -0.29029862536117435,
      "plugin" : "io.github.piedol.lavdeck.capture",
      "source" : "screen"
    }
  ],
  "mics" : [
    {
      "armed" : true,
      "bluetoothID" : "x",
      "index" : 1,
      "name" : "TX1"
    },
    {
      "armed" : false,
      "index" : 2,
      "name" : "TX2"
    }
  ],
  "started" : "2026-10-12T16:43:54Z",
  "version" : 1
}"#;
        assert_eq!(metadata.to_json(), expected);
        assert_eq!(
            serde_json::from_str::<TakeMetadata>(expected).unwrap(),
            metadata
        );
    }

    #[test]
    fn metadata_tolerates_missing_optional_keys_but_needs_started() {
        let minimal: TakeMetadata = serde_json::from_str(
            r#"{"started":"2026-10-11T17:00:00Z","mics":[{"index":1,"name":"Host","armed":true}]}"#,
        )
        .unwrap();
        assert_eq!(
            (
                minimal.version,
                minimal.format.as_str(),
                minimal.mics[0].name.as_str()
            ),
            (1, "wav", "Host")
        );
        assert!(serde_json::from_str::<TakeMetadata>(r#"{"format":"wav"}"#).is_err());
    }

    #[test]
    fn names_and_dates() {
        assert_eq!(stem_of("12.00.00 TX1.wav"), Some("12.00.00"));
        assert_eq!(stem_of("12.00.0 TX1.wav"), None);
        assert_eq!(path_extension("12.00.00 TX1 · voice-ada.wav"), "wav");
        assert_eq!(
            deleting_path_extension("TX1 · voice-ada.wav"),
            "TX1 · voice-ada"
        );
        assert_eq!(path_extension("12.00.00"), "00");
        let started = start_date("2026-10-11", "19.49.23").unwrap();
        assert_eq!(
            day_and_stem(started),
            ("2026-10-11".into(), "19.49.23".into())
        );
        assert!(start_date("2026-02-30", "10.00.00").is_none());
        assert!(start_date("Projects", "10.00.00").is_none());
    }
}
