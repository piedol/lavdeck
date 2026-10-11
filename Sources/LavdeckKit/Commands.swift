import Foundation

/// The only requests this library can send to the receiver. Each one was traced in DJI's
/// code and verified on hardware (see research/protocol.md). There is intentionally no API
/// for sending arbitrary frames: firmware-upgrade and reboot commands must stay unreachable.
public enum MicCommand: Equatable, Sendable {
    /// General `0x00/0x01` get version (read-only).
    case getVersion
    /// `0x5B/0x12` get wireless-mic capability (read-only). `id` 0 = all, 5 = channel modes, 6 = gain.
    case getCapability(id: UInt16)
    /// `0x5B/0x01` set RX channel mode (V2 format, index 0). Verified: mono and stereo.
    case setChannel(ChannelMode)
    /// `0x5B/0x01` set RX gain in dB (V2 format, TLV 0x0039, index 0). Range from capabilities.
    case setReceiverGain(dB: Int)

    public var isWrite: Bool {
        switch self {
        case .getVersion, .getCapability: false
        case .setChannel, .setReceiverGain: true
        }
    }

    public var commandSet: UInt8 {
        switch self {
        case .getVersion: 0x00
        case .getCapability, .setChannel, .setReceiverGain: 0x5B
        }
    }

    public var commandID: UInt8 {
        switch self {
        case .getVersion: 0x01
        case .getCapability: 0x12
        case .setChannel, .setReceiverGain: 0x01
        }
    }

    /// Payload bytes, or nil if the arguments are outside the verified allowlist.
    public var payload: [UInt8]? {
        switch self {
        case .getVersion:
            return []
        case .getCapability(let id):
            return [0, 0, 0, 0, UInt8(id & 0xff), UInt8(id >> 8)]
        case .setChannel(let mode):
            guard mode == .mono || mode == .stereo else { return nil }
            return Self.tlvSet(id: 0x0008, value: mode.raw)
        case .setReceiverGain(let dB):
            guard (-12...12).contains(dB) else { return nil }
            return Self.tlvSet(id: 0x0039, value: UInt8(bitPattern: Int8(dB)))
        }
    }

    /// `set_wireless_mic_params` V2 body: `02 | index u32 LE (0 = RX) | TLV id u16 LE | 01 | value`.
    /// The older V1 body (`00 00 …`) makes the Mobile RX drop off USB and must not be used.
    static func tlvSet(id: UInt16, value: UInt8) -> [UInt8] {
        [0x02, 0x00, 0x00, 0x00, 0x00, UInt8(id & 0xff), UInt8(id >> 8), 0x01, value]
    }

    func frame(sequence: UInt16) -> DUMLFrame? {
        guard let payload else { return nil }
        return DUMLFrame(sender: DUMLAddress.app, receiver: DUMLAddress.receiver, sequence: sequence,
                         commandType: 0x40, commandSet: commandSet, commandID: commandID, payload: payload)
    }
}

public struct MicCapability: Equatable, Sendable {
    public var id: UInt16
    public var value: [UInt8]

    /// For range capabilities (e.g. gain): min, max, step as signed 32-bit values.
    public var range: ClosedRange<Int>? {
        guard value.count >= 8 else { return nil }
        let lo = Int(Int32(bitPattern: MicStatusDecoder.u32(value, 0)))
        let hi = Int(Int32(bitPattern: MicStatusDecoder.u32(value, 4)))
        return lo <= hi ? lo...hi : nil
    }

    public var step: Int? {
        value.count >= 12 ? Int(Int32(bitPattern: MicStatusDecoder.u32(value, 8))) : nil
    }
}

/// Capabilities per device index (0 = RX, 1/2 = TX).
public struct MicCapabilities: Equatable, Sendable {
    public var devices: [Int: [UInt16: MicCapability]] = [:]

    public static let channelModesID: UInt16 = 0x05
    public static let gainID: UInt16 = 0x06

    public var receiverGainRange: ClosedRange<Int>? { devices[0]?[Self.gainID]?.range }
    public var receiverGainStep: Int? { devices[0]?[Self.gainID]?.step }
    public var receiverChannelModeMask: UInt8? { devices[0]?[Self.channelModesID]?.value.first }

    /// Response layout: `total u16 | (index u32 | len u16 | items)…`; item: `id u16 | len u8 | value`.
    public init?(payload p: [UInt8]) {
        guard p.count >= 2 else { return nil }
        var i = 2
        while i + 6 <= p.count {
            let index = Int(MicStatusDecoder.u32(p, i))
            let length = Int(p[i + 4]) | Int(p[i + 5]) << 8
            var j = i + 6
            let end = min(p.count, j + length)
            var items: [UInt16: MicCapability] = [:]
            while j + 3 <= end {
                let id = UInt16(p[j]) | UInt16(p[j + 1]) << 8
                let itemLength = Int(p[j + 2])
                let valueEnd = min(end, j + 3 + itemLength)
                items[id] = MicCapability(id: id, value: Array(p[(j + 3)..<valueEnd]))
                j = j + 3 + itemLength
            }
            devices[index] = items
            i = end
        }
    }
}
