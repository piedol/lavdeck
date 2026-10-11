import AppKit
import LavdeckKit
import SwiftUI

struct SettingsView: View {
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
        .frame(width: 460)
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
