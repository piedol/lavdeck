import Foundation
import LavdeckPluginKit

/// An installed plugin: its folder and parsed manifest.
public struct InstalledPlugin: Identifiable, Sendable, Equatable {
    public var folder: URL
    public var manifest: PluginManifest
    public var id: String { manifest.id }

    public var executableURL: URL { folder.appendingPathComponent(manifest.executable) }

    public func handles(event: String) -> Bool { manifest.events?.contains(event) == true }

    /// A post-job request in the protocol version this plugin declared, so older plugins keep working.
    public func request(kind: String, name: String, take: TakeDescriptor, settings: [String: JSONValue]) -> PluginRequest {
        var request = PluginRequest(kind: kind, name: name, take: take, settings: settings,
                                    outputDirectory: take.folder, pluginDirectory: folder.path)
        request.protocol = min(manifest.protocol, PluginProtocol.version)
        return request
    }
}

/// Finds plugins and keeps their enabled state and settings in the app's defaults domain, so the
/// app and the `lavdeck` CLI share them.
public final class PluginStore: @unchecked Sendable {
    public static let defaultsSuite = "io.github.piedol.lavdeck"

    /// `~/Library/Application Support/Lavdeck/Plugins`. Each child folder (or symlink) is one plugin.
    public static var pluginsFolder: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Lavdeck/Plugins", isDirectory: true)
    }

    private let defaults: UserDefaults

    public init(defaults: UserDefaults? = nil) {
        self.defaults = defaults ?? UserDefaults(suiteName: Self.defaultsSuite) ?? .standard
    }

    /// Plugins that parse and speak this protocol version. Problems are returned alongside.
    public func discover() -> (plugins: [InstalledPlugin], problems: [String]) {
        let fm = FileManager.default
        try? fm.createDirectory(at: Self.pluginsFolder, withIntermediateDirectories: true)
        let entries = (try? fm.contentsOfDirectory(at: Self.pluginsFolder, includingPropertiesForKeys: nil)) ?? []
        var plugins: [InstalledPlugin] = []
        var problems: [String] = []
        for entry in entries where !entry.lastPathComponent.hasPrefix(".") {
            let folder = entry.resolvingSymlinksInPath()
            do {
                let manifest = try PluginManifest.load(from: folder)
                guard PluginProtocol.supported.contains(manifest.protocol) else {
                    problems.append("\(manifest.name): needs plugin protocol \(manifest.protocol), Lavdeck speaks up to \(PluginProtocol.version)")
                    continue
                }
                plugins.append(InstalledPlugin(folder: folder, manifest: manifest))
            } catch {
                problems.append("\(entry.lastPathComponent): \(error.localizedDescription)")
            }
        }
        return (plugins.sorted { $0.manifest.name < $1.manifest.name }, problems)
    }

    /// Installs a plugin folder by symlinking it into the plugins folder.
    @discardableResult
    public func install(from folder: URL) throws -> InstalledPlugin {
        let manifest = try PluginManifest.load(from: folder)
        let link = Self.pluginsFolder.appendingPathComponent(manifest.id)
        try FileManager.default.createDirectory(at: Self.pluginsFolder, withIntermediateDirectories: true)
        if FileManager.default.fileExists(atPath: link.path) || (try? link.checkResourceIsReachable()) == false {
            try? FileManager.default.removeItem(at: link)
        }
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: folder)
        return InstalledPlugin(folder: folder, manifest: manifest)
    }

    public func isEnabled(_ id: String) -> Bool {
        (defaults.dictionary(forKey: "plugins.enabled") as? [String: Bool])?[id] ?? false
    }

    public func setEnabled(_ id: String, _ enabled: Bool) {
        var all = (defaults.dictionary(forKey: "plugins.enabled") as? [String: Bool]) ?? [:]
        all[id] = enabled
        defaults.set(all, forKey: "plugins.enabled")
    }

    /// Effective settings: the manifest defaults overlaid with the user's values.
    public func settings(for plugin: InstalledPlugin) -> [String: JSONValue] {
        var values: [String: JSONValue] = [:]
        for setting in plugin.manifest.settings ?? [] { values[setting.key] = setting.default ?? .null }
        let stored = defaults.dictionary(forKey: "plugins.settings.\(plugin.id)") ?? [:]
        for (key, raw) in stored {
            switch raw {
            case let b as Bool: values[key] = .bool(b)
            case let n as NSNumber: values[key] = .number(n.doubleValue)
            case let s as String: values[key] = .string(s)
            default: break
            }
        }
        return values
    }

    public func setSetting(_ value: JSONValue, key: String, for pluginID: String) {
        var stored = defaults.dictionary(forKey: "plugins.settings.\(pluginID)") ?? [:]
        switch value {
        case .bool(let b): stored[key] = b
        case .number(let n): stored[key] = n
        case .string(let s): stored[key] = s
        case .null: stored[key] = nil
        }
        defaults.set(stored, forKey: "plugins.settings.\(pluginID)")
    }
}

public enum PluginRunError: Error, CustomStringConvertible {
    case launch(String), failed(String), cancelled

    public var description: String {
        switch self {
        case .launch(let m): "Couldn’t start the plugin: \(m)"
        case .failed(let m): m
        case .cancelled: "Cancelled"
        }
    }
}

/// Runs one plugin job: launch the executable, send the request, stream its messages.
public enum PluginRunner {
    public struct Result: Sendable {
        public var outputs: [PluginMessage]
        public var log: [String]
    }

    private static let ignoreSIGPIPE: Void = { signal(SIGPIPE, SIG_IGN) }()
    /// Shared with ``CaptureHost``: a plugin that exits early must not take Lavdeck down with SIGPIPE.
    static var ignoreSIGPIPEOnce: Void { ignoreSIGPIPE }

    public static func run(_ plugin: InstalledPlugin, request: PluginRequest,
                           onMessage: @escaping @Sendable (PluginMessage) -> Void = { _ in }) async throws -> Result {
        let process = Process()
        process.executableURL = plugin.executableURL
        process.arguments = plugin.manifest.arguments ?? []
        process.currentDirectoryURL = plugin.folder
        var env = ProcessInfo.processInfo.environment
        env["LAVDECK_PLUGIN_PROTOCOL"] = String(PluginProtocol.version)
        env["LAVDECK_PLUGIN_DIR"] = plugin.folder.path
        for (key, value) in request.settings {
            env["LAVDECK_SETTING_\(key.uppercased().replacingOccurrences(of: "-", with: "_"))"] = value.description
        }
        process.environment = env
        let stdin = Pipe(), stdout = Pipe(), stderr = Pipe()
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = stderr

        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        let body = try encoder.encode(request)

        let collector = Collector(onMessage: onMessage)
        stdout.fileHandleForReading.readabilityHandler = { handle in collector.feed(handle.availableData) }
        stderr.fileHandleForReading.readabilityHandler = { handle in collector.feedLog(handle.availableData) }

        // Attach the exit signal before launch so a plugin that exits instantly can't be missed.
        let (exited, exitSignal) = AsyncStream<Void>.makeStream()
        process.terminationHandler = { _ in exitSignal.yield(); exitSignal.finish() }

        // A plugin that exits without reading stdin must not take the host down with SIGPIPE.
        _ = ignoreSIGPIPE
        do { try process.run() } catch { throw PluginRunError.launch(error.localizedDescription) }
        try? stdin.fileHandleForWriting.write(contentsOf: body)
        try? stdin.fileHandleForWriting.close()

        await withTaskCancellationHandler {
            for await _ in exited { break }
        } onCancel: {
            if process.isRunning { process.terminate() }
        }
        stdout.fileHandleForReading.readabilityHandler = nil
        stderr.fileHandleForReading.readabilityHandler = nil
        collector.feed(stdout.fileHandleForReading.readDataToEndOfFile())
        collector.flush()

        if Task.isCancelled { throw PluginRunError.cancelled }
        let snapshot = collector.snapshot()
        if let error = snapshot.messages.last(where: { $0.type == .error }) {
            throw PluginRunError.failed(error.message ?? "The plugin reported an error")
        }
        guard process.terminationStatus == 0 else {
            let tail = snapshot.log.suffix(5).joined(separator: "\n")
            throw PluginRunError.failed("The plugin exited with status \(process.terminationStatus).\(tail.isEmpty ? "" : "\n\(tail)")")
        }
        return Result(outputs: snapshot.messages.filter { $0.type == .output }, log: snapshot.log)
    }

    /// Thread-safe line splitter for the plugin's stdout/stderr.
    final class Collector: @unchecked Sendable {
        private let lock = NSLock()
        private var buffer = Data()
        private var messages: [PluginMessage] = []
        private var log: [String] = []
        private let onMessage: @Sendable (PluginMessage) -> Void

        init(onMessage: @escaping @Sendable (PluginMessage) -> Void) { self.onMessage = onMessage }

        func feed(_ data: Data) {
            guard !data.isEmpty else { return }
            let lines: [Data] = lock.withLock {
                buffer.append(data)
                var out: [Data] = []
                while let nl = buffer.firstIndex(of: 0x0A) {
                    out.append(buffer[buffer.startIndex..<nl])
                    buffer.removeSubrange(buffer.startIndex...nl)
                }
                return out
            }
            lines.forEach(handle)
        }

        func flush() {
            let rest: Data = lock.withLock { defer { buffer.removeAll() }; return buffer }
            if !rest.isEmpty { handle(rest) }
        }

        func feedLog(_ data: Data) {
            guard !data.isEmpty, let text = String(data: data, encoding: .utf8) else { return }
            lock.withLock { log.append(contentsOf: text.split(separator: "\n").map(String.init)) }
        }

        private func handle(_ line: Data) {
            guard !line.isEmpty else { return }
            if let message = try? JSONDecoder().decode(PluginMessage.self, from: line) {
                lock.withLock { messages.append(message) }
                onMessage(message)
            } else if let text = String(data: line, encoding: .utf8) {
                lock.withLock { log.append(text) }
            }
        }

        func snapshot() -> (messages: [PluginMessage], log: [String]) { lock.withLock { (messages, log) } }
    }
}
