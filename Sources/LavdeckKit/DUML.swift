import Foundation

/// DJI DUML v1 framing as spoken by the DJI Mic Mobile RX on USB interface 6.
///
/// ```
/// 0      0x55
/// 1-2    length (10 bits, whole frame incl. CRC16) | version 1 << 10
/// 3      CRC8 of bytes 0-2
/// 4      sender   (type | index << 5)
/// 5      receiver (type | index << 5)
/// 6-7    sequence (LE)
/// 8      command type (0x00 push, 0x40 request with ack, bit 7 = response)
/// 9      command set
/// 10     command id
/// 11..   payload
/// n-2..  CRC16 over everything before it (LE)
/// ```
public enum DUML {
    public static let magic: UInt8 = 0x55
    public static let headerLength = 11
    public static let minimumFrameLength = 13

    public static func crc8(_ bytes: some Sequence<UInt8>) -> UInt8 {
        var crc: UInt8 = 0x77
        for byte in bytes {
            crc ^= byte
            for _ in 0..<8 { crc = crc & 1 != 0 ? (crc >> 1) ^ 0x8C : crc >> 1 }
        }
        return crc
    }

    public static func crc16(_ bytes: some Sequence<UInt8>) -> UInt16 {
        var crc: UInt16 = 0x3692
        for byte in bytes {
            crc ^= UInt16(byte)
            for _ in 0..<8 { crc = crc & 1 != 0 ? (crc >> 1) ^ 0x8408 : crc >> 1 }
        }
        return crc
    }
}

/// Host addresses on the DUML bus.
public enum DUMLAddress {
    /// The Mac, acting as DJI's "app" host. The RX pushes status to this address.
    public static let app: UInt8 = 0x02
    /// The receiver (device type 0x1a, index 2; DJI host ID 0x021a).
    public static let receiver: UInt8 = 0x5a
}

public struct DUMLFrame: Equatable, Sendable {
    public var sender: UInt8
    public var receiver: UInt8
    public var sequence: UInt16
    public var commandType: UInt8
    public var commandSet: UInt8
    public var commandID: UInt8
    public var payload: [UInt8]

    public init(sender: UInt8, receiver: UInt8, sequence: UInt16, commandType: UInt8,
                commandSet: UInt8, commandID: UInt8, payload: [UInt8] = []) {
        self.sender = sender
        self.receiver = receiver
        self.sequence = sequence
        self.commandType = commandType
        self.commandSet = commandSet
        self.commandID = commandID
        self.payload = payload
    }

    public var isResponse: Bool { commandType & 0x80 != 0 }

    public func encoded() -> [UInt8] {
        let length = DUML.minimumFrameLength + payload.count
        var bytes: [UInt8] = [DUML.magic, UInt8(length & 0xff), UInt8(((length >> 8) & 0x03) | 0x04)]
        bytes.append(DUML.crc8(bytes))
        bytes += [sender, receiver, UInt8(sequence & 0xff), UInt8(sequence >> 8),
                  commandType, commandSet, commandID]
        bytes += payload
        let crc = DUML.crc16(bytes)
        bytes += [UInt8(crc & 0xff), UInt8(crc >> 8)]
        return bytes
    }

    /// Decodes exactly one frame; returns nil if either checksum fails.
    public static func decode(_ bytes: [UInt8]) -> DUMLFrame? {
        guard bytes.count >= DUML.minimumFrameLength, bytes[0] == DUML.magic,
              DUML.crc8(bytes[0..<3]) == bytes[3] else { return nil }
        let length = Int(bytes[1]) | Int(bytes[2] & 0x03) << 8
        guard length == bytes.count else { return nil }
        let crc = UInt16(bytes[length - 2]) | UInt16(bytes[length - 1]) << 8
        guard DUML.crc16(bytes[0..<(length - 2)]) == crc else { return nil }
        return DUMLFrame(sender: bytes[4], receiver: bytes[5],
                         sequence: UInt16(bytes[6]) | UInt16(bytes[7]) << 8,
                         commandType: bytes[8], commandSet: bytes[9], commandID: bytes[10],
                         payload: Array(bytes[DUML.headerLength..<(length - 2)]))
    }
}

/// Reassembles DUML frames from a byte stream that also carries unrelated bytes
/// (the RX interleaves iAP2 link-sync bytes `ff 55 02 00 ee 10`).
public struct DUMLStreamParser: Sendable {
    private var buffer: [UInt8] = []
    public private(set) var rejectedFrames = 0

    public init() {}

    public mutating func append(_ bytes: some Sequence<UInt8>) -> [DUMLFrame] {
        buffer.append(contentsOf: bytes)
        var frames: [DUMLFrame] = []
        while let start = buffer.firstIndex(of: DUML.magic) {
            if start > 0 { buffer.removeFirst(start) }
            guard buffer.count >= 4 else { break }
            let length = Int(buffer[1]) | Int(buffer[2] & 0x03) << 8
            guard DUML.crc8(buffer[0..<3]) == buffer[3], length >= DUML.minimumFrameLength else {
                buffer.removeFirst()
                continue
            }
            guard buffer.count >= length else { break }
            let candidate = Array(buffer[0..<length])
            if let frame = DUMLFrame.decode(candidate) {
                frames.append(frame)
                buffer.removeFirst(length)
            } else {
                rejectedFrames += 1
                buffer.removeFirst()
            }
        }
        if buffer.count > 4096 { buffer.removeFirst(buffer.count - 4096) }
        return frames
    }
}
