//! The receiver's live status model and the decoder for the status it pushes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::SystemTime;

use crate::duml::DumlFrame;

/// Receiver channel mode as reported in status and accepted by `set_wireless_mic_params`.
/// Verified on the Mobile RX: 0 = mono, 2 = stereo. Value 1 is rejected by that receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChannelMode {
    Mono,
    Stereo,
    Other(u8),
}

impl ChannelMode {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Mono,
            2 => Self::Stereo,
            other => Self::Other(other),
        }
    }

    pub fn raw(self) -> u8 {
        match self {
            Self::Mono => 0,
            Self::Stereo => 2,
            Self::Other(value) => value,
        }
    }
}

/// `mono`, `stereo` or `mode N`: the words stored in `.take.json` and printed by the CLI.
impl fmt::Display for ChannelMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mono => f.write_str("mono"),
            Self::Stereo => f.write_str("stereo"),
            Self::Other(value) => write!(f, "mode {value}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub product_name: Option<String>,
    pub firmware: Option<String>,
    pub serial_number: Option<String>,
    pub bluetooth_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceiverStatus {
    pub identity: DeviceIdentity,
    pub channel_mode: Option<ChannelMode>,
    pub noise_cancelling: Option<bool>,
    pub noise_cancelling_level: Option<i32>,
    pub auto_power_off: Option<bool>,
    pub attached_over_usb: Option<bool>,
    pub gain_db: Option<i32>,
    /// DJI's 0-7 battery level (mapping to percent is unconfirmed).
    pub battery_level: Option<i32>,
    /// TX indexes (1, 2) the receiver reports as connected.
    pub connected_transmitters: BTreeSet<i32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransmitterStatus {
    pub index: i32,
    pub identity: DeviceIdentity,
    /// DJI's 0-7 battery level (mapping to percent is unconfirmed).
    pub battery_level: Option<i32>,
    pub is_charging: Option<bool>,
    pub noise_cancelling: Option<bool>,
    pub gain_db: Option<i32>,
    pub signal_level: Option<i32>,
    /// Live level meter value reported by the RX (record type 0x05).
    pub meter_level: Option<i32>,
    pub raw_flags: Option<u32>,
}

impl TransmitterStatus {
    pub fn new(index: i32) -> Self {
        Self {
            index,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicStatus {
    pub receiver: ReceiverStatus,
    pub transmitters: BTreeMap<i32, TransmitterStatus>,
    pub last_push: Option<SystemTime>,
}

impl MicStatus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Transmitters the receiver currently reports as connected, in index order.
    pub fn connected_transmitters(&self) -> Vec<TransmitterStatus> {
        self.receiver
            .connected_transmitters
            .iter()
            .map(|&i| {
                self.transmitters
                    .get(&i)
                    .cloned()
                    .unwrap_or_else(|| TransmitterStatus::new(i))
            })
            .collect()
    }

    fn transmitter(&mut self, index: i32) -> &mut TransmitterStatus {
        self.transmitters
            .entry(index)
            .or_insert_with(|| TransmitterStatus::new(index))
    }
}

/// Decodes the RX's unsolicited `0x5B/0x03` version-3 status pushes.
///
/// Payload: `03, total length (u16 LE), records…`; record: `type (1), index (u32 LE, 0 = RX,
/// 1/2 = TX), length (1), value`. Offsets below are relative to the record start `r`, matching
/// DJI's handlers; the value starts at `r + 6`.
pub mod decoder {
    use super::*;

    pub const STATUS_COMMAND_SET: u8 = 0x5B;
    pub const STATUS_COMMAND_ID: u8 = 0x03;

    /// Applies one frame to `status`; false if it isn't a status push.
    pub fn apply(frame: &DumlFrame, status: &mut MicStatus) -> bool {
        apply_at(frame, status, SystemTime::now())
    }

    pub fn apply_at(frame: &DumlFrame, status: &mut MicStatus, date: SystemTime) -> bool {
        if frame.command_set != STATUS_COMMAND_SET
            || frame.command_id != STATUS_COMMAND_ID
            || frame.is_response()
        {
            return false;
        }
        let p = &frame.payload;
        if p.len() < 3 || p[0] != 3 {
            return false;
        }
        let declared = p[1] as usize | (p[2] as usize) << 8;
        let total = p.len().min(declared + 3);
        let mut r = 3;
        while r + 6 <= total {
            let kind = p[r];
            let index = u32_le(p, r + 1) as i32;
            let length = p[r + 5] as usize;
            let end = p.len().min(r + 6 + length);
            let record = &p[r..end];
            apply_record(kind, index, record, &record[6.min(record.len())..], status);
            r += 6 + length;
        }
        status.last_push = Some(date);
        true
    }

    fn apply_record(kind: u8, index: i32, record: &[u8], value: &[u8], status: &mut MicStatus) {
        match kind {
            0x01 => {
                if value.len() < 4 {
                    return;
                }
                let firmware = value[..4]
                    .iter()
                    .rev()
                    .map(|b| format!("{b:02}"))
                    .collect::<Vec<_>>()
                    .join(".");
                let id = identity_mut(status, index);
                id.firmware = Some(firmware);
                id.serial_number = Some(ascii(&value[4..]));
            }
            0x04 => {
                identity_mut(status, index).bluetooth_id = Some(ascii(value));
            }
            0x06 => {
                identity_mut(status, index).product_name = Some(ascii(value));
            }
            0x05 => {
                if index > 0
                    && let Some(&level) = value.first()
                {
                    status.transmitter(index).meter_level = Some(level as i32);
                }
            }
            0x02 => {
                if index <= 0 || record.len() < 10 {
                    return;
                }
                let w = u32_le(record, 6);
                let tx = status.transmitter(index);
                tx.raw_flags = Some(w);
                tx.battery_level = Some((w >> 10 & 7) as i32);
                tx.is_charging = Some(w >> 9 & 1 == 1);
                tx.noise_cancelling = Some(w >> 8 & 1 == 1);
                if record.len() > 0x0c {
                    tx.signal_level = Some((record[0x0c] >> 4) as i32);
                }
                if record.len() > 0x0d {
                    tx.gain_db = Some(record[0x0d] as i8 as i32);
                }
            }
            0x03 => {
                if record.len() < 8 {
                    return;
                }
                let h = record[6] as u16 | (record[7] as u16) << 8;
                let rx = &mut status.receiver;
                rx.noise_cancelling = Some(h >> 5 & 1 == 1);
                rx.noise_cancelling_level = Some((h >> 3 & 3) as i32);
                rx.auto_power_off = Some(h >> 8 & 1 == 1);
                rx.channel_mode = Some(ChannelMode::from_raw((h >> 9 & 7) as u8));
                rx.battery_level = Some((h >> 13) as i32);
                if record.len() > 0x08 {
                    rx.gain_db = Some(record[0x08] as i8 as i32);
                }
                if record.len() > 0x1e {
                    let mask = record[0x1e];
                    rx.connected_transmitters = (0..8)
                        .filter(|bit| mask >> bit & 1 == 1)
                        .map(|bit| bit + 1)
                        .collect();
                }
                if record.len() > 0x22 {
                    rx.attached_over_usb = Some(record[0x22] >> 3 & 1 == 1);
                }
            }
            _ => {}
        }
    }

    fn identity_mut(status: &mut MicStatus, index: i32) -> &mut DeviceIdentity {
        if index == 0 {
            &mut status.receiver.identity
        } else {
            &mut status.transmitter(index).identity
        }
    }

    /// Little-endian u32 at `i`; bytes past the end read as zero.
    pub(crate) fn u32_le(b: &[u8], i: usize) -> u32 {
        (0..4).fold(0, |acc, k| {
            if i + k < b.len() {
                acc | (b[i + k] as u32) << (8 * k)
            } else {
                acc
            }
        })
    }

    /// Printable ASCII only, as the receiver pads its strings.
    pub(crate) fn ascii(bytes: &[u8]) -> String {
        bytes
            .iter()
            .filter(|&&b| (0x20..0x7f).contains(&b))
            .map(|&b| b as char)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duml::tests::{identity_push, rx_tx_status_push, tx_level_push};

    #[test]
    fn decodes_status_model() {
        let mut status = MicStatus::new();
        for raw in [identity_push(), rx_tx_status_push(), tx_level_push()] {
            assert!(decoder::apply(
                &DumlFrame::decode(&raw).unwrap(),
                &mut status
            ));
        }
        let rx = &status.receiver;
        assert_eq!(rx.identity.product_name.as_deref(), Some("DJI Mic Mini 2"));
        assert_eq!(rx.identity.firmware.as_deref(), Some("02.03.11.00"));
        assert_eq!(rx.channel_mode, Some(ChannelMode::Mono));
        assert_eq!(rx.noise_cancelling, Some(true));
        assert_eq!(rx.auto_power_off, Some(true));
        assert_eq!(rx.gain_db, Some(0));
        assert_eq!(rx.connected_transmitters, BTreeSet::from([1, 2]));
        assert_eq!(rx.attached_over_usb, Some(true));
        let tx1 = &status.transmitters[&1];
        assert_eq!(tx1.identity.bluetooth_id.as_deref(), Some("a1b2c3"));
        assert_eq!(tx1.identity.firmware.as_deref(), Some("02.03.17.00"));
        assert_eq!(tx1.battery_level, Some(1));
        assert_eq!(tx1.is_charging, Some(false));
        assert_eq!(tx1.meter_level, Some(0x22));
        assert_eq!(
            status.transmitters[&2].identity.firmware.as_deref(),
            Some("01.01.00.56")
        );
        assert!(status.last_push.is_some());
    }

    #[test]
    fn ignores_responses_and_other_commands() {
        let mut frame = DumlFrame::decode(&tx_level_push()).unwrap();
        frame.command_type |= 0x80;
        let mut status = MicStatus::new();
        assert!(!decoder::apply(&frame, &mut status));
        assert_eq!(status, MicStatus::new());
    }

    #[test]
    fn channel_mode_words() {
        assert_eq!(ChannelMode::Mono.to_string(), "mono");
        assert_eq!(ChannelMode::Stereo.to_string(), "stereo");
        assert_eq!(ChannelMode::from_raw(1).to_string(), "mode 1");
    }
}
