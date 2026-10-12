import AppKit
import LavdeckKit
import LavdeckPluginKit
import Foundation
import Observation
import ServiceManagement

/// What needs attention, in priority order.
enum AttentionKind: Equatable { case dropped, lowBattery, error }

/// The menu bar glyph's states.
enum GlyphState: Equatable { case healthy, recording, attention(AttentionKind), absent }

@MainActor
@Observable
final class AppModel {
    // Live device state
    private(set) var status = MicStatus()
    private(set) var isConnected = false
    /// Transmitters seen connected at least once this session (a drop from this set is a problem).
    private(set) var knownTransmitters: Set<Int> = []

    // Pending control writes
    private(set) var pendingChannel: ChannelMode?
    private(set) var pendingGain: Int?
    private(set) var lastError: String?

    // Recording
    private(set) var isRecording = false
    private(set) var recordingStarted: Date?
    private(set) var recordingLevels: [Float] = []
    private(set) var lastSavedFolder: URL?

    // Settings (persisted)
    var recordingsRoot: URL { didSet { defaults.set(recordingsRoot.path, forKey: "recordingsRoot") } }
    var format: RecordingFormat { didSet { defaults.set(format.rawValue, forKey: "format") } }
    var splitTracks: Bool { didSet { defaults.set(splitTracks, forKey: "splitTracks") } }
    var tapToRecord: Bool { didSet { defaults.set(tapToRecord, forKey: "tapToRecord"); restartButtons() } }
    /// Transmitters that record. One armed mic records alone (needs stereo); none disables Record.
    var armed: Set<Int> { didSet { defaults.set(Array(armed), forKey: "armed") } }
    /// User names for physical mics, keyed by Bluetooth ID so a name follows the mic, not the slot.
    var micNames: [String: String] { didSet { defaults.set(micNames, forKey: "micNames") } }

    // Identify mode: temporarily stereo so each mic's own level shows which one is being spoken into.
    private(set) var identifying = false
    private(set) var talkingMic: Int?
    private var detector = TalkingDetector()
    private var identifyRestore: ChannelMode?
    private var identifyTimeout: Task<Void, Never>?
    var launchAtLogin: Bool {
        get { SMAppService.mainApp.status == .enabled }
        set {
            do { newValue ? try SMAppService.mainApp.register() : try SMAppService.mainApp.unregister() }
            catch { lastError = "Login item: \(error.localizedDescription)" }
        }
    }

    private let defaults = UserDefaults.standard
    let plugins = PluginHost()
    let permissions = PermissionCenter()
    private let controller = MicController()
    private let recorder = MicRecorder()
    private var buttons: ButtonMonitor?
    private var hotKey: HotKey?
    private var levelTimer: Timer?
    private var currentStem: URL?
    private(set) var recordingMode: ChannelMode?
    private var recordingSolo: Int?
    /// Capture plugins (screen, camera, …) recording alongside the current take.
    private var capture: CaptureHost?
    private var captureStop: Task<(outputs: [CaptureOutput], problems: [CaptureHost.Problem]), Never>?
    /// Sources capture plugins have started for the current take, e.g. `screen`, `camera`.
    private(set) var captureSources: [String] = []

    init() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        recordingsRoot = defaults.string(forKey: "recordingsRoot").map { URL(fileURLWithPath: $0) }
            ?? home.appendingPathComponent("Recordings/Lavdeck", isDirectory: true)
        format = defaults.string(forKey: "format").flatMap(RecordingFormat.init(rawValue:)) ?? .wav
        splitTracks = defaults.object(forKey: "splitTracks") as? Bool ?? true
        tapToRecord = defaults.object(forKey: "tapToRecord") as? Bool ?? true
        armed = Set(defaults.array(forKey: "armed") as? [Int] ?? [1, 2])
        micNames = defaults.dictionary(forKey: "micNames") as? [String: String] ?? [:]

        controller.onStatus = { [weak self] status in
            Task { @MainActor in self?.apply(status) }
        }
        controller.onConnectionChange = { [weak self] connected in
            Task { @MainActor in self?.isConnected = connected }
        }
        recorder.onFinish = { [weak self] url, error in
            Task { @MainActor in self?.recordingFinished(url, error: error) }
        }
        controller.start()
        restartButtons()
        hotKey = HotKey(keyCode: 15 /* R */, modifiers: [.command, .option]) { [weak self] in
            Task { @MainActor in self?.toggleRecording() }
        }
    }

    // MARK: Derived state

    var receiver: ReceiverStatus { status.receiver }

    func transmitter(_ index: Int) -> TransmitterStatus? { status.transmitters[index] }

    func isLinked(_ index: Int) -> Bool { isConnected && receiver.connectedTransmitters.contains(index) }

    var droppedTransmitters: [Int] { knownTransmitters.filter { !isLinked($0) }.sorted() }

    /// Development aid for snapshots: report every linked mic as low.
    var debugForceLowBattery = false

    /// Linked transmitters whose battery estimate is at or below 1 of 7.
    /// Mics to warn about for low battery. The Mobile RX doesn't report battery level (its level field is a
    /// fixed placeholder), so there's nothing truthful to warn on; only the snapshot aid uses this.
    var lowBattery: [Int] {
        debugForceLowBattery ? receiver.connectedTransmitters.sorted() : []
    }

    var glyph: GlyphState {
        if isRecording { return .recording }
        if !isConnected { return .absent }
        if !droppedTransmitters.isEmpty { return .attention(.dropped) }
        if lastError != nil { return .attention(.error) }
        if !lowBattery.isEmpty { return .attention(.lowBattery) }
        return .healthy
    }

    /// Transmitters whose firmware is older than the newest one connected, and which one that is.
    var outdatedFirmware: [(index: Int, firmware: String, newest: String, newestIndex: Int)] {
        let versions = status.transmitters.compactMap { index, tx in tx.identity.firmware.map { (index, $0) } }
        guard let newest = versions.max(by: { $0.1.compare($1.1, options: .numeric) == .orderedAscending }) else { return [] }
        return versions.filter { $0.1.compare(newest.1, options: .numeric) == .orderedAscending }
            .sorted { $0.0 < $1.0 }.map { ($0.0, $0.1, newest.1, newest.0) }
    }

    /// The single armed mic when exactly one is armed.
    var soloMic: Int? { armed.count == 1 ? armed.first : nil }

    func toggleArm(_ index: Int) {
        guard !isRecording else { return }
        if armed.contains(index) { armed.remove(index) } else { armed.insert(index) }
    }

    /// What a strip's file label says while recording.
    func destination(for index: Int) -> String? {
        guard armed.contains(index) else { return nil }
        let ext = format.rawValue
        if let solo = soloMic { return solo == index ? "TX\(index).\(ext)" : nil }
        if recordingMode == .stereo { return splitTracks && format == .wav ? "TX\(index).\(ext)" : "Stereo" }
        return "Mix.\(ext)"
    }

    var displayedChannel: ChannelMode? { pendingChannel ?? receiver.channelMode }
    var displayedGain: Int? { pendingGain ?? receiver.gainDB }

    /// The receiver's physical O / L / H noise-cancelling switch: off, or on at level 0 (Low) / 1 (High).
    var noiseCancelLabel: String {
        guard let on = receiver.noiseCancelling else { return "…" }
        guard on else { return "Off" }
        switch receiver.noiseCancellingLevel {
        case 0?: return "Low"
        case 1?: return "High"
        case let level?: return "On (\(level))"
        case nil: return "On"
        }
    }

    /// In mono the receiver mixes both mics, so it (and the audio) can only report the mix.
    var metersAreMix: Bool { receiver.channelMode != .stereo }

    /// 0...1 meter fill for a transmitter. A disarmed mic reads 0 (it is not in the take).
    /// While recording this is real per-channel audio (L = TX1, R = TX2 in stereo); otherwise it is
    /// the receiver's own meter, which is per mic in stereo and the mix in mono.
    func meter(_ index: Int) -> Double {
        guard identifying || armed.contains(index), isLinked(index) else { return 0 }
        if isRecording, recordingLevels.count == 2 {
            let channel = receiver.channelMode == .stereo ? index - 1 : 0
            let db = Double(recordingLevels[min(max(channel, 0), 1)])
            return max(0, min(1, (db + 60) / 60))
        }
        guard let raw = transmitter(index)?.meterLevel else { return 0 }
        // Receiver meter units: ~30 in a quiet room, ~45 at rest with noise, ~75 for loud speech/tone.
        return max(0, min(1, (Double(raw) - 25) / 60))
    }

    func dBFS(_ index: Int) -> Int? {
        guard isRecording, recordingLevels.count == 2 else { return nil }
        let channel = receiver.channelMode == .stereo ? index - 1 : 0
        return Int(recordingLevels[min(max(channel, 0), 1)].rounded())
    }

    // MARK: Controls

    func setChannel(_ mode: ChannelMode) {
        guard !isRecording, pendingChannel == nil, receiver.channelMode != mode else { return }
        pendingChannel = mode
        lastError = nil
        Task {
            do { try await controller.setChannel(mode) } catch { lastError = "Couldn’t switch to \(mode): \(error)" }
            pendingChannel = nil
        }
    }

    func setGain(_ dB: Int) {
        guard pendingGain == nil, receiver.gainDB != dB else { return }
        pendingGain = dB
        lastError = nil
        Task {
            do { try await controller.setReceiverGain(dB: dB) } catch { lastError = "Couldn’t set gain to \(dB) dB: \(error)" }
            pendingGain = nil
        }
    }

    // MARK: Recording

    func toggleRecording() {
        isRecording ? stopRecording() : startRecording()
    }

    func startRecording() {
        guard !isRecording else { return }
        guard !armed.isEmpty else {
            // The hotkey and mic taps work with the popover closed, so say why nothing happened.
            NSSound.beep()
            lastError = "Both mics are switched off. Click OFF under a mic to arm it, then record."
            return
        }
        lastError = nil
        Task { [weak self] in
            guard let self else { return }
            if identifying { await endIdentify() }
            if soloMic != nil, receiver.channelMode != .stereo {
                // One mic alone needs the receiver in stereo (TX1 left, TX2 right); mono mixes both.
                do { try await controller.setChannel(.stereo) } catch {
                    lastError = "Couldn’t switch to stereo to record one mic: \(error)"
                    return
                }
            }
            guard await MicRecorder.requestPermission() else {
                lastError = "Microphone access is off. Allow it in System Settings › Privacy & Security › Microphone."
                return
            }
            let now = Date()
            let day = recordingsRoot.appendingPathComponent(Self.dayFormatter.string(from: now), isDirectory: true)
            do {
                try FileManager.default.createDirectory(at: day, withIntermediateDirectories: true)
                let stamp = Self.stemFormatter.string(from: now)  // 24-hour, e.g. 19.49.23
                let stem = day.appendingPathComponent(stamp)
                let target: URL
                if soloMic != nil {
                    // Record stereo to a scratch file, keep only the armed mic's channel at the end.
                    let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("lavdeck-\(UUID().uuidString).\(format.rawValue)")
                    target = scratch
                } else {
                    // Mono mixes both mics into L and R, so the take is named for what it is.
                    let kind = receiver.channelMode == .stereo ? "Stereo" : "Mix"
                    target = day.appendingPathComponent("\(stamp) \(kind).\(format.rawValue)")
                }
                try recorder.start(to: target, format: format)
                await startCapture(folder: day, stem: stamp)
                currentStem = stem
                recordingMode = receiver.channelMode
                recordingSolo = soloMic
                recordingStarted = now
                isRecording = true
                levelTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 15, repeats: true) { [weak self] _ in
                    Task { @MainActor in self?.recordingLevels = self?.recorder.channelLevels() ?? [] }
                }
            } catch {
                lastError = "Couldn’t start recording: \(error)"
            }
        }
    }

    /// Local date (the ISO 8601 format style defaults to UTC, which put evening takes in tomorrow's folder).
    private static let dayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    private static let stemFormatter: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "HH.mm.ss"
        return f
    }()

    func stopRecording() {
        guard isRecording else { return }
        stopCapture()
        recorder.stop()
    }

    private func startCapture(folder: URL, stem: String) async {
        let enabled = plugins.capturePlugins
        captureSources = []
        guard !enabled.isEmpty else { return }
        // Lavdeck asks for each permission once, for every plugin; plugins run under Lavdeck's grants.
        let needed = Array(Set(enabled.flatMap { $0.manifest.permissions ?? [] }))
        await permissions.request(needed)
        let granted = Set(permissions.granted(needed))
        if let missing = needed.first(where: { !granted.contains($0) }) {
            lastError = missing == "screen-recording"
                ? "Screen recording isn’t on for this Lavdeck yet. Turn on Lavdeck in System Settings › Privacy & Security › Screen & System Audio Recording, then quit and reopen Lavdeck (macOS applies it on relaunch)."
                : "\(PermissionCenter.name(missing)) access is off for Lavdeck. Turn it on in System Settings › Privacy & Security."
        }
        let host = CaptureHost { [weak self] plugin, event in
            Task { @MainActor in self?.captureEvent(plugin, event) }
        }
        let problems = host.start(enabled.map { ($0, plugins.store.settings(for: $0)) },
                                  take: CaptureTake(folder: folder.path, stem: stem), granted: granted)
        if let problem = problems.first { lastError = "\(problem.pluginName): \(problem.message)" }
        capture = host.isRunning ? host : nil
    }

    private func captureEvent(_ plugin: InstalledPlugin, _ event: CaptureEvent) {
        switch event.type {
        case .started:
            if let source = event.source, !captureSources.contains(source) { captureSources.append(source) }
        case .error:
            lastError = "\(plugin.manifest.name): \(event.message ?? "error")"
        default: break
        }
    }

    private func stopCapture() {
        guard let host = capture, captureStop == nil else { return }
        capture = nil
        captureStop = Task.detached { await host.stop() }
    }

    private func recordingFinished(_ url: URL, error: Error?) {
        levelTimer?.invalidate()
        levelTimer = nil
        let started = recordingStarted
        isRecording = false
        recordingLevels = []
        recordingStarted = nil
        lastSavedFolder = currentStem?.deletingLastPathComponent() ?? url.deletingLastPathComponent()
        let stem = currentStem, solo = recordingSolo, mode = recordingMode, fmt = format, split = splitTracks
        let audioStart = recorder.startHostTime
        stopCapture()  // the audio can end on its own (an error, the receiver unplugged)
        let captureDone = captureStop
        captureStop = nil
        currentStem = nil
        recordingSolo = nil
        if let error {
            lastError = "Recording stopped with an error: \(error.localizedDescription)"
            Task { _ = await captureDone?.value; self.captureSources = []; self.takeSaved() }
            return
        }
        guard let stem else { takeSaved(); return }
        // Snapshot who was on which mic now; names can change later.
        let mics = [1, 2].map { index in
            TakeMetadata.Mic(index: index, name: displayName(index), bluetoothID: transmitter(index)?.identity.bluetoothID,
                             armed: armed.contains(index))
        }
        Task { [weak self] in
            // Post-process off the main thread: keep one mic's channel, or split stereo into tracks.
            let failure: String? = await Task.detached { () -> String? in
                if let solo {
                    let out = URL(fileURLWithPath: stem.path + " TX\(solo).\(fmt.rawValue)")
                    defer { try? FileManager.default.removeItem(at: url) }  // the scratch stereo take
                    do { try TrackSplitter.extract(url, channel: solo - 1, to: out) } catch { return "Couldn’t save TX\(solo)’s track: \(error)" }
                } else if split, mode == .stereo, fmt == .wav {
                    let left = URL(fileURLWithPath: stem.path + " TX1.wav"), right = URL(fileURLWithPath: stem.path + " TX2.wav")
                    do { try TrackSplitter.split(url, left: left, right: right) } catch { return "Couldn’t split tracks: \(error)" }
                }
                return nil
            }.value
            let captured = await captureDone?.value
            guard let self else { return }
            captureSources = []
            if let failure { lastError = failure }
            if let problem = captured?.problems.first { lastError = "\(problem.pluginName): \(problem.message)" }
            let folder = stem.deletingLastPathComponent()
            let stemName = stem.lastPathComponent
            let media = captured?.outputs.map { output in
                TakeMetadata.Media(file: (output.path as NSString).lastPathComponent, source: output.source, kind: output.kind,
                                   offset: zip(output.startHostTime, audioStart).map { $0 - $1 }, plugin: output.pluginID)
            }
            let metadata = TakeMetadata(started: started ?? Date(), duration: started.map { Date().timeIntervalSince($0) },
                                        channelMode: mode.map { "\($0)" }, format: fmt.rawValue,
                                        mics: mics.map { var m = $0; if solo != nil { m.armed = m.index == solo }; return m },
                                        audioStartHostTime: audioStart, media: (media?.isEmpty ?? true) ? nil : media)
            try? metadata.write(to: folder.appendingPathComponent("\(stemName).take.json"))
            if let take = TakeLibrary.take(folder: folder, stem: stemName, micNames: micNames) {
                plugins.takeFinished(take)
            }
            libraryRevision += 1
            takeSaved()
        }
    }

    private var takeSavedWaiters: [CheckedContinuation<Void, Never>] = []

    /// Stops a running take and waits until its files and metadata are written (for quitting).
    func finishRecordingForQuit() async {
        guard isRecording || currentStem != nil else { return }
        await withCheckedContinuation { continuation in
            takeSavedWaiters.append(continuation)
            stopRecording()
        }
    }

    private func takeSaved() {
        let waiters = takeSavedWaiters
        takeSavedWaiters = []
        waiters.forEach { $0.resume() }
    }

    /// Bumped when a take is written or deleted, so the recordings manager refreshes.
    private(set) var libraryRevision = 0
    func noteLibraryChanged() { libraryRevision += 1 }

    /// All takes in the recordings folder, newest first.
    func scanTakes() -> [Take] { TakeLibrary.scan(root: recordingsRoot, micNames: micNames) }

    func revealRecordings() {
        let folder = lastSavedFolder ?? recordingsRoot
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        NSWorkspace.shared.activateFileViewerSelecting([folder])
    }

    // MARK: Plumbing

    /// Demo mode (screenshots, no hardware): synthetic devices, live updates ignored.
    private var demo = false

    func loadDemo(recording: Bool) {
        demo = true
        controller.stop()
        var s = MicStatus()
        s.receiver.identity.productName = "DJI Mic Mini 2"
        s.receiver.identity.firmware = "02.03.11.00"
        s.receiver.channelMode = .stereo
        s.receiver.gainDB = 0
        s.receiver.noiseCancelling = true
        s.receiver.noiseCancellingLevel = 1
        s.receiver.connectedTransmitters = [1, 2]
        for (index, id, fw, meter, battery, charging) in [(1, "a1b2c3", "02.03.17.00", 58, 1, false), (2, "d4e5f6", "02.03.17.00", 41, 1, !recording)] {
            var tx = TransmitterStatus(index: index)
            tx.identity.productName = "DJI Mic Mini"
            tx.identity.bluetoothID = id
            tx.identity.firmware = fw
            tx.batteryLevel = battery
            tx.isCharging = charging
            tx.meterLevel = meter
            s.transmitters[index] = tx
        }
        demoNames = ["a1b2c3": "Host", "d4e5f6": "Guest"]
        status = s
        isConnected = true
        knownTransmitters = [1, 2]
        armed = [1, 2]
        if recording {
            isRecording = true
            recordingMode = .stereo
            recordingStarted = Date().addingTimeInterval(-767)
            recordingLevels = [-14, -21]
        } else {
            isRecording = false
            recordingStarted = nil
            recordingLevels = []
        }
    }

    // MARK: Identify and naming

    /// Names used by the demo snapshots, so they never touch the user's saved names.
    private var demoNames: [String: String] = [:]

    /// Demo snapshot of identify mode: one mic highlighted as talking.
    func loadDemoIdentify() {
        loadDemo(recording: false)
        identifying = true
        talkingMic = 1
    }

    /// The name shown on a strip: the user's name for that physical mic, else the slot.
    func displayName(_ index: Int) -> String {
        if demo, let id = transmitter(index)?.identity.bluetoothID, let name = demoNames[id] { return name }
        if let id = transmitter(index)?.identity.bluetoothID, let name = micNames[id], !name.isEmpty { return name }
        return "TX\(index)"
    }

    func rename(_ index: Int, to name: String) {
        guard let id = transmitter(index)?.identity.bluetoothID else { return }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        micNames[id] = trimmed.isEmpty || trimmed == "TX\(index)" ? nil : String(trimmed.prefix(14))
    }

    func startIdentify() {
        guard !isRecording, !identifying, isConnected else { return }
        identifying = true
        detector.reset()
        talkingMic = nil
        if receiver.channelMode != .stereo {
            identifyRestore = receiver.channelMode
            setChannel(.stereo)
        }
        identifyTimeout?.cancel()
        identifyTimeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(45))
            guard !Task.isCancelled else { return }
            self?.stopIdentify()
        }
    }

    func stopIdentify() {
        guard identifying else { return }
        Task { await endIdentify() }
    }

    /// Leaves identify mode and, once any in-flight switch has settled, restores the previous mode.
    func endIdentify() async {
        guard identifying else { return }
        identifying = false
        talkingMic = nil
        identifyTimeout?.cancel()
        identifyTimeout = nil
        guard let restore = identifyRestore else { return }
        identifyRestore = nil
        for _ in 0..<40 where pendingChannel != nil { try? await Task.sleep(for: .milliseconds(100)) }
        guard receiver.channelMode != restore else { return }
        pendingChannel = restore
        do { try await controller.setChannel(restore) } catch { lastError = "Couldn’t switch back to \(restore): \(error)" }
        pendingChannel = nil
    }

    private func apply(_ new: MicStatus) {
        guard !demo else { return }
        status = new
        if identifying, new.receiver.channelMode == .stereo {
            talkingMic = detector.update(tx1: new.transmitters[1]?.meterLevel, tx2: new.transmitters[2]?.meterLevel)
        }
        knownTransmitters.formUnion(new.receiver.connectedTransmitters)
        logBattery(new)
        if !isConnected, new.receiver.channelMode != nil { isConnected = true }
        if new.receiver.channelMode == nil, new.lastPush == nil { isConnected = false }
    }

    private static let logFormatter: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return f
    }()

    /// Battery level and charging flag per transmitter, as last logged.
    private var loggedBattery: [Int: String] = [:]

    /// Appends a line to ~/Library/Logs/Lavdeck/battery.log whenever a transmitter's charging flag or raw
    /// status changes, to keep studying what the receiver reports.
    private func logBattery(_ new: MicStatus) {
        for index in new.receiver.connectedTransmitters {
            guard let tx = new.transmitters[index], let level = tx.batteryLevel else { continue }
            let state = "battery code \(level) (\(ChargeState.label(for: level) ?? "?")), charging \(tx.isCharging == true ? "yes" : "no"), raw \(String(format: "%08x", tx.rawFlags ?? 0))"
            guard loggedBattery[index] != state else { continue }
            loggedBattery[index] = state
            let folder = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/Lavdeck", isDirectory: true)
            try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let url = folder.appendingPathComponent("battery.log")
            let line = "\(Self.logFormatter.string(from: Date())) TX\(index) \(displayName(index)): \(state)\n"
            if let handle = try? FileHandle(forWritingTo: url) {
                handle.seekToEndOfFile()
                handle.write(Data(line.utf8))
                try? handle.close()
            } else {
                try? Data(line.utf8).write(to: url)
            }
        }
    }

    private func restartButtons() {
        buttons?.stop()
        buttons = nil
        guard tapToRecord else { return }
        let monitor = ButtonMonitor(seize: true)
        monitor.onEvent = { [weak self] event in
            guard event.isDown, event.button == .volumeUp else { return }
            Task { @MainActor in self?.toggleRecording() }
        }
        monitor.start()
        buttons = monitor
    }
}

private func zip(_ a: Double?, _ b: Double?) -> (Double, Double)? {
    guard let a, let b else { return nil }
    return (a, b)
}
