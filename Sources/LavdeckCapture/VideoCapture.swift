import AppKit
import Foundation
import LavdeckKit
import LavdeckPluginKit

/// Records the screen and camera for one take, alongside Lavdeck's audio, with the camera preview.
@MainActor
public final class VideoCapture {
    /// Sources that have written their first frame (`screen`, `camera`).
    public private(set) var started: [String] = []
    /// Sources that couldn't start, with why.
    public private(set) var failed: [String: String] = [:]
    private var screen: ScreenRecorder?
    private var camera: CameraRecorder?
    private var bubble: CameraBubble?
    private var finished = false
    private let onEvent: @MainActor () -> Void

    /// `onEvent` runs whenever a source starts or fails, e.g. to update the deck.
    public init(onEvent: @escaping @MainActor () -> Void = {}) {
        self.onEvent = onEvent
    }

    /// The sources `settings` will try to record, given the permissions Lavdeck holds.
    public static func expected(_ settings: VideoSettings, granted: Set<String>) -> Set<String> {
        var out: Set<String> = []
        if settings.camera && granted.contains("camera") { out.insert("camera") }
        if settings.screen && granted.contains("screen-recording") { out.insert("screen") }
        return out
    }

    /// Starts the camera (and its preview) and the screen. Returns once each has started or failed to; use
    /// ``started`` to wait for first frames.
    public func start(folder: URL, stem: String, settings: VideoSettings, granted: Set<String>) async {
        if settings.camera && !granted.contains("camera") {
            fail("camera", "Camera access is off for Lavdeck, so the camera wasn’t recorded.")
        }
        if settings.screen && !granted.contains("screen-recording") {
            fail("screen", "Screen recording is off for Lavdeck, so the screen wasn’t recorded.")
        }
        if settings.camera && granted.contains("camera") {
            let url = folder.appendingPathComponent("\(stem) Camera.mov")
            do {
                let recorder = try await CameraRecorder.make(url: url, hevc: settings.hevc)
                recorder.onStarted = { [weak self] _ in Task { @MainActor in self?.didStart("camera") } }
                camera = recorder
                if settings.cameraPreview {
                    bubble = CameraBubble(session: recorder.session, size: CameraBubble.Size(rawValue: settings.previewSize.rawValue) ?? .bubble)
                    bubble?.show()
                }
                recorder.start()
            } catch {
                fail("camera", "\(error)")
            }
        }
        if settings.screen && granted.contains("screen-recording") {
            let url = folder.appendingPathComponent("\(stem) Screen.mov")
            do {
                let recorder = ScreenRecorder(url: url, fps: settings.screenFPS, hevc: settings.hevc, systemAudio: settings.systemAudio)
                recorder.onStarted = { [weak self] _ in Task { @MainActor in self?.didStart("screen") } }
                try await recorder.start(excludingWindowNumbers: bubble.map { [$0.windowNumber] } ?? [])
                screen = recorder
            } catch {
                fail("screen", "\(error)")
            }
        }
    }

    /// Stops everything and returns the files written. Safe to call more than once.
    public func stop() async -> (outputs: [CaptureOutput], problems: [CaptureHost.Problem]) {
        guard !finished else { return ([], []) }
        finished = true
        bubble?.close()
        bubble = nil
        async let screenResult = screen?.stop()
        async let cameraResult = camera?.stop()
        var outputs: [CaptureOutput] = []
        var problems: [CaptureHost.Problem] = []
        for result in await [screenResult, cameraResult] {
            switch result {
            case .success(let file)?:
                outputs.append(CaptureOutput(pluginID: "lavdeck", path: file.url.path, source: file.source, kind: "video",
                                             startHostTime: file.startHostTime))
            case .failure(let error)?:
                problems.append(CaptureHost.Problem(pluginName: "Video", message: "\(error)"))
            case nil:
                break
            }
        }
        return (outputs, problems)
    }

    private func didStart(_ source: String) {
        if !started.contains(source) { started.append(source) }
        onEvent()
    }

    private func fail(_ source: String, _ message: String) {
        failed[source] = message
        onEvent()
    }
}

struct RecordedFile {
    var url: URL
    var source: String
    var startHostTime: Double?
}

enum CaptureError: Error, CustomStringConvertible {
    case screenPermission, cameraPermission, noCamera, noDisplay, nothingRecorded(String), writer(String)

    var description: String {
        switch self {
        case .screenPermission:
            "Screen recording is off for Lavdeck. Turn on Lavdeck in System Settings › Privacy & Security › Screen & System Audio Recording, then reopen Lavdeck."
        case .cameraPermission:
            "Camera access is off for Lavdeck. Turn it on in System Settings › Privacy & Security › Camera."
        case .noCamera: "No camera found."
        case .noDisplay: "No display found to record."
        case .nothingRecorded(let what): "No \(what) frames arrived, so nothing was saved."
        case .writer(let message): "Couldn’t write the video: \(message)"
        }
    }
}
