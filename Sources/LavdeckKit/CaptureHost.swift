import Foundation
import LavdeckPluginKit

/// One file a capture plugin recorded for a take.
public struct CaptureOutput: Sendable, Equatable {
    public var pluginID: String
    public var path: String
    public var source: String
    public var kind: String
    /// Host-clock seconds of the file's first frame.
    public var startHostTime: Double?
}

/// Runs every enabled capture plugin for one recording: start them together, stop them together,
/// collect what they wrote.
public final class CaptureHost: @unchecked Sendable {
    public struct Problem: Sendable, Equatable {
        public var pluginName: String
        public var message: String
    }

    private final class Running {
        let plugin: InstalledPlugin
        let process: SpawnedProcess
        let exited: AsyncStream<Void>
        let collector: Collector
        init(plugin: InstalledPlugin, process: SpawnedProcess, exited: AsyncStream<Void>, collector: Collector) {
            self.plugin = plugin
            self.process = process
            self.exited = exited
            self.collector = collector
        }
    }

    private var running: [Running] = []
    private let onEvent: @Sendable (InstalledPlugin, CaptureEvent) -> Void

    /// `onEvent` sees every event as it arrives (for status and errors in the UI), on a background thread.
    public init(onEvent: @escaping @Sendable (InstalledPlugin, CaptureEvent) -> Void = { _, _ in }) {
        self.onEvent = onEvent
    }

    public var isRunning: Bool { !running.isEmpty }

    /// Starts each plugin's capture process and sends it `start`. Plugins that fail to launch are
    /// reported and skipped; the recording goes ahead without them.
    public func start(_ plugins: [(plugin: InstalledPlugin, settings: [String: JSONValue])], take: CaptureTake) -> [Problem] {
        var problems: [Problem] = []
        let hostTime = HostClock.now()
        for (plugin, settings) in plugins where plugin.manifest.isCapture {
            let spec = plugin.manifest.capture!
            var env = ProcessInfo.processInfo.environment
            env["LAVDECK_PLUGIN_PROTOCOL"] = String(PluginProtocol.version)
            env["LAVDECK_PLUGIN_DIR"] = plugin.folder.path
            _ = PluginRunner.ignoreSIGPIPEOnce
            let process: SpawnedProcess
            do {
                process = try SpawnedProcess(executable: plugin.folder.appendingPathComponent(spec.executable ?? plugin.manifest.executable),
                                             arguments: spec.arguments ?? [], environment: env, directory: plugin.folder,
                                             disclaimResponsibility: spec.ownPermissions == true)
            } catch {
                problems.append(Problem(pluginName: plugin.manifest.name, message: "Couldn’t start: \(error)"))
                continue
            }
            let collector = Collector(plugin: plugin, onEvent: onEvent)
            let stdout = process.stdout, stderr = process.stderr
            stdout.readabilityHandler = { collector.feed($0.availableData) }
            stderr.readabilityHandler = { collector.feedLog($0.availableData) }
            let (exited, signal) = AsyncStream<Void>.makeStream()
            process.onExit { _ in
                stdout.readabilityHandler = nil
                stderr.readabilityHandler = nil
                collector.feed(stdout.readDataToEndOfFile())
                collector.flush()
                signal.yield()
                signal.finish()
            }
            let command = CaptureCommand(type: .start, take: take, settings: settings,
                                         pluginDirectory: plugin.folder.path, hostTime: hostTime)
            write(command, to: process.stdin)
            running.append(Running(plugin: plugin, process: process, exited: exited, collector: collector))
        }
        return problems
    }

    /// Sends `stop` to every capture process and waits (up to `timeout` seconds each) for them to finish
    /// writing. Returns the files they reported and any problems.
    public func stop(timeout: Double = 30) async -> (outputs: [CaptureOutput], problems: [Problem]) {
        let all = running
        running = []
        for r in all {
            write(CaptureCommand(type: .stop), to: r.process.stdin)
            try? r.process.stdin.close()
        }
        var outputs: [CaptureOutput] = []
        var problems: [Problem] = []
        for r in all {
            let exited = r.exited
            let finished = await withTaskGroup(of: Bool.self) { group in
                group.addTask { for await _ in exited { break }; return true }
                group.addTask { try? await Task.sleep(for: .seconds(timeout)); return false }
                let first = await group.next() ?? false
                group.cancelAll()
                return first
            }
            if !finished {
                r.process.terminate()
                problems.append(Problem(pluginName: r.plugin.manifest.name, message: "Didn’t finish writing within \(Int(timeout)) seconds and was stopped."))
            }
            let snapshot = r.collector.snapshot()
            outputs += snapshot.outputs
            problems += snapshot.errors.map { Problem(pluginName: r.plugin.manifest.name, message: $0) }
            if finished, let status = r.process.terminationStatus, status != 0, snapshot.errors.isEmpty {
                let tail = snapshot.log.suffix(3).joined(separator: " ")
                problems.append(Problem(pluginName: r.plugin.manifest.name,
                                        message: "Exited with status \(status). \(tail)"))
            }
        }
        return (outputs, problems)
    }

    private func write(_ command: CaptureCommand, to handle: FileHandle) {
        guard let data = try? JSONEncoder().encode(command) else { return }
        try? handle.write(contentsOf: data + Data("\n".utf8))
    }

    /// Splits a capture process's stdout into events.
    final class Collector: @unchecked Sendable {
        private let lock = NSLock()
        private var buffer = Data()
        private var outputs: [CaptureOutput] = []
        private var errors: [String] = []
        private var log: [String] = []
        private let plugin: InstalledPlugin
        private let onEvent: @Sendable (InstalledPlugin, CaptureEvent) -> Void

        init(plugin: InstalledPlugin, onEvent: @escaping @Sendable (InstalledPlugin, CaptureEvent) -> Void) {
            self.plugin = plugin
            self.onEvent = onEvent
        }

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
            guard let event = try? JSONDecoder().decode(CaptureEvent.self, from: line) else {
                if let text = String(data: line, encoding: .utf8) { lock.withLock { log.append(text) } }
                return
            }
            lock.withLock {
                switch event.type {
                case .output:
                    if let path = event.path {
                        outputs.append(CaptureOutput(pluginID: plugin.id, path: path, source: event.source ?? "video",
                                                     kind: event.kind ?? "video", startHostTime: event.startHostTime))
                    }
                case .error: errors.append(event.message ?? "Unknown error")
                case .log: log.append(event.message ?? "")
                case .started, .stopped: break
                }
            }
            onEvent(plugin, event)
        }

        func snapshot() -> (outputs: [CaptureOutput], errors: [String], log: [String]) {
            lock.withLock { (outputs, errors, log) }
        }
    }
}
