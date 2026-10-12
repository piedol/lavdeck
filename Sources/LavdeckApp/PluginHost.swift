import AppKit
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
    var problems: [String] = []
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
    /// Enabled plugins that ship a companion app (an editor, say).
    var appPlugins: [InstalledPlugin] { _ = settingsRevision; return enabledPlugins.filter(\.manifest.isApp) }

    /// Opens a take in an app plugin.
    func open(_ take: Take, in plugin: InstalledPlugin) {
        guard let spec = plugin.manifest.app, let url = spec.url(for: take.descriptor) else { return }
        let bundle = plugin.folder.appendingPathComponent(spec.bundle)
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        NSWorkspace.shared.open([url], withApplicationAt: bundle, configuration: configuration) { _, error in
            if let error { Task { @MainActor in self.problems.append("\(plugin.manifest.name): \(error.localizedDescription)") } }
        }
    }

    /// A capture source the deck can switch on and off: a source listed in `capture.sources` that has a
    /// bool setting with the same key (the Screen & Camera plugin's `screen` and `camera`).
    struct CaptureToggle: Identifiable, Equatable {
        let plugin: InstalledPlugin
        let source: String
        var id: String { "\(plugin.id)/\(source)" }
        static func == (a: Self, b: Self) -> Bool { a.id == b.id }
    }

    /// Toggles for every installed capture plugin, enabled or not, so one click can switch a plugin on.
    var captureToggles: [CaptureToggle] {
        plugins.filter(\.manifest.isCapture).flatMap { plugin in
            let bools = Set((plugin.manifest.settings ?? []).filter { $0.type == .bool }.map(\.key))
            return (plugin.manifest.capture?.sources ?? []).filter(bools.contains).map { CaptureToggle(plugin: plugin, source: $0) }
        }
    }

    /// On when the plugin is enabled and its setting for this source is on.
    func isOn(_ toggle: CaptureToggle) -> Bool {
        isEnabled(toggle.plugin) && store.settings(for: toggle.plugin)[toggle.source]?.boolValue == true
    }

    func setOn(_ toggle: CaptureToggle, _ on: Bool) {
        if on, !store.isEnabled(toggle.plugin.id) {
            // Switching a plugin on from the deck records just the source that was clicked.
            for other in captureToggles where other.plugin.id == toggle.plugin.id {
                store.setSetting(.bool(false), key: other.source, for: other.plugin.id)
            }
            store.setEnabled(toggle.plugin.id, true)
        }
        store.setSetting(.bool(on), key: toggle.source, for: toggle.plugin.id)
        settingsRevision += 1
    }

    /// Enabled plugins that record alongside the audio, leaving out any whose deck toggles are all off.
    var capturePlugins: [InstalledPlugin] {
        _ = settingsRevision
        let toggles = captureToggles
        return enabledPlugins.filter(\.manifest.isCapture).filter { plugin in
            let own = toggles.filter { $0.plugin.id == plugin.id }
            return own.isEmpty || own.contains(where: isOn)
        }
    }

    /// The permissions `plugin` needs for the sources it will record: a source switched off doesn't need its permission.
    func neededPermissions(_ plugin: InstalledPlugin) -> [String] {
        let off = captureToggles.filter { $0.plugin.id == plugin.id && !isOn($0) }
            .compactMap { PermissionCenter.permission(forSource: $0.source) }
        return (plugin.manifest.permissions ?? []).filter { !off.contains($0) }
    }
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
