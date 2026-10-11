import Foundation

/// The standard transcript format. Transcription plugins write it as `<stem>.transcript.json` next
/// to the take; the recordings manager reads it for viewing, search and playback.
public struct TranscriptDocument: Codable, Sendable, Equatable {
    public var version: Int
    /// Take id, e.g. `2026-10-11/12.34.54`.
    public var take: String
    public var model: String
    public var language: String?
    public var created: Date
    public var speakers: [Speaker]
    public var segments: [Segment]

    public struct Speaker: Codable, Sendable, Equatable {
        public var id: String
        public var name: String
        public init(id: String, name: String) { self.id = id; self.name = name }
    }

    public struct Segment: Codable, Sendable, Equatable, Identifiable {
        public var start: Double
        public var end: Double
        public var speaker: String
        public var text: String
        public var words: [Word]?
        public var id: String { "\(speaker)@\(start)" }

        public init(start: Double, end: Double, speaker: String, text: String, words: [Word]? = nil) {
            self.start = start
            self.end = end
            self.speaker = speaker
            self.text = text
            self.words = words
        }
    }

    public struct Word: Codable, Sendable, Equatable {
        public var start: Double
        public var end: Double
        public var text: String
        public init(start: Double, end: Double, text: String) { self.start = start; self.end = end; self.text = text }
    }

    public init(version: Int = 1, take: String, model: String, language: String? = nil, created: Date = Date(),
                speakers: [Speaker], segments: [Segment]) {
        self.version = version
        self.take = take
        self.model = model
        self.language = language
        self.created = created
        self.speakers = speakers
        self.segments = segments
    }

    public func speakerName(_ id: String) -> String { speakers.first { $0.id == id }?.name ?? id }

    public var plainText: String { segments.map(\.text).joined(separator: " ") }

    public static func load(_ url: URL) throws -> TranscriptDocument {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode(TranscriptDocument.self, from: Data(contentsOf: url))
    }

    public static func timestamp(_ seconds: Double) -> String {
        let s = Int(seconds.rounded(.down))
        return String(format: "%02d:%02d:%02d", s / 3600, s / 60 % 60, s % 60)
    }
}
