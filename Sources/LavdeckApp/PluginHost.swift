import Foundation
import LavdeckKit
import LavdeckPluginKit
import Observation

/// App-side plugin manager: installed plugins, their settings, and running jobs.
@MainActor
@Observable
final class PluginHost {
    struct Job: Identifiable, Equatable {
        enum State: Equatable { case running, finished(String?), failed(String) }
        let id = UUID()
        let pluginID: String
        let pluginName: String
        let takeID: String
        let title: String
        var fraction: Double = 0
        var message: String?
        var state: State = .running
    }

    private(set) var plugins: [InstalledPlugin] = []
    private(set) var problems: [String] = []
    private(set) var jobs: [Job] = []
    /// Bumped whenever a job writes files, so views listing takes can refresh.
    private(set) var outputsRevision = 0
    /// Bumped on enable/settings changes so views re-read the store.
    private(set) var settingsRevision = 0

    let store = PluginStore(defaults: .standard)
    private var tasks: [UUID: Task<Void, Never>] = [:]

    init() { reload() }

    func reload() {
        let found = store.discover()
        plugins = found.plugins
        problems = found.problems
    }

    var enabledPlugins: [InstalledPlugin] { plugins.filter { store.isEnabled($0.id) } }
    /// Enabled plugins that record alongside the audio.
    var capturePlugins: [InstalledPlugin] { _ = settingsRevision; return enabledPlugins.filter(\.manifest.isCapture) }
    var runningJobs: [Job] { jobs.filter { $0.state == .running } }

    func isEnabled(_ plugin: InstalledPlugin) -> Bool { _ = settingsRevision; return store.isEnabled(plugin.id) }

    func setEnabled(_ plugin: InstalledPlugin, _ on: Bool) {
        store.setEnabled(plugin.id, on)
        settingsRevision += 1
    }

    func value(_ setting: PluginSetting, for plugin: InstalledPlugin) -> JSONValue {
        _ = settingsRevision
        return store.settings(for: plugin)[setting.key] ?? .null
    }

    func setValue(_ value: JSONValue, _ setting: PluginSetting, for plugin: InstalledPlugin) {
        store.setSetting(value, key: setting.key, for: plugin.id)
        settingsRevision += 1
    }

    func install(from folder: URL) throws {
        try store.install(from: folder)
        reload()
    }

    /// Enabled plugins that offer actions, paired with each action.
    var actions: [(plugin: InstalledPlugin, action: PluginAction)] {
        _ = settingsRevision
        return enabledPlugins.flatMap { p in (p.manifest.actions ?? []).map { (p, $0) } }
    }

    func job(for takeID: String, plugin: String? = nil) -> Job? {
        jobs.last { $0.takeID == takeID && $0.state == .running && (plugin == nil || $0.pluginID == plugin) }
    }

    /// Runs every enabled plugin that listens to `take.finished`.
    func takeFinished(_ take: Take) {
        for plugin in enabledPlugins where plugin.handles(event: PluginProtocol.takeFinished) {
            start(plugin, kind: "event", name: PluginProtocol.takeFinished, title: plugin.manifest.name, take: take)
        }
    }

    func run(_ action: PluginAction, of plugin: InstalledPlugin, on take: Take) {
        guard job(for: take.id, plugin: plugin.id) == nil else { return }
        start(plugin, kind: "action", name: action.id, title: action.title, take: take)
    }

    func cancel(_ job: Job) { tasks[job.id]?.cancel() }

    func dismissFinished() { jobs.removeAll { $0.state != .running } }

    private func start(_ plugin: InstalledPlugin, kind: String, name: String, title: String, take: Take) {
        let request = plugin.request(kind: kind, name: name, take: take.descriptor, settings: store.settings(for: plugin))
        let job = Job(pluginID: plugin.id, pluginName: plugin.manifest.name, takeID: take.id, title: title)
        jobs.append(job)
        let id = job.id
        tasks[id] = Task { [weak self] in
            do {
                let result = try await PluginRunner.run(plugin, request: request) { message in
                    Task { @MainActor in self?.update(id, with: message) }
                }
                self?.finish(id, .finished(result.outputs.isEmpty ? nil : "\(result.outputs.count) file(s)"))
            } catch PluginRunError.cancelled {
                self?.finish(id, .failed("Cancelled"))
            } catch {
                self?.finish(id, .failed("\(error)"))
            }
        }
    }

    private func update(_ id: UUID, with message: PluginMessage) {
        guard let i = jobs.firstIndex(where: { $0.id == id }) else { return }
        switch message.type {
        case .progress:
            jobs[i].fraction = message.fraction ?? jobs[i].fraction
            jobs[i].message = message.message ?? jobs[i].message
        case .output: outputsRevision += 1
        case .done: jobs[i].message = message.message ?? jobs[i].message
        case .log, .error: break
        }
    }

    private func finish(_ id: UUID, _ state: Job.State) {
        tasks[id] = nil
        guard let i = jobs.firstIndex(where: { $0.id == id }) else { return }
        jobs[i].state = state
        if case .finished(let note) = state { jobs[i].fraction = 1; jobs[i].message = jobs[i].message ?? note }
        outputsRevision += 1
    }
}
