import LavdeckKit
import LavdeckPluginKit
import Foundation

setvbuf(stdout, nil, _IOLBF, 0)

let usage = """
usage: lavdeck <command>
  status [--watch SECONDS] [--json]   live receiver/transmitter status (read-only)
  version                             receiver hardware name (read-only)
  caps                                receiver capabilities (read-only)
  set channel mono|stereo             switch the receiver's channel mode
  set gain <dB>                       set receiver gain (-12...12, steps from caps)
  record <file.wav|file.m4a> [--seconds N]
  buttons [--seconds N] [--seize]     print transmitter/receiver button events
  takes                               list recordings (newest first)
  plugins                             list installed plugins
  plugins install <folder>            install a plugin folder (symlinked)
  plugins enable|disable <id>         turn a plugin on or off
  run <plugin-id> <action> <take-id>  run a plugin action (or event) on a take, e.g. 2026-10-11/12.34.54
  (takes and run accept --root <folder> to use another recordings folder)
"""

func recordingsRoot() -> URL {
    let argv = CommandLine.arguments
    if let i = argv.firstIndex(of: "--root"), i + 1 < argv.count { return URL(fileURLWithPath: argv[i + 1]) }
    let defaults = UserDefaults(suiteName: PluginStore.defaultsSuite)
    return defaults?.string(forKey: "recordingsRoot").map { URL(fileURLWithPath: $0) }
        ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Recordings/Lavdeck")
}

func micNames() -> [String: String] {
    (UserDefaults(suiteName: PluginStore.defaultsSuite)?.dictionary(forKey: "micNames") as? [String: String]) ?? [:]
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

func option(_ name: String, in args: [String]) -> String? {
    guard let i = args.firstIndex(of: name), i + 1 < args.count else { return nil }
    return args[i + 1]
}

func describe(_ status: MicStatus) -> String {
    let rx = status.receiver
    var lines: [String] = []
    let name = rx.identity.productName ?? "receiver"
    lines.append("RX  \(name)  fw \(rx.identity.firmware ?? "?")  \(rx.channelMode.map { "\($0)" } ?? "?")"
        + "  gain \(rx.gainDB.map { "\($0) dB" } ?? "?")  noise-cancel \(rx.noiseCancelling.map { $0 ? (rx.noiseCancellingLevel == 1 ? "high" : "low") : "off" } ?? "?")"
        + "  connected \(rx.connectedTransmitters.sorted().map { "TX\($0)" }.joined(separator: ","))")
    for index in status.transmitters.keys.sorted() {
        let tx = status.transmitters[index]!
        let connected = rx.connectedTransmitters.contains(index) ? "connected" : "not connected"
        lines.append("TX\(index) \(tx.identity.productName ?? "?") [\(tx.identity.bluetoothID ?? "?")]  fw \(tx.identity.firmware ?? "?")"
            + "  \(connected)  battery \(tx.batteryLevel.map { "\($0)/7" } ?? "?")\(tx.isCharging == true ? " charging" : "")"
            + "  meter \(tx.meterLevel.map(String.init) ?? "?")")
    }
    return lines.joined(separator: "\n")
}

func json(_ status: MicStatus) -> String {
    let rx = status.receiver
    var object: [String: Any] = [
        "receiver": [
            "name": rx.identity.productName as Any, "firmware": rx.identity.firmware as Any,
            "serial": rx.identity.serialNumber as Any, "channel": rx.channelMode.map { "\($0)" } as Any,
            "gainDB": rx.gainDB as Any, "noiseCancelling": rx.noiseCancelling as Any,
            "noiseCancellingLevel": rx.noiseCancellingLevel as Any,
            "connected": rx.connectedTransmitters.sorted(),
        ],
    ]
    object["transmitters"] = status.transmitters.keys.sorted().map { index -> [String: Any] in
        let tx = status.transmitters[index]!
        return ["index": index, "name": tx.identity.productName as Any, "firmware": tx.identity.firmware as Any,
                "rawFlags": tx.rawFlags.map { String($0, radix: 16) } as Any,
                "bluetoothID": tx.identity.bluetoothID as Any, "batteryLevel": tx.batteryLevel as Any,
                "charging": tx.isCharging as Any, "meter": tx.meterLevel as Any]
    }
    let data = try! JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
    return String(decoding: data, as: UTF8.self)
}

func connectedController(timeout: Duration = .seconds(4)) async -> MicController {
    let controller = MicController()
    controller.start()
    let deadline = ContinuousClock.now + timeout
    while ContinuousClock.now < deadline {
        if controller.isConnected && controller.status.receiver.channelMode != nil { return controller }
        try? await Task.sleep(for: .milliseconds(100))
    }
    fail(controller.isConnected ? "Receiver connected but sent no status" : "Receiver not found on USB")
}

let args = Array(CommandLine.arguments.dropFirst())
guard let command = args.first else { print(usage); exit(2) }

switch command {
case "status":
    let controller = await connectedController()
    let asJSON = args.contains("--json")
    if let seconds = option("--watch", in: args).flatMap(Double.init) {
        controller.onStatus = { status in print(asJSON ? json(status) : describe(status) + "\n") }
        try? await Task.sleep(for: .seconds(seconds))
    } else {
        try? await Task.sleep(for: .seconds(1))
        print(asJSON ? json(controller.status) : describe(controller.status))
    }
    controller.stop()

case "version":
    let controller = await connectedController()
    do { print(try await controller.version()) } catch { fail("\(error)") }
    controller.stop()

case "caps":
    let controller = await connectedController()
    do {
        let caps = try await controller.capabilities()
        if let range = caps.receiverGainRange { print("RX gain: \(range.lowerBound)...\(range.upperBound) dB, step \(caps.receiverGainStep ?? 0)") }
        if let mask = caps.receiverChannelModeMask { print(String(format: "RX channel mode mask: 0x%02x", mask)) }
        for (index, items) in caps.devices.sorted(by: { $0.key < $1.key }) {
            let list = items.keys.sorted().map { String(format: "0x%02x", $0) }.joined(separator: " ")
            print("\(index == 0 ? "RX" : "TX\(index)") capability ids: \(list)")
        }
    } catch { fail("\(error)") }
    controller.stop()

case "set":
    guard args.count == 3 else { fail(usage) }
    let controller = await connectedController()
    do {
        switch args[1] {
        case "channel":
            guard let mode = ["mono": ChannelMode.mono, "stereo": .stereo][args[2]] else { fail("channel must be mono or stereo") }
            try await controller.setChannel(mode)
            print("channel set to \(mode)")
        case "gain":
            guard let dB = Int(args[2]) else { fail("gain must be an integer dB value") }
            let caps = try await controller.capabilities(id: MicCapabilities.gainID)
            if let range = caps.receiverGainRange, let step = caps.receiverGainStep, step > 0,
               !range.contains(dB) || (dB - range.lowerBound) % step != 0 {
                fail("gain must be within \(range) in steps of \(step)")
            }
            try await controller.setReceiverGain(dB: dB)
            print("receiver gain set to \(dB) dB")
        default:
            fail(usage)
        }
    } catch { fail("\(error)") }
    controller.stop()

case "record":
    guard args.count >= 2 else { fail(usage) }
    let url = URL(fileURLWithPath: args[1])
    let format: RecordingFormat = url.pathExtension.lowercased() == "m4a" ? .m4a : .wav
    let seconds = option("--seconds", in: args).flatMap(Double.init) ?? 5
    guard await MicRecorder.requestPermission() else { fail("\(RecorderError.permissionDenied)") }
    let recorder = MicRecorder()
    let done = DispatchSemaphore(value: 0)
    recorder.onFinish = { url, error in
        if let error { print("finished with error: \(error)") } else { print("saved \(url.path)") }
        done.signal()
    }
    do { try recorder.start(to: url, format: format) } catch { fail("\(error)") }
    print("recording \(seconds)s to \(url.lastPathComponent)…")
    try? await Task.sleep(for: .seconds(seconds))
    recorder.stop()
    await withCheckedContinuation { continuation in
        DispatchQueue.global().async { done.wait(); continuation.resume() }
    }

case "buttons":
    let seconds = option("--seconds", in: args).flatMap(Double.init) ?? 30
    let monitor = ButtonMonitor(seize: args.contains("--seize"))
    monitor.onEvent = { print("\($0.timestamp.formatted(date: .omitted, time: .standard))  \($0.button) \($0.isDown ? "down" : "up")") }
    monitor.onRawReport = { id, bytes in print(String(format: "  raw report 0x%02x: ", id) + bytes.map { String(format: "%02x", $0) }.joined(separator: " ")) }
    let result = await MainActor.run { monitor.start() }
    print(String(format: "listening for %.0fs (open result 0x%x)…", seconds, result))
    try? await Task.sleep(for: .seconds(seconds))
    await MainActor.run { monitor.stop() }

case "takes":
    for take in TakeLibrary.scan(root: recordingsRoot(), micNames: micNames()) {
        let d = take.descriptor
        let speakers = d.speakers.map(\.name).joined(separator: ", ")
        print("\(d.id)  \(d.duration.map { String(format: "%5.1fs", $0) } ?? "    ?")  \(d.channelMode ?? "?")  [\(speakers)]\(take.hasTranscript ? "  transcript" : "")")
    }

case "plugins":
    let store = PluginStore()
    if args.count >= 3, args[1] == "install" {
        do { let p = try store.install(from: URL(fileURLWithPath: args[2]).standardizedFileURL); print("installed \(p.manifest.name) (\(p.id))") }
        catch { fail("\(error)") }
    } else if args.count >= 3, ["enable", "disable"].contains(args[1]) {
        store.setEnabled(args[2], args[1] == "enable")
        print("\(args[2]) \(args[1])d")
    } else {
        let (plugins, problems) = store.discover()
        if plugins.isEmpty { print("no plugins in \(PluginStore.pluginsFolder.path)") }
        for p in plugins {
            print("\(store.isEnabled(p.id) ? "●" : "○") \(p.manifest.name) \(p.manifest.version)  \(p.id)")
            print("    events: \((p.manifest.events ?? []).joined(separator: ", "))  actions: \((p.manifest.actions ?? []).map(\.id).joined(separator: ", "))")
        }
        problems.forEach { print("! \($0)") }
    }

case "run":
    guard args.count >= 4 else { fail(usage) }
    let store = PluginStore()
    guard let plugin = store.discover().plugins.first(where: { $0.id == args[1] }) else { fail("no plugin \(args[1])") }
    let parts = args[3].split(separator: "/").map(String.init)
    guard parts.count == 2, let take = TakeLibrary.take(folder: recordingsRoot().appendingPathComponent(parts[0]), stem: parts[1], micNames: micNames()) else {
        fail("no take \(args[3])")
    }
    let isEvent = plugin.manifest.events?.contains(args[2]) == true
    let request = plugin.request(kind: isEvent ? "event" : "action", name: args[2], take: take.descriptor,
                                 settings: store.settings(for: plugin))
    let started = Date()
    do {
        let result = try await PluginRunner.run(plugin, request: request) { message in
            switch message.type {
            case .progress: print(String(format: "  %3.0f%%  %@", (message.fraction ?? 0) * 100, message.message ?? ""))
            case .output: print("  → \(message.path ?? "") (\(message.kind ?? ""))")
            case .log: print("  · \(message.message ?? "")")
            case .done: print("  done\(message.message.map { ": \($0)" } ?? "")")
            case .error: print("  error: \(message.message ?? "")")
            }
        }
        print(String(format: "finished in %.1fs, %d output(s)", Date().timeIntervalSince(started), result.outputs.count))
    } catch { fail("\(error)") }

default:
    print(usage)
    exit(2)
}
