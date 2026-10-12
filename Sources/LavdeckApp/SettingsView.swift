import AppKit
import LavdeckKit
import LavdeckPluginKit
import SwiftUI

struct SettingsView: View {
    var body: some View {
        TabView {
            GeneralSettings().tabItem { Label("General", systemImage: "gearshape") }
            PluginSettings().tabItem { Label("Plugins", systemImage: "puzzlepiece.extension") }
        }
        .frame(width: 520)
    }
}

private struct GeneralSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Form {
            Section("Recordings") {
                LabeledContent("Folder") {
                    HStack {
                        Text(model.recordingsRoot.path(percentEncoded: false).replacingOccurrences(of: NSHomeDirectory(), with: "~"))
                            .lineLimit(1).truncationMode(.middle)
                        Button("Choose…", action: chooseFolder)
                    }
                }
                Picker("Format", selection: $model.format) {
                    Text("WAV 24-bit (lossless)").tag(RecordingFormat.wav)
                    Text("M4A (AAC 256 kbps)").tag(RecordingFormat.m4a)
                }
                Toggle("Save TX1 and TX2 as separate files in stereo", isOn: $model.splitTracks)
                    .disabled(model.format != .wav)
                Text("Files go into a dated folder, named by start time. Split files need WAV and stereo mode.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            Section("Controls") {
                Toggle("Tap a mic’s link button to start or stop recording", isOn: $model.tapToRecord)
                Text("While on, the receiver’s button presses no longer change the Mac’s volume. Shortcut: ⌥⌘R.")
                    .font(.callout).foregroundStyle(.secondary)
                Toggle("Open at login", isOn: Binding(get: { model.launchAtLogin }, set: { model.launchAtLogin = $0 }))
            }
        }
        .formStyle(.grouped)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.directoryURL = model.recordingsRoot
        if panel.runModal() == .OK, let url = panel.url { model.recordingsRoot = url }
    }
}

private struct PluginSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let host = model.plugins
        Form {
            if host.plugins.isEmpty {
                Section {
                    Text("No plugins installed. Plugins add optional features such as transcription. Install one from a folder, or drop it into the plugins folder.")
                        .foregroundStyle(.secondary)
                }
            }
            ForEach(host.plugins) { plugin in
                Section {
                    Toggle(isOn: Binding(get: { host.isEnabled(plugin) }, set: { on in
                        host.setEnabled(plugin, on)
                        if on, let needed = plugin.manifest.permissions { Task { await model.permissions.request(needed) } }
                    })) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(plugin.manifest.name).font(.headline)
                            if let summary = plugin.manifest.summary { Text(summary).font(.callout).foregroundStyle(.secondary) }
                            Text("\(plugin.manifest.version) · \(plugin.id)").font(.caption).foregroundStyle(.tertiary)
                        }
                    }
                    if let needed = plugin.manifest.permissions, !needed.isEmpty {
                        PermissionRow(permissions: needed)
                    }
                    if host.isEnabled(plugin) {
                        ForEach(plugin.manifest.settings ?? []) { setting in
                            SettingRow(setting: setting, plugin: plugin)
                        }
                    }
                }
            }
            if !host.problems.isEmpty {
                Section("Couldn’t load") {
                    ForEach(host.problems, id: \.self) { Text($0).font(.callout).foregroundStyle(.secondary) }
                }
            }
            Section {
                HStack {
                    Button("Install Plugin…", action: install)
                    Button("Open Plugins Folder") {
                        try? FileManager.default.createDirectory(at: PluginStore.pluginsFolder, withIntermediateDirectories: true)
                        NSWorkspace.shared.open(PluginStore.pluginsFolder)
                    }
                    Spacer()
                    Button("Reload") { host.reload() }
                }
            } footer: {
                Text("Plugins run as separate programs with your permissions and Lavdeck’s. Lavdeck asks macOS for each permission once, whichever plugins need it. Only install plugins you trust.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .frame(minHeight: 360)
    }

    private func install() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.message = "Choose a plugin folder (it contains plugin.json)"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do { try model.plugins.install(from: url) } catch {
            let alert = NSAlert()
            alert.messageText = "That folder isn’t a Lavdeck plugin"
            alert.informativeText = "\(error.localizedDescription)"
            alert.runModal()
        }
    }
}

private struct SettingRow: View {
    @Environment(AppModel.self) private var model
    let setting: PluginSetting
    let plugin: InstalledPlugin

    var body: some View {
        let host = model.plugins
        let value = host.value(setting, for: plugin)
        Group {
            switch setting.type {
            case .bool:
                Toggle(setting.title, isOn: Binding(get: { value.boolValue ?? false },
                                                    set: { host.setValue(.bool($0), setting, for: plugin) }))
            case .choice:
                Picker(setting.title, selection: Binding(get: { value.description },
                                                         set: { host.setValue(.string($0), setting, for: plugin) })) {
                    ForEach(setting.options ?? [], id: \.value) { Text($0.title).tag($0.value) }
                }
            case .number:
                TextField(setting.title, text: Binding(get: { value.description },
                                                       set: { if let n = Double($0) { host.setValue(.number(n), setting, for: plugin) } }))
            case .string, .path:
                LabeledContent(setting.title) {
                    HStack {
                        TextField("", text: Binding(get: { value.description },
                                                    set: { host.setValue(.string($0), setting, for: plugin) }))
                            .textFieldStyle(.roundedBorder)
                        if setting.type == .path {
                            Button("Choose…") { choosePath(current: value.description) }
                        }
                    }
                }
            }
        }
        .help(setting.help ?? "")
    }

    private func choosePath(current: String) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.directoryURL = URL(fileURLWithPath: (current as NSString).expandingTildeInPath).deletingLastPathComponent()
        if panel.runModal() == .OK, let url = panel.url {
            model.plugins.setValue(.string(url.path), setting, for: plugin)
        }
    }
}

/// The permissions a plugin relies on, with Lavdeck's current grant for each.
private struct PermissionRow: View {
    @Environment(AppModel.self) private var model
    let permissions: [String]

    var body: some View {
        let center = model.permissions
        HStack(alignment: .firstTextBaseline, spacing: 14) {
            Label("Uses Lavdeck’s", systemImage: "lock.shield").font(.callout).foregroundStyle(.secondary)
            ForEach(permissions, id: \.self) { permission in
                let state = center.state(permission)
                HStack(spacing: 4) {
                    Image(systemName: state == .granted ? "checkmark.circle.fill" : "exclamationmark.circle")
                        .foregroundStyle(state == .granted ? Color.green : Color.orange)
                    Text(PermissionCenter.name(permission)).font(.callout)
                }
                .help(state == .granted ? "Allowed" : "Not allowed yet")
            }
            Spacer()
            if permissions.contains(where: { center.state($0) != .granted }) {
                Button("Allow…") {
                    Task {
                        await center.request(permissions)
                        if let blocked = permissions.first(where: { center.state($0) == .denied }) { center.openSettings(for: blocked) }
                    }
                }
                .controlSize(.small)
            }
        }
    }
}
