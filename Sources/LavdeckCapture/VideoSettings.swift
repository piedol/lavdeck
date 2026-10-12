import Foundation

/// How Lavdeck records video alongside the mics, and how it makes each take's finished video.
public struct VideoSettings: Codable, Sendable, Equatable {
    public enum PreviewSize: String, Codable, Sendable, CaseIterable { case bubble, large, full }
    public enum Codec: String, Codable, Sendable, CaseIterable { case hevc, h264 }
    public enum Corner: String, Codable, Sendable, CaseIterable { case bottomRight = "bottom-right", bottomLeft = "bottom-left", topRight = "top-right", topLeft = "top-left" }
    public enum BubbleSize: String, Codable, Sendable, CaseIterable { case small, medium, large }
    public enum BubbleShape: String, Codable, Sendable, CaseIterable { case circle, rounded }

    /// Record the main display.
    public var screen = false
    /// Record the default camera.
    public var camera = false
    /// Show a floating camera preview while recording (never in the screen recording).
    public var cameraPreview = true
    public var previewSize = PreviewSize.bubble
    /// Record system audio into the screen recording.
    public var systemAudio = false
    public var screenFPS = 30
    public var codec = Codec.hevc
    /// Make `<stem> PiP.mp4` after each take: the screen with the camera in a corner, and the mic audio.
    public var makeVideo = true
    public var bubbleCorner = Corner.bottomRight
    public var bubbleSize = BubbleSize.medium
    public var bubbleShape = BubbleShape.circle

    public init() {}

    public var hevc: Bool { codec == .hevc }
    public var recordsVideo: Bool { screen || camera }

    private static let key = "video.settings"

    public static func load(_ defaults: UserDefaults = .standard) -> VideoSettings {
        if let data = defaults.data(forKey: key), let saved = try? JSONDecoder().decode(VideoSettings.self, from: data) { return saved }
        return migrated(defaults) ?? VideoSettings()
    }

    public func save(_ defaults: UserDefaults = .standard) {
        if let data = try? JSONEncoder().encode(self) { defaults.set(data, forKey: Self.key) }
    }

    /// Settings from the Screen & Camera plugin this replaces, so switching to the built-in keeps people's choices.
    static func migrated(_ defaults: UserDefaults) -> VideoSettings? {
        let id = "io.github.piedol.lavdeck.capture"
        let enabled = (defaults.dictionary(forKey: "plugins.enabled") as? [String: Bool])?[id] ?? false
        guard let old = defaults.dictionary(forKey: "plugins.settings.\(id)") ?? (enabled ? [:] : nil) else { return nil }
        var s = VideoSettings()
        func bool(_ k: String, _ fallback: Bool) -> Bool { old[k] as? Bool ?? fallback }
        func string(_ k: String) -> String? { old[k] as? String ?? (old[k] as? NSNumber).map { "\($0.intValue)" } }
        // A switched-off plugin recorded nothing; keep it that way.
        s.screen = enabled && bool("screen", true)
        s.camera = enabled && bool("camera", true)
        s.cameraPreview = bool("camera_preview", true)
        s.previewSize = string("camera_preview_size").flatMap(PreviewSize.init(rawValue:)) ?? .bubble
        s.systemAudio = bool("system_audio", false)
        s.screenFPS = string("fps").flatMap(Int.init) ?? 30
        s.codec = string("codec").flatMap(Codec.init(rawValue:)) ?? .hevc
        s.makeVideo = bool("pip", true)
        s.bubbleCorner = string("pip_corner").flatMap(Corner.init(rawValue:)) ?? .bottomRight
        s.bubbleSize = string("pip_size").flatMap(BubbleSize.init(rawValue:)) ?? .medium
        s.bubbleShape = string("pip_shape").flatMap(BubbleShape.init(rawValue:)) ?? .circle
        return s
    }
}
