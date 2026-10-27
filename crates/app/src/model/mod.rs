//! The view model: everything the deck, the menu bar icon, Recordings and Settings show,
//! mirroring what `AppModel.swift` and `PluginHost.swift` publish, plus the derived values
//! the views read (the glyph state, a strip's meter, its file label…). It holds no AppKit
//! objects, so the logic is tested directly. [`crate::store`] owns the live instance.

pub mod plugins;
pub mod recordings;
pub mod settings;
pub mod status;
pub mod takes;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::format;
use plugins::PluginHost;
use settings::{RecordingFormat, Settings};
use status::{ChannelMode, MicStatus, TransmitterStatus};

/// What needs attention, in priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    Dropped,
    LowBattery,
    Error,
}

/// The menu bar glyph's states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Healthy,
    Recording,
    Attention(Attention),
    Absent,
}

impl Glyph {
    pub const ALL: [Glyph; 6] = [
        Glyph::Healthy,
        Glyph::Recording,
        Glyph::Attention(Attention::Dropped),
        Glyph::Attention(Attention::LowBattery),
        Glyph::Attention(Attention::Error),
        Glyph::Absent,
    ];

    /// The snapshot file name part, as the Swift app writes it.
    pub fn file_name(self) -> &'static str {
        match self {
            Glyph::Healthy => "healthy",
            Glyph::Recording => "recording",
            Glyph::Attention(Attention::Dropped) => "attention-dropped",
            Glyph::Attention(Attention::LowBattery) => "attention-lowBattery",
            Glyph::Attention(Attention::Error) => "attention-error",
            Glyph::Absent => "absent",
        }
    }
}

/// A transmitter whose firmware is behind the newest one linked.
#[derive(Clone, Debug, PartialEq)]
pub struct Outdated {
    pub index: u8,
    pub firmware: String,
    pub newest: String,
    pub newest_index: u8,
}

/// The link LED's three states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Linked,
    /// A mic seen this session has dropped.
    Dropped,
    Off,
}

#[derive(Clone, Debug)]
pub struct AppState {
    // Live device state
    pub status: MicStatus,
    pub is_connected: bool,
    /// Transmitters seen linked at least once this session (a drop from this set is a problem).
    pub known_transmitters: BTreeSet<u8>,

    // Pending control writes
    pub pending_channel: Option<ChannelMode>,
    pub pending_gain: Option<i32>,
    pub last_error: Option<String>,

    // Recording
    pub is_recording: bool,
    /// Between Record and the first audio sample, while cameras and screen capture warm up.
    pub is_starting: bool,
    /// Seconds since 1970.
    pub recording_started: Option<f64>,
    /// Per-channel dBFS while recording.
    pub recording_levels: Vec<f32>,
    pub recording_mode: Option<ChannelMode>,
    /// Sources capturing in the current take (`screen`, `camera`, a plugin's).
    pub capture_sources: Vec<String>,
    pub last_saved_folder: Option<PathBuf>,

    pub settings: Settings,
    pub launch_at_login: bool,

    // Identify mode: temporarily stereo so each mic's own level shows which one is spoken into.
    pub identifying: bool,
    pub talking_mic: Option<u8>,
    pub identify_restore: Option<ChannelMode>,

    pub plugins: PluginHost,
    /// Bumped when a take is written or deleted, so Recordings refreshes.
    pub library_revision: u64,
    /// A take Recordings should select when it next appears ("Show" on a failure note).
    pub focus_take_id: Option<String>,
    /// Snapshot aid: report every linked mic as low.
    pub force_low_battery: bool,

    /// Demo mode: made-up mics, and names that never touch the user's saved ones.
    pub demo: bool,
    pub demo_names: BTreeMap<String, String>,
}

impl AppState {
    pub fn new(settings: Settings) -> AppState {
        AppState {
            status: MicStatus::default(),
            is_connected: false,
            known_transmitters: BTreeSet::new(),
            pending_channel: None,
            pending_gain: None,
            last_error: None,
            is_recording: false,
            is_starting: false,
            recording_started: None,
            recording_levels: Vec::new(),
            recording_mode: None,
            capture_sources: Vec::new(),
            last_saved_folder: None,
            settings,
            launch_at_login: false,
            identifying: false,
            talking_mic: None,
            identify_restore: None,
            plugins: PluginHost::default(),
            library_revision: 0,
            focus_take_id: None,
            force_low_battery: false,
            demo: false,
            demo_names: BTreeMap::new(),
        }
    }

    pub fn transmitter(&self, index: u8) -> Option<&TransmitterStatus> {
        self.status.transmitters.get(&index)
    }

    pub fn channel_mode(&self) -> Option<ChannelMode> {
        self.status.receiver.channel_mode
    }

    pub fn is_linked(&self, index: u8) -> bool {
        self.is_connected && self.status.receiver.connected_transmitters.contains(&index)
    }

    pub fn link(&self, index: u8) -> Link {
        if self.is_linked(index) {
            Link::Linked
        } else if self.known_transmitters.contains(&index) {
            Link::Dropped
        } else {
            Link::Off
        }
    }

    pub fn dropped_transmitters(&self) -> Vec<u8> {
        self.known_transmitters
            .iter()
            .copied()
            .filter(|i| !self.is_linked(*i))
            .collect()
    }

    /// Mics to warn about for low battery. The Mobile RX doesn't report a battery level (its
    /// level field is a fixed placeholder), so only the snapshot aid sets this.
    pub fn low_battery(&self) -> Vec<u8> {
        if self.force_low_battery {
            self.status
                .receiver
                .connected_transmitters
                .iter()
                .copied()
                .collect()
        } else {
            Vec::new()
        }
    }

    pub fn glyph(&self) -> Glyph {
        if self.is_recording {
            Glyph::Recording
        } else if !self.is_connected {
            Glyph::Absent
        } else if !self.dropped_transmitters().is_empty() {
            Glyph::Attention(Attention::Dropped)
        } else if self.last_error.is_some() {
            Glyph::Attention(Attention::Error)
        } else if !self.low_battery().is_empty() {
            Glyph::Attention(Attention::LowBattery)
        } else {
            Glyph::Healthy
        }
    }

    /// Transmitters whose firmware is older than the newest one known, and which one that is.
    pub fn outdated_firmware(&self) -> Vec<Outdated> {
        let versions: Vec<(u8, &str)> = self
            .status
            .transmitters
            .iter()
            .filter_map(|(i, tx)| tx.identity.firmware.as_deref().map(|f| (*i, f)))
            .collect();
        let Some(&(newest_index, newest)) = versions
            .iter()
            .max_by(|a, b| format::compare_numeric(a.1, b.1))
        else {
            return Vec::new();
        };
        versions
            .iter()
            .filter(|(_, f)| format::compare_numeric(f, newest).is_lt())
            .map(|(i, f)| Outdated {
                index: *i,
                firmware: f.to_string(),
                newest: newest.to_string(),
                newest_index,
            })
            .collect()
    }

    /// The single armed mic when exactly one is armed.
    pub fn solo_mic(&self) -> Option<u8> {
        let armed = &self.settings.armed;
        if armed.len() == 1 {
            armed.iter().next().copied()
        } else {
            None
        }
    }

    pub fn is_armed(&self, index: u8) -> bool {
        self.settings.armed.contains(&index)
    }

    /// What a strip's file label says while recording.
    pub fn destination(&self, index: u8) -> Option<String> {
        if !self.is_armed(index) {
            return None;
        }
        let ext = self.settings.format.key();
        if let Some(solo) = self.solo_mic() {
            return (solo == index).then(|| format!("TX{index}.{ext}"));
        }
        if self.recording_mode == Some(ChannelMode::Stereo) {
            return Some(
                if self.settings.split_tracks && self.settings.format == RecordingFormat::Wav {
                    format!("TX{index}.{ext}")
                } else {
                    "Stereo".into()
                },
            );
        }
        Some(format!("Mix.{ext}"))
    }

    pub fn displayed_channel(&self) -> Option<ChannelMode> {
        self.pending_channel.or(self.channel_mode())
    }

    pub fn displayed_gain(&self) -> Option<i32> {
        self.pending_gain.or(self.status.receiver.gain_db)
    }

    /// In mono the receiver mixes both mics, so it (and the audio) can only report the mix.
    pub fn meters_are_mix(&self) -> bool {
        self.channel_mode() != Some(ChannelMode::Stereo)
    }

    /// The recorded channel a strip reads while recording: its own in stereo, the mix in mono.
    fn level_channel(&self, index: u8) -> usize {
        if self.channel_mode() == Some(ChannelMode::Stereo) {
            usize::from(index.saturating_sub(1)).min(1)
        } else {
            0
        }
    }

    /// 0…1 meter fill for a strip. A disarmed mic reads 0 (it isn't in the take). While
    /// recording it's the recorded audio (L = TX1, R = TX2 in stereo); otherwise the receiver's
    /// own meter, per mic in stereo and the mix in mono.
    pub fn meter(&self, index: u8) -> f64 {
        if !(self.identifying || self.is_armed(index)) || !self.is_linked(index) {
            return 0.0;
        }
        if self.is_recording && self.recording_levels.len() == 2 {
            let db = f64::from(self.recording_levels[self.level_channel(index)]);
            return ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        }
        let Some(raw) = self.transmitter(index).and_then(|t| t.meter_level) else {
            return 0.0;
        };
        // Receiver meter units: ~30 in a quiet room, ~45 at rest with noise, ~75 for loud speech.
        ((f64::from(raw) - 25.0) / 60.0).clamp(0.0, 1.0)
    }

    pub fn dbfs(&self, index: u8) -> Option<i32> {
        if !self.is_recording || self.recording_levels.len() != 2 {
            return None;
        }
        Some(self.recording_levels[self.level_channel(index)].round() as i32)
    }

    /// The name shown on a strip: the user's name for that physical mic, else the slot.
    pub fn display_name(&self, index: u8) -> String {
        let id = self
            .transmitter(index)
            .and_then(|t| t.identity.bluetooth_id.as_deref());
        if self.demo
            && let Some(name) = id.and_then(|id| self.demo_names.get(id))
        {
            return name.clone();
        }
        match id.and_then(|id| self.settings.mic_names.get(id)) {
            Some(name) if !name.is_empty() => name.clone(),
            _ => format!("TX{index}"),
        }
    }

    /// The name to store for a strip: trimmed, at most 14 characters; empty or the slot name
    /// clears it. `None` when the mic has no Bluetooth ID to key it by.
    pub fn renamed(&self, index: u8, name: &str) -> Option<(String, Option<String>)> {
        let id = self.transmitter(index)?.identity.bluetooth_id.clone()?;
        let trimmed = name.trim();
        let value = if trimmed.is_empty() || trimmed == format!("TX{index}") {
            None
        } else {
            Some(trimmed.chars().take(14).collect())
        };
        Some((id, value))
    }

    /// The capture sources switched on for the next take, in the order the header names them
    /// while starting: Lavdeck's camera and screen, then plugins'.
    pub fn starting_sources(&self) -> Vec<String> {
        let v = &self.settings.video;
        let mut out: Vec<String> = [v.camera.then_some("camera"), v.screen.then_some("screen")]
            .into_iter()
            .flatten()
            .map(String::from)
            .collect();
        for t in self.plugins.capture_toggles() {
            if self.plugins.is_on(&t) {
                out.push(t.source);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use status::DeviceIdentity;

    fn connected() -> AppState {
        let mut s = AppState::new(Settings::defaults(std::path::Path::new("/Users/me")));
        s.is_connected = true;
        s.status.receiver.channel_mode = Some(ChannelMode::Stereo);
        s.status.receiver.connected_transmitters = [1, 2].into();
        for (i, id, fw, meter) in [(1, "a1", "02.03.17.00", 58), (2, "b2", "02.03.17.00", 41)] {
            let mut tx = TransmitterStatus::new(i);
            tx.identity = DeviceIdentity {
                bluetooth_id: Some(id.into()),
                firmware: Some(fw.into()),
                ..Default::default()
            };
            tx.meter_level = Some(meter);
            s.status.transmitters.insert(i, tx);
        }
        s.known_transmitters = [1, 2].into();
        s
    }

    #[test]
    fn glyph_priority() {
        let mut s = connected();
        assert_eq!(s.glyph(), Glyph::Healthy);
        s.force_low_battery = true;
        assert_eq!(s.glyph(), Glyph::Attention(Attention::LowBattery));
        s.last_error = Some("x".into());
        assert_eq!(s.glyph(), Glyph::Attention(Attention::Error));
        s.status.receiver.connected_transmitters.remove(&2);
        assert_eq!(s.glyph(), Glyph::Attention(Attention::Dropped));
        assert_eq!(s.dropped_transmitters(), [2]);
        assert_eq!(s.link(2), Link::Dropped);
        s.is_connected = false;
        assert_eq!(s.glyph(), Glyph::Absent);
        s.is_recording = true;
        assert_eq!(s.glyph(), Glyph::Recording);
    }

    #[test]
    fn meters() {
        let mut s = connected();
        assert!((s.meter(1) - 33.0 / 60.0).abs() < 1e-9);
        s.settings.armed = [2].into();
        assert_eq!(s.meter(1), 0.0, "disarmed");
        s.identifying = true;
        assert!(s.meter(1) > 0.0, "identify shows every mic");
        s.identifying = false;
        s.settings.armed = [1, 2].into();
        s.is_recording = true;
        s.recording_levels = vec![-14.0, -21.0];
        assert!((s.meter(1) - 46.0 / 60.0).abs() < 1e-9);
        assert_eq!((s.dbfs(1), s.dbfs(2)), (Some(-14), Some(-21)));
        s.status.receiver.channel_mode = Some(ChannelMode::Mono);
        assert_eq!(s.dbfs(2), Some(-14), "mono reads the mix");
        assert!(s.meters_are_mix());
    }

    #[test]
    fn destinations() {
        let mut s = connected();
        s.recording_mode = Some(ChannelMode::Stereo);
        assert_eq!(s.destination(1).as_deref(), Some("TX1.wav"));
        s.settings.split_tracks = false;
        assert_eq!(s.destination(2).as_deref(), Some("Stereo"));
        s.recording_mode = Some(ChannelMode::Mono);
        assert_eq!(s.destination(2).as_deref(), Some("Mix.wav"));
        s.settings.armed = [2].into();
        s.settings.format = RecordingFormat::M4a;
        assert_eq!(s.destination(1), None);
        assert_eq!(
            s.destination(2).as_deref(),
            Some("TX2.m4a"),
            "a solo mic records alone"
        );
    }

    #[test]
    fn names() {
        let mut s = connected();
        assert_eq!(s.display_name(1), "TX1");
        s.settings.mic_names.insert("a1".into(), "Host".into());
        assert_eq!(s.display_name(1), "Host");
        assert_eq!(
            s.renamed(1, "  A very long name indeed "),
            Some(("a1".into(), Some("A very long na".into())))
        );
        assert_eq!(s.renamed(1, "TX1"), Some(("a1".into(), None)));
        assert_eq!(s.renamed(1, "   "), Some(("a1".into(), None)));
        s.status
            .transmitters
            .get_mut(&2)
            .unwrap()
            .identity
            .bluetooth_id = None;
        assert_eq!(s.renamed(2, "Guest"), None);
        s.demo = true;
        s.demo_names.insert("a1".into(), "Demo".into());
        assert_eq!(s.display_name(1), "Demo");
    }

    #[test]
    fn outdated_firmware() {
        let mut s = connected();
        assert!(s.outdated_firmware().is_empty());
        s.status.transmitters.get_mut(&2).unwrap().identity.firmware = Some("02.03.9.00".into());
        assert_eq!(
            s.outdated_firmware(),
            [Outdated {
                index: 2,
                firmware: "02.03.9.00".into(),
                newest: "02.03.17.00".into(),
                newest_index: 1
            }]
        );
    }

    #[test]
    fn solo_and_pending() {
        let mut s = connected();
        assert_eq!(s.solo_mic(), None);
        s.settings.armed = [2].into();
        assert_eq!(s.solo_mic(), Some(2));
        s.status.receiver.gain_db = Some(0);
        s.pending_gain = Some(6);
        assert_eq!(s.displayed_gain(), Some(6));
        s.pending_channel = Some(ChannelMode::Mono);
        assert_eq!(s.displayed_channel(), Some(ChannelMode::Mono));
    }
}
