import AppKit
import LavdeckKit
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
    var launchAtLogin: Bool {
        get { SMAppService.mainApp.status == .enabled }
        set {
            do { newValue ? try SMAppService.mainApp.register() : try SMAppService.mainApp.unregister() }
            catch { lastError = "Login item: \(error.localizedDescription)" }
        }
    }

    private let defaults = UserDefaults.standard
    private let controller = MicController()
    private let recorder = MicRecorder()
    private var buttons: ButtonMonitor?
    private var hotKey: HotKey?
    private var levelTimer: Timer?
    private var currentStem: URL?
    private(set) var recordingMode: ChannelMode?
    private var recordingSolo: Int?

    init() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        recordingsRoot = defaults.string(forKey: "recordingsRoot").map { URL(fileURLWithPath: $0) }
            ?? home.appendingPathComponent("Recordings/Lavdeck", isDirectory: true)
        format = defaults.string(forKey: "format").flatMap(RecordingFormat.init(rawValue:)) ?? .wav
        splitTracks = defaults.object(forKey: "splitTracks") as? Bool ?? true
        tapToRecord = defaults.object(forKey: "tapToRecord") as? Bool ?? true
        armed = Set(defaults.array(forKey: "armed") as? [Int] ?? [1, 2])

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
    var lowBattery: [Int] {
        receiver.connectedTransmitters.filter { debugForceLowBattery || (transmitter($0)?.batteryLevel ?? 7) <= 1 }.sorted()
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

    /// In mono the receiver mixes both mics, so it (and the audio) can only report the mix.
    var metersAreMix: Bool { receiver.channelMode != .stereo }

    /// 0...1 meter fill for a transmitter. A disarmed mic reads 0 (it is not in the take).
    /// While recording this is real per-channel audio (L = TX1, R = TX2 in stereo); otherwise it is
    /// the receiver's own meter, which is per mic in stereo and the mix in mono.
    func meter(_ index: Int) -> Double {
        guard armed.contains(index), isLinked(index) else { return 0 }
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
        guard !isRecording, !armed.isEmpty else { return }
        lastError = nil
        Task { [weak self] in
            guard let self else { return }
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
            let day = recordingsRoot.appendingPathComponent(now.formatted(.iso8601.year().month().day()), isDirectory: true)
            do {
                try FileManager.default.createDirectory(at: day, withIntermediateDirectories: true)
                let stamp = now.formatted(.dateTime.hour(.twoDigits(amPM: .omitted)).minute(.twoDigits).second(.twoDigits))
                    .replacingOccurrences(of: ":", with: ".")
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

    func stopRecording() {
        guard isRecording else { return }
        recorder.stop()
    }

    private func recordingFinished(_ url: URL, error: Error?) {
        levelTimer?.invalidate()
        levelTimer = nil
        isRecording = false
        recordingLevels = []
        recordingStarted = nil
        lastSavedFolder = currentStem?.deletingLastPathComponent() ?? url.deletingLastPathComponent()
        if let error {
            lastError = "Recording stopped with an error: \(error.localizedDescription)"
        } else if let solo = recordingSolo, let stem = currentStem {
            let out = URL(fileURLWithPath: stem.path + " TX\(solo).\(format.rawValue)")
            Task.detached {
                do { try TrackSplitter.extract(url, channel: solo - 1, to: out) }
                catch { await MainActor.run { self.lastError = "Couldn’t save TX\(solo)’s track: \(error)" } }
                try? FileManager.default.removeItem(at: url)  // the scratch stereo take
            }
        } else if splitTracks, recordingMode == .stereo, format == .wav, let stem = currentStem {
            let left = URL(fileURLWithPath: stem.path + " TX1.wav"), right = URL(fileURLWithPath: stem.path + " TX2.wav")
            Task.detached {
                do { try TrackSplitter.split(url, left: left, right: right) }
                catch { await MainActor.run { self.lastError = "Couldn’t split tracks: \(error)" } }
            }
        }
        currentStem = nil
        recordingSolo = nil
    }

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
        s.receiver.noiseCancelling = false
        s.receiver.connectedTransmitters = [1, 2]
        for (index, id, fw, meter) in [(1, "a1b2c3", "02.03.17.00", 58), (2, "d4e5f6", "02.03.17.00", 41)] {
            var tx = TransmitterStatus(index: index)
            tx.identity.productName = "DJI Mic Mini"
            tx.identity.bluetoothID = id
            tx.identity.firmware = fw
            tx.batteryLevel = 5
            tx.isCharging = false
            tx.meterLevel = meter
            s.transmitters[index] = tx
        }
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

    private func apply(_ new: MicStatus) {
        guard !demo else { return }
        status = new
        knownTransmitters.formUnion(new.receiver.connectedTransmitters)
        if !isConnected, new.receiver.channelMode != nil { isConnected = true }
        if new.receiver.channelMode == nil, new.lastPush == nil { isConnected = false }
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
