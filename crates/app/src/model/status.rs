//! What the receiver reports, as the views read it. These mirror `LavdeckKit`'s status
//! types; the kit crate's decoder fills them once it's wired in (phase 2).

use std::collections::{BTreeMap, BTreeSet};

/// Receiver channel mode. The Mobile RX reports 0 for mono and 2 for stereo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelMode {
    Mono,
    Stereo,
    Other(u8),
}

impl ChannelMode {
    pub fn from_raw(raw: u8) -> ChannelMode {
        match raw {
            0 => ChannelMode::Mono,
            2 => ChannelMode::Stereo,
            r => ChannelMode::Other(r),
        }
    }

    /// The name the Swift app writes into `.take.json` (`mono`, `stereo`, `mode 1`).
    pub fn key(self) -> String {
        match self {
            ChannelMode::Mono => "mono".into(),
            ChannelMode::Stereo => "stereo".into(),
            ChannelMode::Other(v) => format!("mode {v}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceIdentity {
    pub product_name: Option<String>,
    pub firmware: Option<String>,
    pub serial_number: Option<String>,
    pub bluetooth_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReceiverStatus {
    pub identity: DeviceIdentity,
    pub channel_mode: Option<ChannelMode>,
    pub noise_cancelling: Option<bool>,
    /// 0 is Low, 1 is High on the receiver's O / L / H switch.
    pub noise_cancelling_level: Option<i32>,
    pub gain_db: Option<i32>,
    /// Transmitter slots (1, 2) the receiver reports as linked.
    pub connected_transmitters: BTreeSet<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransmitterStatus {
    pub index: u8,
    pub identity: DeviceIdentity,
    /// The receiver's coarse battery code (bits 10-12 of the transmitter status), not a percentage.
    pub battery_level: Option<i32>,
    pub is_charging: Option<bool>,
    /// The receiver's live level meter value.
    pub meter_level: Option<i32>,
}

impl TransmitterStatus {
    pub fn new(index: u8) -> TransmitterStatus {
        TransmitterStatus {
            index,
            identity: DeviceIdentity::default(),
            battery_level: None,
            is_charging: None,
            meter_level: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MicStatus {
    pub receiver: ReceiverStatus,
    pub transmitters: BTreeMap<u8, TransmitterStatus>,
}
