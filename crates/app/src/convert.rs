//! Between the kit's and capture's types and the view model's. The view model keeps its own
//! small types (built for the views and their tests); these map them field for field.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lavdeck_kit as kit;

use crate::model::plugins::{
    AppSpec, JsonValue, Manifest, Plugin, PluginAction, PluginSetting, SettingKind,
};
use crate::model::settings::{RecordingFormat, VideoSettings};
use crate::model::status::{
    ChannelMode, DeviceIdentity, MicStatus, ReceiverStatus, TransmitterStatus,
};
use crate::model::takes::{Role, Take, TakeFile, TakeSpeaker};

pub fn seconds(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn system_time(t: f64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs_f64(t.max(0.0))
}

pub fn mode(m: kit::ChannelMode) -> ChannelMode {
    match m {
        kit::ChannelMode::Mono => ChannelMode::Mono,
        kit::ChannelMode::Stereo => ChannelMode::Stereo,
        other => ChannelMode::Other(other.raw()),
    }
}

pub fn kit_mode(m: ChannelMode) -> kit::ChannelMode {
    match m {
        ChannelMode::Mono => kit::ChannelMode::Mono,
        ChannelMode::Stereo => kit::ChannelMode::Stereo,
        ChannelMode::Other(raw) => kit::ChannelMode::from_raw(raw),
    }
}

pub fn format(f: RecordingFormat) -> kit::RecordingFormat {
    match f {
        RecordingFormat::Wav => kit::RecordingFormat::Wav,
        RecordingFormat::M4a => kit::RecordingFormat::M4a,
    }
}

fn identity(i: &kit::DeviceIdentity) -> DeviceIdentity {
    DeviceIdentity {
        product_name: i.product_name.clone(),
        firmware: i.firmware.clone(),
        serial_number: i.serial_number.clone(),
        bluetooth_id: i.bluetooth_id.clone(),
    }
}

/// Transmitter slots outside 1…255 can't be shown; the receiver uses 1 and 2.
fn slot(i: i32) -> Option<u8> {
    u8::try_from(i).ok().filter(|i| *i > 0)
}

pub fn status(s: &kit::MicStatus) -> MicStatus {
    let r = &s.receiver;
    MicStatus {
        receiver: ReceiverStatus {
            identity: identity(&r.identity),
            channel_mode: r.channel_mode.map(mode),
            noise_cancelling: r.noise_cancelling,
            noise_cancelling_level: r.noise_cancelling_level,
            gain_db: r.gain_db,
            connected_transmitters: r
                .connected_transmitters
                .iter()
                .filter_map(|i| slot(*i))
                .collect(),
        },
        transmitters: s
            .transmitters
            .iter()
            .filter_map(|(i, tx)| {
                let index = slot(*i)?;
                Some((
                    index,
                    TransmitterStatus {
                        index,
                        identity: identity(&tx.identity),
                        battery_level: tx.battery_level,
                        is_charging: tx.is_charging,
                        meter_level: tx.meter_level,
                        raw_flags: tx.raw_flags,
                    },
                ))
            })
            .collect(),
    }
}

fn role(r: kit::TakeRole) -> Role {
    match r {
        kit::TakeRole::Stereo => Role::Stereo,
        kit::TakeRole::Mix => Role::Mix,
        kit::TakeRole::Track => Role::Track,
        kit::TakeRole::Derived => Role::Derived,
        kit::TakeRole::Metadata => Role::Metadata,
        kit::TakeRole::Capture => Role::Capture,
    }
}

fn kit_role(r: Role) -> kit::TakeRole {
    match r {
        Role::Stereo => kit::TakeRole::Stereo,
        Role::Mix => kit::TakeRole::Mix,
        Role::Track => kit::TakeRole::Track,
        Role::Derived => kit::TakeRole::Derived,
        Role::Metadata => kit::TakeRole::Metadata,
        Role::Capture => kit::TakeRole::Capture,
    }
}

pub fn take(d: &kit::TakeDescriptor) -> Take {
    Take {
        id: d.id.clone(),
        folder: PathBuf::from(&d.folder),
        stem: d.stem.clone(),
        started: d.started.map(seconds),
        duration: d.duration,
        channel_mode: d.channel_mode.clone(),
        files: d
            .files
            .iter()
            .map(|f| TakeFile {
                path: PathBuf::from(&f.path),
                role: role(f.role),
                mic: f.mic.and_then(|m| u8::try_from(m).ok()),
                kind: f.kind.clone(),
                source: f.source.clone(),
                offset: f.offset,
            })
            .collect(),
        speakers: d
            .speakers
            .iter()
            .map(|s| TakeSpeaker {
                id: s.id.clone(),
                mic: u8::try_from(s.mic).unwrap_or(0),
                name: s.name.clone(),
                file: s.file.as_ref().map(PathBuf::from),
            })
            .collect(),
    }
}

pub fn descriptor(t: &Take) -> kit::TakeDescriptor {
    let mut d = kit::TakeDescriptor::new(&t.id, &t.folder.to_string_lossy(), &t.stem);
    d.started = t.started.map(system_time);
    d.duration = t.duration;
    d.channel_mode = t.channel_mode.clone();
    d.files = t
        .files
        .iter()
        .map(|f| {
            let mut k = kit::TakeFile::new(&f.path.to_string_lossy(), kit_role(f.role));
            k.mic = f.mic.map(i64::from);
            k.kind = f.kind.clone();
            k.source = f.source.clone();
            k.offset = f.offset;
            k
        })
        .collect();
    d.speakers = t
        .speakers
        .iter()
        .map(|s| kit::TakeSpeaker {
            id: s.id.clone(),
            mic: i64::from(s.mic),
            name: s.name.clone(),
            file: s.file.as_ref().map(|p| p.to_string_lossy().into_owned()),
        })
        .collect();
    d
}

pub fn value(v: &kit::JsonValue) -> JsonValue {
    match v {
        kit::JsonValue::String(s) => JsonValue::String(s.clone()),
        kit::JsonValue::Number(n) => JsonValue::Number(*n),
        kit::JsonValue::Bool(b) => JsonValue::Bool(*b),
        kit::JsonValue::Null => JsonValue::Null,
    }
}

pub fn kit_value(v: &JsonValue) -> kit::JsonValue {
    match v {
        JsonValue::String(s) => kit::JsonValue::String(s.clone()),
        JsonValue::Number(n) => kit::JsonValue::Number(*n),
        JsonValue::Bool(b) => kit::JsonValue::Bool(*b),
        JsonValue::Null => kit::JsonValue::Null,
    }
}

pub fn values(m: &BTreeMap<String, kit::JsonValue>) -> BTreeMap<String, JsonValue> {
    m.iter().map(|(k, v)| (k.clone(), value(v))).collect()
}

fn setting_kind(k: kit::PluginSettingKind) -> SettingKind {
    match k {
        kit::PluginSettingKind::Bool => SettingKind::Bool,
        kit::PluginSettingKind::String => SettingKind::String,
        kit::PluginSettingKind::Number => SettingKind::Number,
        kit::PluginSettingKind::Choice => SettingKind::Choice,
        kit::PluginSettingKind::Path => SettingKind::Path,
    }
}

pub fn plugin(p: &kit::InstalledPlugin) -> Plugin {
    let m = &p.manifest;
    Plugin {
        folder: p.folder.clone(),
        manifest: Manifest {
            id: m.id.clone(),
            name: m.name.clone(),
            version: m.version.clone(),
            summary: m.summary.clone(),
            events: m.events.clone().unwrap_or_default(),
            actions: m
                .actions
                .iter()
                .flatten()
                .map(|a| PluginAction {
                    id: a.id.clone(),
                    title: a.title.clone(),
                    requires: a.requires.clone().unwrap_or_default(),
                })
                .collect(),
            settings: m
                .settings
                .iter()
                .flatten()
                .map(|s| PluginSetting {
                    key: s.key.clone(),
                    title: s.title.clone(),
                    kind: setting_kind(s.kind),
                    default: s.default.as_ref().map(value).unwrap_or(JsonValue::Null),
                    options: s
                        .options
                        .iter()
                        .flatten()
                        .map(|o| (o.value.clone(), o.title.clone()))
                        .collect(),
                    help: s.help.clone(),
                })
                .collect(),
            produces: m.produces.clone().unwrap_or_default(),
            kind: m.kind.clone(),
            permissions: m.permissions.clone().unwrap_or_default(),
            capture_sources: m
                .capture
                .as_ref()
                .map(|c| c.sources.clone().unwrap_or_default()),
            app: m.app.as_ref().map(|a| AppSpec {
                bundle: a.bundle.clone(),
                open_take: a.open_take.clone(),
                action_title: a.action_title.clone(),
            }),
        },
    }
}

/// The view model's video settings and the capture crate's share one JSON form (`video.settings`).
pub fn video(v: &VideoSettings) -> lavdeck_capture::VideoSettings {
    serde_json::to_value(v)
        .ok()
        .and_then(|j| serde_json::from_value(j).ok())
        .unwrap_or_default()
}

pub fn video_from(v: &lavdeck_capture::VideoSettings) -> VideoSettings {
    serde_json::to_value(v)
        .ok()
        .and_then(|j| serde_json::from_value(j).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_settings_cross_over() {
        let v = VideoSettings {
            screen: true,
            screen_fps: 60,
            bubble_corner: "top-left".into(),
            codec: "h264".into(),
            ..Default::default()
        };
        let back = video_from(&video(&v));
        assert_eq!(back, v);
        assert_eq!(video(&v).screen_fps, 60);
    }

    #[test]
    fn takes_cross_over() {
        let mut d = kit::TakeDescriptor::new("2026-10-26/11.05.09", "/r/2026-10-26", "11.05.09");
        d.started = Some(system_time(1_790_000_000.0));
        d.duration = Some(842.0);
        let mut f = kit::TakeFile::new("/r/2026-10-26/11.05.09 TX1.wav", kit::TakeRole::Track);
        f.mic = Some(1);
        d.files.push(f);
        d.speakers.push(kit::TakeSpeaker {
            id: "tx1".into(),
            mic: 1,
            name: "Host".into(),
            file: Some("/r/2026-10-26/11.05.09 TX1.wav".into()),
        });
        let t = take(&d);
        assert_eq!(t.files[0].mic, Some(1));
        assert_eq!(descriptor(&t), d);
    }
}
