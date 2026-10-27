//! DJI DUML v1 framing as spoken by the DJI Mic Mobile RX on USB interface 6.
//!
//! ```text
//! 0      0x55
//! 1-2    length (10 bits, whole frame incl. CRC16) | version 1 << 10
//! 3      CRC8 of bytes 0-2
//! 4      sender   (type | index << 5)
//! 5      receiver (type | index << 5)
//! 6-7    sequence (LE)
//! 8      command type (0x00 push, 0x40 request with ack, bit 7 = response)
//! 9      command set
//! 10     command id
//! 11..   payload
//! n-2..  CRC16 over everything before it (LE)
//! ```

pub const MAGIC: u8 = 0x55;
pub const HEADER_LENGTH: usize = 11;
pub const MINIMUM_FRAME_LENGTH: usize = 13;

/// The Mac, acting as DJI's "app" host. The RX pushes status to this address.
pub const ADDRESS_APP: u8 = 0x02;
/// The receiver (device type 0x1a, index 2; DJI host ID 0x021a).
pub const ADDRESS_RECEIVER: u8 = 0x5a;

pub fn crc8(bytes: &[u8]) -> u8 {
    let mut crc: u8 = 0x77;
    for &byte in bytes {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x8C
            } else {
                crc >> 1
            };
        }
    }
    crc
}

pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0x3692;
    for &byte in bytes {
        crc ^= byte as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x8408
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// One DUML frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DumlFrame {
    pub sender: u8,
    pub receiver: u8,
    pub sequence: u16,
    pub command_type: u8,
    pub command_set: u8,
    pub command_id: u8,
    pub payload: Vec<u8>,
}

impl DumlFrame {
    pub fn is_response(&self) -> bool {
        self.command_type & 0x80 != 0
    }

    pub fn encoded(&self) -> Vec<u8> {
        let length = MINIMUM_FRAME_LENGTH + self.payload.len();
        let mut bytes = vec![
            MAGIC,
            (length & 0xff) as u8,
            (((length >> 8) & 0x03) | 0x04) as u8,
        ];
        bytes.push(crc8(&bytes));
        bytes.extend([
            self.sender,
            self.receiver,
            (self.sequence & 0xff) as u8,
            (self.sequence >> 8) as u8,
            self.command_type,
            self.command_set,
            self.command_id,
        ]);
        bytes.extend(&self.payload);
        let crc = crc16(&bytes);
        bytes.extend([(crc & 0xff) as u8, (crc >> 8) as u8]);
        bytes
    }

    /// Decodes exactly one frame; `None` if either checksum fails.
    pub fn decode(bytes: &[u8]) -> Option<DumlFrame> {
        if bytes.len() < MINIMUM_FRAME_LENGTH || bytes[0] != MAGIC || crc8(&bytes[..3]) != bytes[3]
        {
            return None;
        }
        let length = bytes[1] as usize | ((bytes[2] & 0x03) as usize) << 8;
        if length != bytes.len() {
            return None;
        }
        let crc = bytes[length - 2] as u16 | (bytes[length - 1] as u16) << 8;
        if crc16(&bytes[..length - 2]) != crc {
            return None;
        }
        Some(DumlFrame {
            sender: bytes[4],
            receiver: bytes[5],
            sequence: bytes[6] as u16 | (bytes[7] as u16) << 8,
            command_type: bytes[8],
            command_set: bytes[9],
            command_id: bytes[10],
            payload: bytes[HEADER_LENGTH..length - 2].to_vec(),
        })
    }
}

/// Reassembles DUML frames from a byte stream that also carries unrelated bytes
/// (the RX interleaves iAP2 link-sync bytes `ff 55 02 00 ee 10`).
#[derive(Debug, Default)]
pub struct DumlStreamParser {
    buffer: Vec<u8>,
    rejected_frames: usize,
}

impl DumlStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames whose header checked out but whose CRC16 did not.
    pub fn rejected_frames(&self) -> usize {
        self.rejected_frames
    }

    pub fn append(&mut self, bytes: &[u8]) -> Vec<DumlFrame> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some(start) = self.buffer.iter().position(|&b| b == MAGIC) {
            if start > 0 {
                self.buffer.drain(..start);
            }
            if self.buffer.len() < 4 {
                break;
            }
            let length = self.buffer[1] as usize | ((self.buffer[2] & 0x03) as usize) << 8;
            if crc8(&self.buffer[..3]) != self.buffer[3] || length < MINIMUM_FRAME_LENGTH {
                self.buffer.remove(0);
                continue;
            }
            if self.buffer.len() < length {
                break;
            }
            if let Some(frame) = DumlFrame::decode(&self.buffer[..length]) {
                frames.push(frame);
                self.buffer.drain(..length);
            } else {
                self.rejected_frames += 1;
                self.buffer.remove(0);
            }
        }
        if self.buffer.len() > 4096 {
            let excess = self.buffer.len() - 4096;
            self.buffer.drain(..excess);
        }
        frames
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn bytes(hex: &str) -> Vec<u8> {
        hex.split(' ')
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    // Frames captured from the Mobile RX on Oct 11 2026.
    pub fn tx_level_push() -> Vec<u8> {
        bytes(
            "55 1e 04 8a 5a 02 00 00 00 5b 03 03 0e 00 05 01 00 00 00 01 22 05 02 00 00 00 01 22 76 29",
        )
    }
    pub fn rx_tx_status_push() -> Vec<u8> {
        bytes(
            "55 76 04 a6 5a 02 00 00 00 5b 03 03 66 00 03 00 00 00 00 20 20 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
             00 00 00 00 00 00 00 00 03 00 00 00 3e 00 00 00 02 01 00 00 00 1a 90 25 00 00 00 00 00 00 00 00 00 00 00 00 \
             00 00 00 00 00 00 00 00 00 00 00 00 02 02 00 00 00 1a 90 25 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
             00 00 00 00 00 00 00 00 f9 46",
        )
    }
    pub fn identity_push() -> Vec<u8> {
        bytes(
            "55 b4 04 83 5a 02 00 00 00 5b 03 03 a4 00 01 00 00 00 00 12 00 0b 03 02 30 30 30 30 52 58 30 30 30 30 30 30 \
             30 31 04 00 00 00 00 06 30 61 30 62 30 63 06 00 00 00 00 0e 44 4a 49 20 4d 69 63 20 4d 69 6e 69 20 32 01 01 00 \
             00 00 12 00 11 03 02 30 30 30 30 54 58 30 30 30 30 30 30 30 31 04 01 00 00 00 06 61 31 62 32 63 33 06 01 00 00 \
             00 0c 44 4a 49 20 4d 69 63 20 4d 69 6e 69 01 02 00 00 00 12 38 00 01 01 30 30 30 30 54 58 30 30 30 30 30 30 30 \
             32 04 02 00 00 00 06 64 34 65 35 66 36 06 02 00 00 00 0c 44 4a 49 20 4d 69 63 20 4d 69 6e 69 b8 fe",
        )
    }

    #[test]
    fn decodes_captured_frames() {
        for raw in [tx_level_push(), rx_tx_status_push(), identity_push()] {
            let frame = DumlFrame::decode(&raw).expect("frame");
            assert_eq!(frame.sender, ADDRESS_RECEIVER);
            assert_eq!(frame.receiver, ADDRESS_APP);
            assert_eq!(frame.encoded(), raw);
        }
    }

    #[test]
    fn rejects_corrupt_frames() {
        let mut corrupt = tx_level_push();
        corrupt[20] ^= 0xff;
        assert!(DumlFrame::decode(&corrupt).is_none());
    }

    #[test]
    fn stream_parser_skips_iap2_noise_and_splits_frames() {
        let mut parser = DumlStreamParser::new();
        let mut stream = bytes("ff 55 02 00 ee 10 ff 55 02 00 ee 10");
        stream.extend(tx_level_push());
        stream.extend(rx_tx_status_push());
        let first = parser.append(&stream[..40]);
        let rest = parser.append(&stream[40..]);
        assert_eq!(first.len() + rest.len(), 2);
    }

    #[test]
    fn stream_parser_counts_bad_crc16_and_keeps_going() {
        let mut parser = DumlStreamParser::new();
        let mut corrupt = tx_level_push();
        corrupt[20] ^= 0xff;
        let mut stream = corrupt;
        stream.extend(identity_push());
        let frames = parser.append(&stream);
        assert_eq!(frames.len(), 1);
        assert_eq!(parser.rejected_frames(), 1);
    }
}
