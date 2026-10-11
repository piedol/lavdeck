import AVFoundation
import Foundation
import LavdeckPluginKit

/// Lavdeck's per-take metadata, written as `<stem>.take.json` when a recording finishes.
public struct TakeMetadata: Codable, Sendable, Equatable {
    public struct Mic: Codable, Sendable, Equatable {
        public var index: Int
        public var name: String
        public var bluetoothID: String?
        public var armed: Bool
        public init(index: Int, name: String, bluetoothID: String?, armed: Bool) {
            self.index = index
            self.name = name
            self.bluetoothID = bluetoothID
            self.armed = armed
        }
    }

    public var version = 1
    public var started: Date
    public var duration: Double?
    public var channelMode: String?
    public var format: String
    public var mics: [Mic]

    public init(started: Date, duration: Double?, channelMode: String?, format: String, mics: [Mic]) {
        self.started = started
        self.duration = duration
        self.channelMode = channelMode
        self.format = format
        self.mics = mics
    }

    private enum CodingKeys: String, CodingKey { case version, started, duration, channelMode, format, mics }

    /// Tolerant of missing optional keys, so a hand-written or older file still yields its mic names.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
        started = try c.decode(Date.self, forKey: .started)
        duration = try c.decodeIfPresent(Double.self, forKey: .duration)
        channelMode = try c.decodeIfPresent(String.self, forKey: .channelMode)
        format = try c.decodeIfPresent(String.self, forKey: .format) ?? "wav"
        mics = try c.decodeIfPresent([Mic].self, forKey: .mics) ?? []
    }

    public func write(to url: URL) throws {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(self).write(to: url, options: .atomic)
    }

    public static func load(_ url: URL) -> TakeMetadata? {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return (try? Data(contentsOf: url)).flatMap { try? decoder.decode(TakeMetadata.self, from: $0) }
    }
}

/// A recording on disk.
public struct Take: Identifiable, Sendable, Equatable {
    public var descriptor: TakeDescriptor
    public var metadata: TakeMetadata?
    public var id: String { descriptor.id }

    public var transcriptURL: URL {
        URL(fileURLWithPath: descriptor.folder).appendingPathComponent("\(descriptor.stem).transcript.json")
    }
    public var hasTranscript: Bool { FileManager.default.fileExists(atPath: transcriptURL.path) }
    public var audioFiles: [TakeFile] { descriptor.files.filter { [.stereo, .mix, .track].contains($0.role) } }
    public var allURLs: [URL] { descriptor.files.map { URL(fileURLWithPath: $0.path) } }
}

/// Scans a recordings root (`<root>/<yyyy-MM-dd>/<HH.mm.ss> …`) into takes.
public enum TakeLibrary {
    static let stemPattern = try! NSRegularExpression(pattern: "^(\\d{2}\\.\\d{2}\\.\\d{2})")

    public static func scan(root: URL, micNames: [String: String] = [:]) -> [Take] {
        let fm = FileManager.default
        guard let days = try? fm.contentsOfDirectory(at: root, includingPropertiesForKeys: nil) else { return [] }
        var takes: [Take] = []
        for day in days where (try? day.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true {
            guard let names = try? fm.contentsOfDirectory(atPath: day.path) else { continue }
            var byStem: [String: [String]] = [:]
            for name in names where !name.hasPrefix(".") {
                let range = NSRange(name.startIndex..., in: name)
                guard let m = stemPattern.firstMatch(in: name, range: range), let r = Range(m.range(at: 1), in: name) else { continue }
                byStem[String(name[r]), default: []].append(name)
            }
            for (stem, files) in byStem {
                if let take = take(folder: day, stem: stem, fileNames: files, micNames: micNames), !take.audioFiles.isEmpty {
                    takes.append(take)
                }
            }
        }
        return takes.sorted { ($0.descriptor.started ?? .distantPast) > ($1.descriptor.started ?? .distantPast) }
    }

    /// Builds one take from its folder and stem (used right after recording and by `scan`).
    public static func take(folder: URL, stem: String, fileNames: [String]? = nil, micNames: [String: String] = [:]) -> Take? {
        let names = fileNames ?? ((try? FileManager.default.contentsOfDirectory(atPath: folder.path)) ?? [])
            .filter { $0.hasPrefix(stem) }
        let metadata = TakeMetadata.load(folder.appendingPathComponent("\(stem).take.json"))
        var files: [TakeFile] = []
        for name in names.sorted() {
            let path = folder.appendingPathComponent(name).path
            let rest = String(name.dropFirst(stem.count)).trimmingCharacters(in: .whitespaces)
            let ext = (name as NSString).pathExtension.lowercased()
            if name.hasSuffix(".take.json") {
                files.append(TakeFile(path: path, role: .metadata))
            } else if ["wav", "m4a", "caf", "aif", "aiff"].contains(ext) {
                let base = (rest as NSString).deletingPathExtension
                if base == "Stereo" { files.append(TakeFile(path: path, role: .stereo)) }
                else if base == "Mix" { files.append(TakeFile(path: path, role: .mix)) }
                else if base == "TX1" { files.append(TakeFile(path: path, role: .track, mic: 1)) }
                else if base == "TX2" { files.append(TakeFile(path: path, role: .track, mic: 2)) }
                else { files.append(TakeFile(path: path, role: .derived, mic: base.hasPrefix("TX2") ? 2 : base.hasPrefix("TX1") ? 1 : nil, kind: "audio")) }
            } else if name.contains(".transcript.") {
                files.append(TakeFile(path: path, role: .derived, kind: "transcript"))
            } else {
                files.append(TakeFile(path: path, role: .derived, kind: ext))
            }
        }
        guard !files.isEmpty else { return nil }

        let started = metadata?.started ?? startDate(day: folder.lastPathComponent, stem: stem)
        let main = files.first { $0.role == .stereo } ?? files.first { $0.role == .mix } ?? files.first { $0.role == .track }
        let duration = metadata?.duration ?? main.flatMap { audioDuration(URL(fileURLWithPath: $0.path)) }
        let mode = metadata?.channelMode ?? (files.contains { $0.role == .stereo } ? "stereo" : files.contains { $0.role == .mix } ? "mono" : nil)

        var speakers: [TakeSpeaker] = []
        for index in [1, 2] {
            let track = files.first { $0.role == .track && $0.mic == index }
            let meta = metadata?.mics.first { $0.index == index }
            let name = meta?.name ?? meta?.bluetoothID.flatMap { micNames[$0] } ?? "TX\(index)"
            if track != nil || meta?.armed == true {
                speakers.append(TakeSpeaker(id: "tx\(index)", mic: index, name: name, file: track?.path))
            }
        }
        let descriptor = TakeDescriptor(id: "\(folder.lastPathComponent)/\(stem)", folder: folder.path, stem: stem,
                                        started: started, duration: duration, channelMode: mode,
                                        files: files, speakers: speakers)
        return Take(descriptor: descriptor, metadata: metadata)
    }

    static func startDate(day: String, stem: String) -> Date? {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd HH.mm.ss"
        return f.date(from: "\(day) \(stem)")
    }

    static func audioDuration(_ url: URL) -> Double? {
        guard let file = try? AVAudioFile(forReading: url) else { return nil }
        return Double(file.length) / file.fileFormat.sampleRate
    }

    /// Moves every file of a take to the Trash (recoverable).
    public static func trash(_ take: Take) throws {
        for url in take.allURLs where FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.trashItem(at: url, resultingItemURL: nil)
        }
    }
}
