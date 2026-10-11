import Foundation

/// Receiver channel mode as reported in status and accepted by `set_wireless_mic_params`.
/// Verified on the Mobile RX: 0 = mono, 2 = stereo. Value 1 is rejected by that receiver.
public enum ChannelMode: Equatable, Sendable, CustomStringConvertible {
    case mono
    case stereo
    case other(UInt8)

    public init(raw: UInt8) {
        switch raw {
        case 0: self = .mono
        case 2: self = .stereo
        default: self = .other(raw)
        }
    }

    public var raw: UInt8 {
        switch self {
        case .mono: 0
        case .stereo: 2
        case .other(let value): value
        }
    }

    public var description: String {
        switch self {
        case .mono: "mono"
        case .stereo: "stereo"
        case .other(let value): "mode \(value)"
        }
    }
}

public struct DeviceIdentity: Equatable, Sendable {
    public var productName: String?
    public var firmware: String?
    public var serialNumber: String?
    public var bluetoothID: String?
}

public struct ReceiverStatus: Equatable, Sendable {
    public var identity = DeviceIdentity()
    public var channelMode: ChannelMode?
    public var noiseCancelling: Bool?
    public var noiseCancellingLevel: Int?
    public var autoPowerOff: Bool?
    public var attachedOverUSB: Bool?
    public var gainDB: Int?
    /// DJI's 0-7 battery level (mapping to percent is unconfirmed).
    public var batteryLevel: Int?
    /// TX indexes (1, 2) the receiver reports as connected.
    public var connectedTransmitters: Set<Int> = []
}

public struct TransmitterStatus: Equatable, Sendable {
    public var index: Int
    public var identity = DeviceIdentity()
    /// DJI's 0-7 battery level (mapping to percent is unconfirmed).
    public var batteryLevel: Int?
    public var isCharging: Bool?
    public var noiseCancelling: Bool?
    public var gainDB: Int?
    public var signalLevel: Int?
    /// Live level meter value reported by the RX (record type 0x05).
    public var meterLevel: Int?
    public var rawFlags: UInt32?

    public init(index: Int) { self.index = index }
}

public struct MicStatus: Equatable, Sendable {
    public var receiver = ReceiverStatus()
    public var transmitters: [Int: TransmitterStatus] = [:]
    public var lastPush: Date?

    public init() {}

    /// Transmitters the receiver currently reports as connected, in index order.
    public var connectedTransmitters: [TransmitterStatus] {
        receiver.connectedTransmitters.sorted().map { transmitters[$0] ?? TransmitterStatus(index: $0) }
    }
}

/// Decodes the RX's unsolicited `0x5B/0x03` version-3 status pushes.
///
/// Payload: `03, total length (u16 LE), records…`; record: `type (1), index (u32 LE, 0 = RX,
/// 1/2 = TX), length (1), value`. Offsets below are relative to the record start `r`, matching
/// DJI's handlers; the value starts at `r + 6`.
public enum MicStatusDecoder {
    public static let statusCommandSet: UInt8 = 0x5B
    public static let statusCommandID: UInt8 = 0x03

    @discardableResult
    public static func apply(_ frame: DUMLFrame, to status: inout MicStatus, at date: Date = Date()) -> Bool {
        guard frame.commandSet == statusCommandSet, frame.commandID == statusCommandID,
              !frame.isResponse else { return false }
        let p = frame.payload
        guard p.count >= 3, p[0] == 3 else { return false }
        let declared = Int(p[1]) | (Int(p[2]) << 8)
        let total = min(p.count, declared + 3)
        var r = 3
        while r + 6 <= total {
            let type = p[r]
            let index = Int(u32(p, r + 1))
            let length = Int(p[r + 5])
            let end = min(p.count, r + 6 + length)
            let record = Array(p[r..<end])
            apply(type: type, index: index, record: record, value: Array(record.dropFirst(6)), to: &status)
            r += 6 + length
        }
        status.lastPush = date
        return true
    }

    private static func apply(type: UInt8, index: Int, record: [UInt8], value: [UInt8], to status: inout MicStatus) {
        func updateIdentity(_ update: (inout DeviceIdentity) -> Void) {
            if index == 0 {
                update(&status.receiver.identity)
            } else {
                update(&status.transmitters[index, default: TransmitterStatus(index: index)].identity)
            }
        }
        switch type {
        case 0x01:
            guard value.count >= 4 else { return }
            let firmware = value.prefix(4).reversed().map { String(format: "%02d", $0) }.joined(separator: ".")
            let serial = ascii(value.dropFirst(4))
            updateIdentity { $0.firmware = firmware; $0.serialNumber = serial }
        case 0x04:
            let id = ascii(value)
            updateIdentity { $0.bluetoothID = id }
        case 0x06:
            let name = ascii(value)
            updateIdentity { $0.productName = name }
        case 0x05:
            guard index > 0, let level = value.first else { return }
            status.transmitters[index, default: TransmitterStatus(index: index)].meterLevel = Int(level)
        case 0x02:
            guard index > 0, record.count >= 10 else { return }
            let w = u32(record, 6)
            var tx = status.transmitters[index, default: TransmitterStatus(index: index)]
            tx.rawFlags = w
            tx.batteryLevel = Int(w >> 10 & 7)
            tx.isCharging = w >> 9 & 1 == 1
            tx.noiseCancelling = w >> 8 & 1 == 1
            if record.count > 0x0c { tx.signalLevel = Int(record[0x0c] >> 4) }
            if record.count > 0x0d { tx.gainDB = Int(Int8(bitPattern: record[0x0d])) }
            status.transmitters[index] = tx
        case 0x03:
            guard record.count >= 8 else { return }
            let h = UInt16(record[6]) | UInt16(record[7]) << 8
            var rx = status.receiver
            rx.noiseCancelling = h >> 5 & 1 == 1
            rx.noiseCancellingLevel = Int(h >> 3 & 3)
            rx.autoPowerOff = h >> 8 & 1 == 1
            rx.channelMode = ChannelMode(raw: UInt8(h >> 9 & 7))
            rx.batteryLevel = Int(h >> 13)
            if record.count > 0x08 { rx.gainDB = Int(Int8(bitPattern: record[0x08])) }
            if record.count > 0x1e {
                let mask = record[0x1e]
                rx.connectedTransmitters = Set((0..<8).filter { mask >> $0 & 1 == 1 }.map { $0 + 1 })
            }
            if record.count > 0x22 { rx.attachedOverUSB = record[0x22] >> 3 & 1 == 1 }
            status.receiver = rx
        default:
            break
        }
    }

    static func u32(_ b: [UInt8], _ i: Int) -> UInt32 {
        (0..<4).reduce(UInt32(0)) { acc, k in i + k < b.count ? acc | UInt32(b[i + k]) << (8 * k) : acc }
    }

    static func ascii(_ bytes: some Sequence<UInt8>) -> String {
        String(decoding: bytes.filter { $0 >= 0x20 && $0 < 0x7f }, as: UTF8.self)
    }
}
