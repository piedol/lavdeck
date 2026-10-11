import Foundation
import Testing
@testable import LavdeckKit

private func bytes(_ hex: String) -> [UInt8] {
    hex.split(separator: " ").map { UInt8($0, radix: 16)! }
}

// Frames captured from the Mobile RX on Oct 11 2026.
private let txLevelPush = bytes("55 1e 04 8a 5a 02 00 00 00 5b 03 03 0e 00 05 01 00 00 00 01 22 05 02 00 00 00 01 22 76 29")
private let rxTxStatusPush = bytes("55 76 04 a6 5a 02 00 00 00 5b 03 03 66 00 03 00 00 00 00 20 20 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 03 00 00 00 3e 00 00 00 02 01 00 00 00 1a 90 25 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 02 02 00 00 00 1a 90 25 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 f9 46")
private let identityPush = bytes("55 b4 04 83 5a 02 00 00 00 5b 03 03 a4 00 01 00 00 00 00 12 00 0b 03 02 30 30 30 30 52 58 30 30 30 30 30 30 30 31 04 00 00 00 00 06 30 61 30 62 30 63 06 00 00 00 00 0e 44 4a 49 20 4d 69 63 20 4d 69 6e 69 20 32 01 01 00 00 00 12 00 11 03 02 30 30 30 30 54 58 30 30 30 30 30 30 30 31 04 01 00 00 00 06 61 31 62 32 63 33 06 01 00 00 00 0c 44 4a 49 20 4d 69 63 20 4d 69 6e 69 01 02 00 00 00 12 38 00 01 01 30 30 30 30 54 58 30 30 30 30 30 30 30 32 04 02 00 00 00 06 64 34 65 35 66 36 06 02 00 00 00 0c 44 4a 49 20 4d 69 63 20 4d 69 6e 69 b8 fe")

@Test func decodesCapturedFrames() throws {
    for raw in [txLevelPush, rxTxStatusPush, identityPush] {
        let frame = try #require(DUMLFrame.decode(raw))
        #expect(frame.sender == DUMLAddress.receiver)
        #expect(frame.receiver == DUMLAddress.app)
        #expect(frame.encoded() == raw)
    }
}

@Test func rejectsCorruptFrames() {
    var corrupt = txLevelPush
    corrupt[20] ^= 0xff
    #expect(DUMLFrame.decode(corrupt) == nil)
}

@Test func encodesVerifiedStereoAndMonoCommands() {
    // Sent to the hardware and acknowledged with result 0x00.
    let stereo = MicCommand.setChannel(.stereo).frame(sequence: 0xf936)!.encoded()
    #expect(stereo == bytes("55 16 04 fc 02 5a 36 f9 40 5b 01 02 00 00 00 00 08 00 01 02 58 2b"))
    let mono = MicCommand.setChannel(.mono).frame(sequence: 0x8a17)!.encoded()
    #expect(mono == bytes("55 16 04 fc 02 5a 17 8a 40 5b 01 02 00 00 00 00 08 00 01 00 c9 51"))
}

@Test func allowlistRefusesUnverifiedValues() {
    #expect(MicCommand.setChannel(.other(1)).payload == nil)
    #expect(MicCommand.setReceiverGain(dB: 30).payload == nil)
    #expect(MicCommand.setReceiverGain(dB: -6).payload == [0x02, 0, 0, 0, 0, 0x39, 0x00, 0x01, 0xfa])
}

@Test func streamParserSkipsIAP2NoiseAndSplitsFrames() {
    var parser = DUMLStreamParser()
    let noise = bytes("ff 55 02 00 ee 10 ff 55 02 00 ee 10")
    let stream = noise + txLevelPush + rxTxStatusPush
    let first = parser.append(stream.prefix(40))
    let rest = parser.append(stream.dropFirst(40))
    #expect(first.count + rest.count == 2)
}

@Test func decodesStatusModel() throws {
    var status = MicStatus()
    for raw in [identityPush, rxTxStatusPush, txLevelPush] {
        MicStatusDecoder.apply(try #require(DUMLFrame.decode(raw)), to: &status)
    }
    let rx = status.receiver
    #expect(rx.identity.productName == "DJI Mic Mini 2")
    #expect(rx.identity.firmware == "02.03.11.00")
    #expect(rx.channelMode == .mono)
    #expect(rx.noiseCancelling == true)
    #expect(rx.autoPowerOff == true)
    #expect(rx.gainDB == 0)
    #expect(rx.connectedTransmitters == [1, 2])
    #expect(rx.attachedOverUSB == true)
    let tx1 = try #require(status.transmitters[1])
    #expect(tx1.identity.bluetoothID == "a1b2c3")
    #expect(tx1.identity.firmware == "02.03.17.00")
    #expect(tx1.batteryLevel == 1)
    #expect(tx1.isCharging == false)
    #expect(tx1.meterLevel == 0x22)
    #expect(status.transmitters[2]?.identity.firmware == "01.01.00.56")
}

@Test func parsesCapabilities() throws {
    let payload = bytes("71 00 00 00 00 00 31 00 0d 00 01 01 06 00 0c f4 ff ff ff 0c 00 00 00 06 00 00 00 09 00 0c f4 ff ff ff 0c 00 00 00 01 00 00 00 11 00 08 c8 00 00 00 00 00 00 00 05 00 01 07 01 00 00 00 17 00 02 00 08 42 49 0b 00 00 00 00 00 03 00 01 03 04 00 01 05 12 00 01 01 02 00 00 00 17 00 02 00 08 42 49 0b 00 00 00 00 00 03 00 01 03 04 00 01 05 12 00 01 01")
    let caps = try #require(MicCapabilities(payload: payload))
    #expect(caps.receiverGainRange == -12...12)
    #expect(caps.receiverGainStep == 6)
    #expect(caps.receiverChannelModeMask == 0x07)
    #expect(caps.devices[1]?.keys.sorted() == [0x02, 0x03, 0x04, 0x12])
}

@Test func buttonMonitorDecodesConsumerBits() {
    let monitor = ButtonMonitor()
    let events = EventBox()
    monitor.onEvent = { events.append($0) }
    monitor.handle(reportID: 0x06, bytes: [0x06, 0x01, 0x00])
    monitor.handle(reportID: 0x06, bytes: [0x06, 0x00, 0x00])
    #expect(events.items.map(\.button) == [.volumeUp, .volumeUp])
    #expect(events.items.map(\.isDown) == [true, false])
}

private final class EventBox: @unchecked Sendable {
    var items: [MicButtonEvent] = []
    func append(_ event: MicButtonEvent) { items.append(event) }
}

@Test func talkingDetectorPicksTheLouderMicAndHolds() {
    var detector = TalkingDetector()
    let t0 = Date(timeIntervalSince1970: 1000)
    // At rest and under a shared sound the two mics stay close: nobody is "talking".
    #expect(detector.update(tx1: 34, tx2: 31, at: t0) == nil)
    #expect(detector.update(tx1: 65, tx2: 58, at: t0) == nil)
    // Speaking into TX2 separates the meters.
    #expect(detector.update(tx1: 40, tx2: 74, at: t0.addingTimeInterval(1)) == 2)
    // Held briefly through a pause, then released.
    #expect(detector.update(tx1: 38, tx2: 40, at: t0.addingTimeInterval(2)) == 2)
    #expect(detector.update(tx1: 38, tx2: 40, at: t0.addingTimeInterval(3)) == nil)
    // Switching to TX1.
    #expect(detector.update(tx1: 70, tx2: 41, at: t0.addingTimeInterval(4)) == 1)
}
