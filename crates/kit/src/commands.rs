//! The only requests this library can send to the receiver, and the capability reply.
//!
//! Each command was traced in DJI's code and verified on hardware (see `docs/PROTOCOL.md`). There
//! is intentionally no API for sending arbitrary frames: firmware-upgrade and reboot commands must
//! stay unreachable.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use crate::duml::{ADDRESS_APP, ADDRESS_RECEIVER, DumlFrame};
use crate::status::ChannelMode;
use crate::status::decoder::u32_le;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicCommand {
    /// General `0x00/0x01` get version (read-only).
    GetVersion,
    /// `0x5B/0x12` get wireless-mic capability (read-only). `id` 0 = all, 5 = channel modes, 6 = gain.
    GetCapability { id: u16 },
    /// `0x5B/0x01` set RX channel mode (V2 format, index 0). Verified: mono and stereo.
    SetChannel(ChannelMode),
    /// `0x5B/0x01` set RX gain in dB (V2 format, TLV 0x0039, index 0). Range from capabilities.
    SetReceiverGain { db: i32 },
}

impl MicCommand {
    pub fn is_write(&self) -> bool {
        matches!(self, Self::SetChannel(_) | Self::SetReceiverGain { .. })
    }

    pub fn command_set(&self) -> u8 {
        match self {
            Self::GetVersion => 0x00,
            Self::GetCapability { .. } | Self::SetChannel(_) | Self::SetReceiverGain { .. } => 0x5B,
        }
    }

    pub fn command_id(&self) -> u8 {
        match self {
            Self::GetVersion => 0x01,
            Self::GetCapability { .. } => 0x12,
            Self::SetChannel(_) | Self::SetReceiverGain { .. } => 0x01,
        }
    }

    /// Payload bytes, or `None` if the arguments are outside the verified allowlist.
    pub fn payload(&self) -> Option<Vec<u8>> {
        match *self {
            Self::GetVersion => Some(Vec::new()),
            Self::GetCapability { id } => {
                Some(vec![0, 0, 0, 0, (id & 0xff) as u8, (id >> 8) as u8])
            }
            Self::SetChannel(mode) => matches!(mode, ChannelMode::Mono | ChannelMode::Stereo)
                .then(|| tlv_set(0x0008, mode.raw())),
            Self::SetReceiverGain { db } => (-12..=12)
                .contains(&db)
                .then(|| tlv_set(0x0039, db as i8 as u8)),
        }
    }

    /// The request frame, or `None` outside the allowlist.
    pub(crate) fn frame(&self, sequence: u16) -> Option<DumlFrame> {
        Some(DumlFrame {
            sender: ADDRESS_APP,
            receiver: ADDRESS_RECEIVER,
            sequence,
            command_type: 0x40,
            command_set: self.command_set(),
            command_id: self.command_id(),
            payload: self.payload()?,
        })
    }
}

/// `set_wireless_mic_params` V2 body: `02 | index u32 LE (0 = RX) | TLV id u16 LE | 01 | value`.
/// The older V1 body (`00 00 …`) makes the Mobile RX drop off USB and must not be used.
fn tlv_set(id: u16, value: u8) -> Vec<u8> {
    vec![
        0x02,
        0x00,
        0x00,
        0x00,
        0x00,
        (id & 0xff) as u8,
        (id >> 8) as u8,
        0x01,
        value,
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicCapability {
    pub id: u16,
    pub value: Vec<u8>,
}

impl MicCapability {
    /// For range capabilities (e.g. gain): min, max as signed 32-bit values.
    pub fn range(&self) -> Option<RangeInclusive<i32>> {
        if self.value.len() < 8 {
            return None;
        }
        let lo = u32_le(&self.value, 0) as i32;
        let hi = u32_le(&self.value, 4) as i32;
        (lo <= hi).then_some(lo..=hi)
    }

    pub fn step(&self) -> Option<i32> {
        (self.value.len() >= 12).then(|| u32_le(&self.value, 8) as i32)
    }
}

/// Capabilities per device index (0 = RX, 1/2 = TX).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicCapabilities {
    pub devices: BTreeMap<i64, BTreeMap<u16, MicCapability>>,
}

impl MicCapabilities {
    pub const CHANNEL_MODES_ID: u16 = 0x05;
    pub const GAIN_ID: u16 = 0x06;

    /// Response layout: `total u16 | (index u32 | len u16 | items)…`; item: `id u16 | len u8 | value`.
    pub fn parse(p: &[u8]) -> Option<Self> {
        if p.len() < 2 {
            return None;
        }
        let mut caps = Self::default();
        let mut i = 2;
        while i + 6 <= p.len() {
            let index = u32_le(p, i) as i64;
            let length = p[i + 4] as usize | (p[i + 5] as usize) << 8;
            let mut j = i + 6;
            let end = p.len().min(j + length);
            let mut items = BTreeMap::new();
            while j + 3 <= end {
                let id = p[j] as u16 | (p[j + 1] as u16) << 8;
                let item_length = p[j + 2] as usize;
                let value_end = end.min(j + 3 + item_length);
                items.insert(
                    id,
                    MicCapability {
                        id,
                        value: p[j + 3..value_end].to_vec(),
                    },
                );
                j += 3 + item_length;
            }
            caps.devices.insert(index, items);
            i = end;
        }
        Some(caps)
    }

    fn receiver(&self, id: u16) -> Option<&MicCapability> {
        self.devices.get(&0)?.get(&id)
    }

    pub fn receiver_gain_range(&self) -> Option<RangeInclusive<i32>> {
        self.receiver(Self::GAIN_ID)?.range()
    }

    pub fn receiver_gain_step(&self) -> Option<i32> {
        self.receiver(Self::GAIN_ID)?.step()
    }

    pub fn receiver_channel_mode_mask(&self) -> Option<u8> {
        self.receiver(Self::CHANNEL_MODES_ID)?
            .value
            .first()
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duml::tests::bytes;

    #[test]
    fn encodes_verified_stereo_and_mono_commands() {
        // Sent to the hardware and acknowledged with result 0x00.
        let stereo = MicCommand::SetChannel(ChannelMode::Stereo)
            .frame(0xf936)
            .unwrap()
            .encoded();
        assert_eq!(
            stereo,
            bytes("55 16 04 fc 02 5a 36 f9 40 5b 01 02 00 00 00 00 08 00 01 02 58 2b")
        );
        let mono = MicCommand::SetChannel(ChannelMode::Mono)
            .frame(0x8a17)
            .unwrap()
            .encoded();
        assert_eq!(
            mono,
            bytes("55 16 04 fc 02 5a 17 8a 40 5b 01 02 00 00 00 00 08 00 01 00 c9 51")
        );
    }

    #[test]
    fn allowlist_refuses_unverified_values() {
        assert_eq!(
            MicCommand::SetChannel(ChannelMode::Other(1)).payload(),
            None
        );
        assert_eq!(MicCommand::SetReceiverGain { db: 30 }.payload(), None);
        assert_eq!(MicCommand::SetReceiverGain { db: 13 }.frame(1), None);
        assert_eq!(
            MicCommand::SetReceiverGain { db: -6 }.payload(),
            Some(vec![0x02, 0, 0, 0, 0, 0x39, 0x00, 0x01, 0xfa])
        );
    }

    #[test]
    fn every_command_matches_the_swift_encoder() {
        // Printed by Sources/LavdeckKit's MicCommand + DUMLFrame for the same sequence numbers.
        for (command, sequence, expected) in [
            (
                MicCommand::GetVersion,
                0x1234,
                "55 0d 04 33 02 5a 34 12 40 00 01 df 8e",
            ),
            (
                MicCommand::GetCapability { id: 0 },
                0x1235,
                "55 13 04 03 02 5a 35 12 40 5b 12 00 00 00 00 00 00 03 fc",
            ),
            (
                MicCommand::GetCapability { id: 6 },
                0x7fff,
                "55 13 04 03 02 5a ff 7f 40 5b 12 00 00 00 00 06 00 19 e8",
            ),
            (
                MicCommand::SetReceiverGain { db: 6 },
                0x0100,
                "55 16 04 fc 02 5a 00 01 40 5b 01 02 00 00 00 00 39 00 01 06 db 27",
            ),
            (
                MicCommand::SetReceiverGain { db: -12 },
                0x0101,
                "55 16 04 fc 02 5a 01 01 40 5b 01 02 00 00 00 00 39 00 01 f4 ac 8d",
            ),
            (
                MicCommand::SetReceiverGain { db: 12 },
                0x0102,
                "55 16 04 fc 02 5a 02 01 40 5b 01 02 00 00 00 00 39 00 01 0c 55 75",
            ),
        ] {
            assert_eq!(
                command.frame(sequence).unwrap().encoded(),
                bytes(expected),
                "{command:?}"
            );
        }
        assert!(
            !MicCommand::GetVersion.is_write()
                && MicCommand::SetChannel(ChannelMode::Mono).is_write()
        );
    }

    #[test]
    fn parses_capabilities() {
        let payload = bytes(
            "71 00 00 00 00 00 31 00 0d 00 01 01 06 00 0c f4 ff ff ff 0c 00 00 00 06 00 00 00 09 00 0c f4 ff ff ff 0c 00 \
             00 00 01 00 00 00 11 00 08 c8 00 00 00 00 00 00 00 05 00 01 07 01 00 00 00 17 00 02 00 08 42 49 0b 00 00 00 00 \
             00 03 00 01 03 04 00 01 05 12 00 01 01 02 00 00 00 17 00 02 00 08 42 49 0b 00 00 00 00 00 03 00 01 03 04 00 01 \
             05 12 00 01 01",
        );
        let caps = MicCapabilities::parse(&payload).unwrap();
        assert_eq!(caps.receiver_gain_range(), Some(-12..=12));
        assert_eq!(caps.receiver_gain_step(), Some(6));
        assert_eq!(caps.receiver_channel_mode_mask(), Some(0x07));
        assert_eq!(
            caps.devices[&1].keys().copied().collect::<Vec<_>>(),
            vec![0x02, 0x03, 0x04, 0x12]
        );
    }
}
