import AppKit
import LavdeckCapture
import LavdeckKit
import LavdeckPluginKit
import SwiftUI

struct SettingsView: View {
    var body: some View {
        TabView {
            GeneralSettings().tabItem { Label("General", systemImage: "gearshape") }
            VideoSettingsView().tabItem { Label("Video", systemImage: "video") }
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

/// Screen and camera recording, the camera preview and the finished video.
private struct VideoSettingsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Form {
            Section {
                Toggle("Record the screen", isOn: Binding(get: { model.video.screen }, set: { model.setVideoSource("screen", $0) }))
                Toggle("Record the camera", isOn: Binding(get: { model.video.camera }, set: { model.setVideoSource("camera", $0) }))
                PermissionRow(permissions: ["screen-recording", "camera"], title: "Permissions")
            } header: {
                Text("Record with the mics")
            } footer: {
                Text("Also on the deck, as SCREEN and CAMERA. Lavdeck’s own windows and the camera preview are left out of the screen recording.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Camera preview") {
                Toggle("Show a camera preview while recording", isOn: $model.video.cameraPreview)
                Picker("Starts as", selection: $model.video.previewSize) {
                    Text("Bubble").tag(VideoSettings.PreviewSize.bubble)
                    Text("Large").tag(VideoSettings.PreviewSize.large)
                    Text("Full screen").tag(VideoSettings.PreviewSize.full)
                }
                .disabled(!model.video.cameraPreview)
                Text("Double-click the preview to switch between a bubble, a large view of the whole frame, and full screen.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            Section("Screen recording") {
                Toggle("Also record system audio", isOn: $model.video.systemAudio)
                    .help("Sound from apps (a video you’re demoing, a call) as a second audio track in the screen recording and the finished video.")
                Picker("Frame rate", selection: $model.video.screenFPS) {
                    Text("30 fps").tag(30)
                    Text("60 fps").tag(60)
                }
                Picker("Video format", selection: $model.video.codec) {
                    Text("HEVC (smaller files)").tag(VideoSettings.Codec.hevc)
                    Text("H.264 (plays everywhere)").tag(VideoSettings.Codec.h264)
                }
            }
            Section {
                Toggle("Make a finished video after each take", isOn: $model.video.makeVideo)
                Picker("Camera bubble position", selection: $model.video.bubbleCorner) {
                    Text("Bottom right").tag(VideoSettings.Corner.bottomRight)
                    Text("Bottom left").tag(VideoSettings.Corner.bottomLeft)
                    Text("Top right").tag(VideoSettings.Corner.topRight)
                    Text("Top left").tag(VideoSettings.Corner.topLeft)
                }
                Picker("Camera bubble size", selection: $model.video.bubbleSize) {
                    Text("Small").tag(VideoSettings.BubbleSize.small)
                    Text("Medium").tag(VideoSettings.BubbleSize.medium)
                    Text("Large").tag(VideoSettings.BubbleSize.large)
                }
                Picker("Camera bubble shape", selection: $model.video.bubbleShape) {
                    Text("Circle").tag(VideoSettings.BubbleShape.circle)
                    Text("Rounded rectangle").tag(VideoSettings.BubbleShape.rounded)
                }
            } header: {
                Text("Finished video")
            } footer: {
                Text("One MP4 per take: your screen with the camera in a corner (or the camera alone), and the mic audio. To apply new bubble settings to an earlier take, use Remake Video in Recordings.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .frame(minHeight: 520)
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
    var title = "Uses Lavdeck’s"

    var body: some View {
        let center = model.permissions
        HStack(alignment: .firstTextBaseline, spacing: 14) {
            Label(title, systemImage: "lock.shield").font(.callout).foregroundStyle(.secondary)
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
