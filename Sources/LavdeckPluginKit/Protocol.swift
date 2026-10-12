import Foundation

/// The Lavdeck plugin protocol, version 1.
///
/// A plugin is a folder containing `plugin.json` (a ``PluginManifest``) and an executable. Lavdeck
/// starts the executable for each job, writes one ``PluginRequest`` as JSON to its standard input,
/// and reads newline-delimited ``PluginMessage`` JSON objects from its standard output until it
/// exits. Plugins can be written in any language; see `docs/PLUGINS.md`.
public enum PluginProtocol {
    /// Newest protocol this SDK speaks. Version 2 adds capture plugins (see `Capture.swift`); version 1
    /// plugins keep working unchanged.
    public static let version = 2
    /// Protocol versions Lavdeck accepts in a manifest.
    public static let supported = 1...2
    /// Event sent when a recording has finished and its files (including split tracks) are written.
    public static let takeFinished = "take.finished"
}

// MARK: - Manifest

public struct PluginManifest: Codable, Sendable, Identifiable, Equatable {
    /// Reverse-DNS identifier, e.g. `io.github.example.lavdeck.transcribe`.
    public var id: String
    public var name: String
    public var version: String
    /// Protocol version the plugin speaks. Must be ``PluginProtocol/version``.
    public var `protocol`: Int
    public var summary: String?
    /// Path of the executable, relative to the plugin folder.
    public var executable: String
    public var arguments: [String]?
    /// Events the plugin runs on automatically while enabled, e.g. `take.finished`.
    public var events: [String]?
    /// Actions a user can run on a take from the recordings manager.
    public var actions: [PluginAction]?
    /// Settings Lavdeck renders in Settings › Plugins and passes back with every request.
    public var settings: [PluginSetting]?
    /// Output kinds the plugin can produce, e.g. `transcript`, `audio`.
    public var produces: [String]?
    /// What the plugin is: `post` (default; jobs after a take or on demand), `capture` (records alongside
    /// Lavdeck, see ``CaptureSpec``) or `app` (a companion app with its own windows). A capture plugin may
    /// also handle post events and actions with its main executable.
    public var kind: String?
    /// System permissions the plugin needs, shown before it is switched on: `screen-recording`, `camera`,
    /// `microphone`, `network`.
    public var permissions: [String]?
    /// Oldest Lavdeck version the plugin works with, e.g. `0.2`.
    public var minAppVersion: String?
    /// How to start the plugin's capture process (protocol 2, `kind: capture`).
    public var capture: CaptureSpec?
    /// The companion app to open (protocol 2, `kind: app`).
    public var app: AppSpec?

    public var isCapture: Bool { kind == "capture" && capture != nil }
    public var isApp: Bool { kind == "app" && app != nil }

    public init(id: String, name: String, version: String, protocol: Int = PluginProtocol.version,
                summary: String? = nil, executable: String, arguments: [String]? = nil,
                events: [String]? = nil, actions: [PluginAction]? = nil,
                settings: [PluginSetting]? = nil, produces: [String]? = nil, kind: String? = nil,
                permissions: [String]? = nil, minAppVersion: String? = nil, capture: CaptureSpec? = nil, app: AppSpec? = nil) {
        self.id = id
        self.name = name
        self.version = version
        self.protocol = `protocol`
        self.summary = summary
        self.executable = executable
        self.arguments = arguments
        self.events = events
        self.actions = actions
        self.settings = settings
        self.produces = produces
        self.kind = kind
        self.permissions = permissions
        self.minAppVersion = minAppVersion
        self.capture = capture
        self.app = app
    }

    public static func load(from folder: URL) throws -> PluginManifest {
        let data = try Data(contentsOf: folder.appendingPathComponent("plugin.json"))
        return try JSONDecoder().decode(PluginManifest.self, from: data)
    }
}

/// A companion app a plugin ships (protocol 2, `kind: app`), such as an editor. Lavdeck opens it on a take.
public struct AppSpec: Codable, Sendable, Equatable {
    /// The `.app` bundle, relative to the plugin folder.
    public var bundle: String
    /// URL to open a take with; `{folder}`, `{stem}` and `{id}` are filled in (percent-encoded).
    public var openTake: String?
    /// Button title on a take, e.g. `Open in Studio`.
    public var actionTitle: String?

    public init(bundle: String, openTake: String? = nil, actionTitle: String? = nil) {
        self.bundle = bundle
        self.openTake = openTake
        self.actionTitle = actionTitle
    }

    /// The URL that opens a take in the app.
    public func url(for take: TakeDescriptor) -> URL? {
        guard let template = openTake else { return nil }
        let allowed = CharacterSet.urlQueryAllowed.subtracting(CharacterSet(charactersIn: "&=+?#"))
        func encode(_ s: String) -> String { s.addingPercentEncoding(withAllowedCharacters: allowed) ?? s }
        let filled = template.replacingOccurrences(of: "{folder}", with: encode(take.folder))
            .replacingOccurrences(of: "{stem}", with: encode(take.stem))
            .replacingOccurrences(of: "{id}", with: encode(take.id))
        return URL(string: filled)
    }
}

public struct PluginAction: Codable, Sendable, Identifiable, Equatable {
    public var id: String
    public var title: String
    public var summary: String?

    public init(id: String, title: String, summary: String? = nil) {
        self.id = id
        self.title = title
        self.summary = summary
    }
}

public struct PluginSetting: Codable, Sendable, Identifiable, Equatable {
    public enum Kind: String, Codable, Sendable { case bool, string, number, choice, path }

    public struct Option: Codable, Sendable, Equatable {
        public var value: String
        public var title: String
        public init(value: String, title: String) { self.value = value; self.title = title }
    }

    public var key: String
    public var title: String
    public var type: Kind
    public var `default`: JSONValue?
    public var options: [Option]?
    public var help: String?

    public var id: String { key }

    public init(key: String, title: String, type: Kind, default: JSONValue? = nil,
                options: [Option]? = nil, help: String? = nil) {
        self.key = key
        self.title = title
        self.type = type
        self.default = `default`
        self.options = options
        self.help = help
    }
}

/// A JSON value for plugin settings.
public enum JSONValue: Codable, Sendable, Equatable, CustomStringConvertible {
    case string(String), number(Double), bool(Bool), null

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let b = try? c.decode(Bool.self) { self = .bool(b) }
        else if let n = try? c.decode(Double.self) { self = .number(n) }
        else { self = .string(try c.decode(String.self)) }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .string(let s): try c.encode(s)
        case .number(let n): try c.encode(n)
        case .bool(let b): try c.encode(b)
        case .null: try c.encodeNil()
        }
    }

    public var description: String {
        switch self {
        case .string(let s): s
        case .number(let n): n == n.rounded() ? String(Int(n)) : String(n)
        case .bool(let b): b ? "true" : "false"
        case .null: ""
        }
    }

    public var boolValue: Bool? { if case .bool(let b) = self { b } else { nil } }
    public var stringValue: String? { if case .string(let s) = self { s } else { nil } }
}

// MARK: - Takes

/// One recording ("take"): every file that shares a start-time stem, plus what Lavdeck knows about it.
public struct TakeDescriptor: Codable, Sendable, Equatable, Identifiable {
    /// Stable id: `<day folder>/<stem>`, e.g. `2026-10-11/12.34.54`.
    public var id: String
    /// Folder the take's files live in.
    public var folder: String
    /// Start-time stem shared by the take's files, e.g. `12.34.54`.
    public var stem: String
    public var started: Date?
    public var duration: Double?
    /// `mono` or `stereo` at the time of recording, when known.
    public var channelMode: String?
    public var files: [TakeFile]
    /// Who spoke into which mic. Two entries with mono files means two-speaker mode.
    public var speakers: [TakeSpeaker]

    public init(id: String, folder: String, stem: String, started: Date? = nil, duration: Double? = nil,
                channelMode: String? = nil, files: [TakeFile] = [], speakers: [TakeSpeaker] = []) {
        self.id = id
        self.folder = folder
        self.stem = stem
        self.started = started
        self.duration = duration
        self.channelMode = channelMode
        self.files = files
        self.speakers = speakers
    }

    /// True when each speaker has their own mono track (stereo with split tracks, both mics armed).
    public var isTwoSpeaker: Bool { speakers.filter { $0.file != nil }.count >= 2 }

    /// The best single file holding everyone: stereo, then mix, then the first track.
    public var mainAudio: TakeFile? {
        files.first { $0.role == .stereo } ?? files.first { $0.role == .mix }
            ?? files.first { $0.role == .track }
    }
}

public struct TakeFile: Codable, Sendable, Equatable {
    public enum Role: String, Codable, Sendable {
        /// Stereo take (TX1 left, TX2 right).
        case stereo
        /// Mono take where the receiver mixed both mics.
        case mix
        /// One mic's own mono track.
        case track
        /// Produced by a plugin (transcript, converted voice, …).
        case derived
        /// Lavdeck's own metadata sidecar.
        case metadata
        /// Recorded by a capture plugin alongside the audio (screen, camera, system audio). See `source`
        /// and `offset`.
        case capture
    }

    public var path: String
    public var role: Role
    /// Transmitter index (1 or 2) for `track` files.
    public var mic: Int?
    /// Free-form kind for derived and captured files, e.g. `transcript`, `audio`, `video`.
    public var kind: String?
    /// What a captured or composed video shows: `screen`, `camera`, `system-audio`, `pip`.
    public var source: String?
    /// Seconds after the take's first audio sample that this file's first frame happened (negative when
    /// it started earlier). Set for captured media; use it to line files up.
    public var offset: Double?

    public init(path: String, role: Role, mic: Int? = nil, kind: String? = nil, source: String? = nil, offset: Double? = nil) {
        self.path = path
        self.role = role
        self.mic = mic
        self.kind = kind
        self.source = source
        self.offset = offset
    }
}

public struct TakeSpeaker: Codable, Sendable, Equatable {
    /// `tx1` or `tx2`.
    public var id: String
    public var mic: Int
    /// The user's name for that mic, or `TX1` / `TX2`.
    public var name: String
    /// That mic's own mono track, when the take has one.
    public var file: String?

    public init(id: String, mic: Int, name: String, file: String? = nil) {
        self.id = id
        self.mic = mic
        self.name = name
        self.file = file
    }
}

// MARK: - Requests and messages

/// What Lavdeck sends a plugin on standard input (one JSON object, then EOF).
public struct PluginRequest: Codable, Sendable {
    public var `protocol`: Int
    /// `event` or `action`.
    public var kind: String
    /// Event name (e.g. `take.finished`) or action id.
    public var name: String
    public var take: TakeDescriptor
    public var settings: [String: JSONValue]
    /// Where outputs should be written (the take's folder).
    public var outputDirectory: String
    public var pluginDirectory: String

    public init(kind: String, name: String, take: TakeDescriptor, settings: [String: JSONValue],
                outputDirectory: String, pluginDirectory: String) {
        self.protocol = PluginProtocol.version
        self.kind = kind
        self.name = name
        self.take = take
        self.settings = settings
        self.outputDirectory = outputDirectory
        self.pluginDirectory = pluginDirectory
    }

    /// Reads the request from standard input (for plugins written in Swift).
    public static func readFromStandardInput() throws -> PluginRequest {
        let data = FileHandle.standardInput.readDataToEndOfFile()
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode(PluginRequest.self, from: data)
    }
}

/// One line a plugin writes to standard output.
public struct PluginMessage: Codable, Sendable, Equatable {
    public enum Kind: String, Codable, Sendable { case progress, output, log, done, error }

    public var type: Kind
    /// 0...1 for `progress`.
    public var fraction: Double?
    public var message: String?
    /// File written, for `output`.
    public var path: String?
    /// Output kind, e.g. `transcript`, for `output`.
    public var kind: String?

    public init(type: Kind, fraction: Double? = nil, message: String? = nil, path: String? = nil, kind: String? = nil) {
        self.type = type
        self.fraction = fraction
        self.message = message
        self.path = path
        self.kind = kind
    }

    public static func progress(_ fraction: Double, _ message: String? = nil) -> PluginMessage {
        PluginMessage(type: .progress, fraction: fraction, message: message)
    }
    public static func output(_ path: String, kind: String) -> PluginMessage { PluginMessage(type: .output, path: path, kind: kind) }
    public static func log(_ message: String) -> PluginMessage { PluginMessage(type: .log, message: message) }
    public static func done(_ message: String? = nil) -> PluginMessage { PluginMessage(type: .done, message: message) }
    public static func error(_ message: String) -> PluginMessage { PluginMessage(type: .error, message: message) }

    /// Writes this message as one line on standard output (for plugins written in Swift).
    public func emit() {
        guard let data = try? JSONEncoder().encode(self) else { return }
        FileHandle.standardOutput.write(data + Data("\n".utf8))
    }
}
