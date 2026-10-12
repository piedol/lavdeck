import Darwin
import Foundation

// MARK: - Capture plugins (protocol 2)
//
// A capture plugin records something alongside Lavdeck's audio, such as the screen or a camera. Lavdeck
// starts its capture process when a recording starts and talks to it with JSON lines both ways:
//
//   Lavdeck → plugin (stdin):  {"type":"start", ...}   then later   {"type":"stop"}
//   plugin → Lavdeck (stdout): {"type":"started", ...}, {"type":"output", ...}, {"type":"stopped"}
//
// Timestamps are host-clock seconds (``HostClock``), the same clock AVFoundation and ScreenCaptureKit
// stamp sample buffers with, so Lavdeck can line every file up with the audio to the frame.

/// How Lavdeck starts a plugin's capture process.
public struct CaptureSpec: Codable, Sendable, Equatable {
    /// Executable for capture, relative to the plugin folder. Defaults to the manifest's `executable`.
    public var executable: String?
    /// Arguments for the capture process, e.g. `["--capture"]`.
    public var arguments: [String]?
    /// What the plugin can record, e.g. `["screen", "camera"]`, for display in Lavdeck.
    public var sources: [String]?
    /// Launch the capture process as its own app for macOS permissions, so screen recording and camera
    /// access are granted to the plugin (by its own name) instead of being checked against Lavdeck. The
    /// executable must sit inside a signed `.app` bundle with its own bundle identifier.
    public var ownPermissions: Bool?

    public init(executable: String? = nil, arguments: [String]? = nil, sources: [String]? = nil, ownPermissions: Bool? = nil) {
        self.executable = executable
        self.arguments = arguments
        self.sources = sources
        self.ownPermissions = ownPermissions
    }
}

/// Seconds on the host clock (mach absolute time), shared by Lavdeck, AVFoundation and ScreenCaptureKit.
public enum HostClock {
    private static let timebase: mach_timebase_info_data_t = {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        return info
    }()

    public static func seconds(fromTicks ticks: UInt64) -> Double {
        Double(ticks) * Double(timebase.numer) / Double(timebase.denom) / 1_000_000_000
    }

    public static func now() -> Double { seconds(fromTicks: mach_absolute_time()) }
}

/// The take a capture belongs to.
public struct CaptureTake: Codable, Sendable, Equatable {
    /// Folder the take's files go in.
    public var folder: String
    /// The take's stem, e.g. `14.02.11`. Name every file `<stem> <Something>.<ext>`.
    public var stem: String

    public init(folder: String, stem: String) {
        self.folder = folder
        self.stem = stem
    }
}

/// A line Lavdeck writes to a capture process.
public struct CaptureCommand: Codable, Sendable, Equatable {
    public enum Kind: String, Codable, Sendable { case start, stop }

    public var type: Kind
    public var `protocol`: Int?
    public var take: CaptureTake?
    public var settings: [String: JSONValue]?
    public var pluginDirectory: String?
    /// Host-clock seconds when Lavdeck asked the recording to start.
    public var hostTime: Double?

    public init(type: Kind, take: CaptureTake? = nil, settings: [String: JSONValue]? = nil,
                pluginDirectory: String? = nil, hostTime: Double? = nil) {
        self.type = type
        self.protocol = PluginProtocol.version
        self.take = take
        self.settings = settings
        self.pluginDirectory = pluginDirectory
        self.hostTime = hostTime
    }

    /// Reads commands from standard input, one JSON object per line, until EOF (for Swift plugins).
    public static func lines() -> AsyncThrowingStream<CaptureCommand, Error> {
        AsyncThrowingStream { continuation in
            let thread = Thread {
                let decoder = JSONDecoder()
                while let line = readLine(strippingNewline: true) {
                    guard !line.isEmpty, let data = line.data(using: .utf8) else { continue }
                    do { continuation.yield(try decoder.decode(CaptureCommand.self, from: data)) } catch {
                        continuation.finish(throwing: error)
                        return
                    }
                }
                continuation.finish()
            }
            thread.start()
        }
    }
}

/// A line a capture process writes to Lavdeck.
public struct CaptureEvent: Codable, Sendable, Equatable {
    public enum Kind: String, Codable, Sendable { case started, output, log, error, stopped }

    public var type: Kind
    /// What this is about: `screen`, `camera`, `system-audio`.
    public var source: String?
    /// File written, for `output`.
    public var path: String?
    /// `video` or `audio`, for `output`.
    public var kind: String?
    /// Host-clock seconds of the file's first frame (for `started` and `output`).
    public var startHostTime: Double?
    public var message: String?

    public init(type: Kind, source: String? = nil, path: String? = nil, kind: String? = nil,
                startHostTime: Double? = nil, message: String? = nil) {
        self.type = type
        self.source = source
        self.path = path
        self.kind = kind
        self.startHostTime = startHostTime
        self.message = message
    }

    public static func started(_ source: String, at hostTime: Double) -> CaptureEvent {
        CaptureEvent(type: .started, source: source, startHostTime: hostTime)
    }
    public static func output(_ path: String, source: String, kind: String, startHostTime: Double?) -> CaptureEvent {
        CaptureEvent(type: .output, source: source, path: path, kind: kind, startHostTime: startHostTime)
    }
    public static func log(_ message: String) -> CaptureEvent { CaptureEvent(type: .log, message: message) }
    public static func error(_ message: String, source: String? = nil) -> CaptureEvent {
        CaptureEvent(type: .error, source: source, message: message)
    }
    public static let stopped = CaptureEvent(type: .stopped)

    private static let lock = NSLock()

    /// Writes this event as one line on standard output. Safe to call from any thread.
    public func emit() {
        guard let data = try? JSONEncoder().encode(self) else { return }
        Self.lock.withLock { FileHandle.standardOutput.write(data + Data("\n".utf8)) }
    }
}
