import Foundation
import Synchronization
import Testing
import LavdeckCapture
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

// MARK: - Take library and plugins

import LavdeckPluginKit

private func scratchFolder() throws -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("lavdeck-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

@Test func takeLibraryGroupsFilesByStem() throws {
    let root = try scratchFolder()
    defer { try? FileManager.default.removeItem(at: root) }
    let day = root.appendingPathComponent("2026-10-11")
    try FileManager.default.createDirectory(at: day, withIntermediateDirectories: true)
    for name in ["12.00.00 TX1.wav", "12.00.00 TX2.wav", "12.00.00.transcript.json", "12.00.00 TX1 · voice-ada.wav",
                 "13.30.05 Mix.wav", "notes.txt"] {
        FileManager.default.createFile(atPath: day.appendingPathComponent(name).path, contents: Data())
    }
    try TakeMetadata(started: Date(timeIntervalSince1970: 0), duration: 42, channelMode: "stereo", format: "wav",
                     mics: [.init(index: 1, name: "Host", bluetoothID: "aa", armed: true),
                            .init(index: 2, name: "Guest", bluetoothID: "bb", armed: true)])
        .write(to: day.appendingPathComponent("12.00.00.take.json"))

    let takes = TakeLibrary.scan(root: root)
    #expect(takes.map(\.id).sorted() == ["2026-10-11/12.00.00", "2026-10-11/13.30.05"])
    let duo = try #require(takes.first { $0.id == "2026-10-11/12.00.00" })
    #expect(duo.descriptor.isTwoSpeaker)
    #expect(duo.descriptor.speakers.map(\.name) == ["Host", "Guest"])
    #expect(duo.descriptor.duration == 42)
    #expect(duo.hasTranscript)
    #expect(duo.descriptor.files.contains { $0.role == .derived && $0.kind == "audio" && $0.mic == 1 })
    let mono = try #require(takes.first { $0.id == "2026-10-11/13.30.05" })
    #expect(mono.descriptor.channelMode == "mono")
    #expect(mono.descriptor.speakers.isEmpty)
    #expect(!mono.hasTranscript)
}

private func makePlugin(_ script: String) throws -> InstalledPlugin {
    let folder = try scratchFolder()
    let exe = folder.appendingPathComponent("run")
    try ("#!/bin/sh\n" + script).write(to: exe, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: exe.path)
    let manifest = PluginManifest(id: "test.plugin", name: "Test", version: "1", summary: nil, executable: "run")
    return InstalledPlugin(folder: folder, manifest: manifest)
}

private let sampleRequest = PluginRequest(kind: "action", name: "go",
                                          take: TakeDescriptor(id: "d/s", folder: "/tmp", stem: "s", files: [], speakers: []),
                                          settings: ["model": .string("small")], outputDirectory: "/tmp", pluginDirectory: "/tmp")

@Test func pluginRunnerStreamsMessages() async throws {
    let plugin = try makePlugin("""
    read -r request
    echo '{"type":"progress","fraction":0.5,"message":"half"}'
    echo "not json goes to the log"
    echo "{\\"type\\":\\"output\\",\\"path\\":\\"/tmp/$LAVDECK_SETTING_MODEL.txt\\",\\"kind\\":\\"text\\"}"
    echo '{"type":"done"}'
    """)
    defer { try? FileManager.default.removeItem(at: plugin.folder) }
    let result = try await PluginRunner.run(plugin, request: sampleRequest)
    #expect(result.outputs.map(\.path) == ["/tmp/small.txt"])
    #expect(result.log.contains("not json goes to the log"))
}

@Test func pluginRunnerReportsErrorsAndSurvivesUnreadStdin() async throws {
    let plugin = try makePlugin("""
    echo '{"type":"error","message":"model missing"}'
    exit 1
    """)
    defer { try? FileManager.default.removeItem(at: plugin.folder) }
    await #expect(throws: PluginRunError.self) { try await PluginRunner.run(plugin, request: sampleRequest) }
    do { _ = try await PluginRunner.run(plugin, request: sampleRequest) } catch {
        #expect("\(error)" == "model missing")
    }
}

// MARK: - Capture plugins (protocol 2)

@Test func captureHostStartsStopsAndCollectsOutputs() async throws {
    let folder = try scratchFolder()
    defer { try? FileManager.default.removeItem(at: folder) }
    let exe = folder.appendingPathComponent("capture")
    try """
    #!/bin/sh
    read -r start
    case "$start" in *'"type":"start"'*) ;; *) echo '{"type":"error","message":"expected start"}'; exit 1;; esac
    echo '{"type":"started","source":"screen","startHostTime":100.5}'
    read -r stop
    echo '{"type":"output","source":"screen","kind":"video","path":"/tmp/x Screen.mov","startHostTime":100.5}'
    echo '{"type":"stopped"}'
    """.write(to: exe, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: exe.path)
    let manifest = PluginManifest(id: "test.capture", name: "Capture", version: "1", protocol: 2, executable: "capture",
                                  kind: "capture", capture: CaptureSpec(arguments: []))
    let plugin = InstalledPlugin(folder: folder, manifest: manifest)
    #expect(plugin.manifest.isCapture)

    let started = Mutex<[String]>([])
    let host = CaptureHost { _, event in if event.type == .started { started.withLock { $0.append(event.source ?? "") } } }
    let problems = host.start([(plugin, [:])], take: CaptureTake(folder: "/tmp", stem: "x"))
    #expect(problems.isEmpty)
    try await Task.sleep(for: .milliseconds(300))
    let result = await host.stop(timeout: 5)
    #expect(result.problems.isEmpty)
    #expect(result.outputs == [CaptureOutput(pluginID: "test.capture", path: "/tmp/x Screen.mov", source: "screen",
                                             kind: "video", startHostTime: 100.5)])
    #expect(started.withLock { $0 } == ["screen"])
}

@Test func takeLibraryReadsCapturedMediaWithOffsets() throws {
    let root = try scratchFolder()
    defer { try? FileManager.default.removeItem(at: root) }
    let day = root.appendingPathComponent("2026-10-11")
    try FileManager.default.createDirectory(at: day, withIntermediateDirectories: true)
    for name in ["09.00.00 Stereo.wav", "09.00.00 Screen.mov", "09.00.00 Camera.mov", "09.00.00 PiP.mp4"] {
        FileManager.default.createFile(atPath: day.appendingPathComponent(name).path, contents: Data())
    }
    try TakeMetadata(started: Date(), duration: 10, channelMode: "stereo", format: "wav", mics: [], audioStartHostTime: 50,
                     media: [.init(file: "09.00.00 Screen.mov", source: "screen", kind: "video", offset: 0.42, plugin: "p")])
        .write(to: day.appendingPathComponent("09.00.00.take.json"))
    let take = try #require(TakeLibrary.scan(root: root).first)
    let screen = try #require(take.descriptor.files.first { $0.source == "screen" })
    #expect(screen.role == .capture && screen.offset == 0.42)
    let camera = try #require(take.descriptor.files.first { $0.source == "camera" })
    #expect(camera.role == .capture && camera.offset == nil)
    #expect(take.descriptor.files.first { $0.source == "pip" }?.role == .derived)
    #expect(take.videoFiles.count == 3)
}

@Test func postRequestsUseThePluginsDeclaredProtocol() {
    let v1 = InstalledPlugin(folder: URL(fileURLWithPath: "/tmp"),
                             manifest: PluginManifest(id: "a", name: "A", version: "1", protocol: 1, executable: "x"))
    let take = TakeDescriptor(id: "d/s", folder: "/tmp", stem: "s")
    #expect(v1.request(kind: "action", name: "go", take: take, settings: [:]).protocol == 1)
}

@Test func appPluginsFillTheOpenTakeURL() {
    let spec = AppSpec(bundle: "x.app", openTake: "studio://open?folder={folder}&stem={stem}")
    let take = TakeDescriptor(id: "2026-10-11/20.11.43", folder: "/Users/me/Recordings/Lavdeck/2026-10-11", stem: "20.11.43")
    let url = spec.url(for: take)
    #expect(url?.absoluteString == "studio://open?folder=/Users/me/Recordings/Lavdeck/2026-10-11&stem=20.11.43")
    let spaced = TakeDescriptor(id: "a/b", folder: "/Users/me/My Takes & Co", stem: "b")
    let items = URLComponents(url: spec.url(for: spaced)!, resolvingAgainstBaseURL: false)?.queryItems
    #expect(items?.first { $0.name == "folder" }?.value == "/Users/me/My Takes & Co")
}

@Test func actionsApplyOnlyToTakesWithWhatTheyRequire() {
    let remake = PluginAction(id: "pip", title: "Remake video", requires: ["video"])
    let anything = PluginAction(id: "info", title: "Take info")
    let audio = TakeDescriptor(id: "d/1", folder: "/d", stem: "1", files: [TakeFile(path: "/d/1 TX1.wav", role: .track, mic: 1)])
    var camera = audio
    camera.files.append(TakeFile(path: "/d/1 Camera.mov", role: .capture, kind: "video", source: "camera"))
    // A finished video without its raw recordings can't be remade.
    var finishedOnly = audio
    finishedOnly.files.append(TakeFile(path: "/d/1 PiP.mp4", role: .derived, kind: "video", source: "pip"))
    #expect(!remake.applies(to: audio))
    #expect(remake.applies(to: camera))
    #expect(!remake.applies(to: finishedOnly))
    #expect(anything.applies(to: audio))
    #expect(camera.has("camera") && !camera.has("screen"))
}

@Test func videoSettingsCarryOverFromTheScreenAndCameraPlugin() throws {
    // One fixed suite, emptied before and after, so runs don't leave a preferences file each.
    let suite = "lavdeck.tests.video"
    let defaults = try #require(UserDefaults(suiteName: suite))
    defaults.removePersistentDomain(forName: suite)
    defer { defaults.removePersistentDomain(forName: suite) }
    // No plugin and nothing saved: video stays off.
    #expect(LavdeckCapture.VideoSettings.load(defaults) == LavdeckCapture.VideoSettings())
    defaults.set(["io.github.piedol.lavdeck.capture": true], forKey: "plugins.enabled")
    defaults.set(["screen": false, "camera": true, "fps": 60.0, "pip_corner": "top-left", "camera_preview_size": "large"],
                 forKey: "plugins.settings.io.github.piedol.lavdeck.capture")
    let carried = LavdeckCapture.VideoSettings.load(defaults)
    #expect(!carried.screen && carried.camera)
    #expect(carried.screenFPS == 60 && carried.bubbleCorner == .topLeft && carried.previewSize == .large)
    // Once saved, the built-in settings win.
    var changed = carried
    changed.screen = true
    changed.save(defaults)
    #expect(LavdeckCapture.VideoSettings.load(defaults).screen)
}
