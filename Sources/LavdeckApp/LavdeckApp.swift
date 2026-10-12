import SwiftUI

/// Quitting mid-take (including macOS's "Quit & Reopen" after a permission change) first stops the take
/// so its audio and captured files are all saved.
final class LavdeckAppDelegate: NSObject, NSApplicationDelegate {
    @MainActor static var model: AppModel?
    @MainActor private static var savedForQuit = false
    @MainActor private final class SavedFlag { var isSet = false }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        MainActor.assumeIsolated {
            guard let model = Self.model, model.isRecording, !Self.savedForQuit else { return .terminateNow }
            // Cancel this quit, save the take (at most 20 seconds), then quit again. Waiting inside
            // .terminateLater would block the main actor work that saves the take.
            Task { @MainActor in
                let saved = SavedFlag()
                Task { @MainActor in
                    await model.finishRecordingForQuit()
                    saved.isSet = true
                }
                for _ in 0..<200 where !saved.isSet { try? await Task.sleep(for: .milliseconds(100)) }
                Self.savedForQuit = true
                NSApp.terminate(nil)
            }
            return .terminateCancel
        }
    }
}

@main
struct LavdeckApp: App {
    @NSApplicationDelegateAdaptor(LavdeckAppDelegate.self) private var delegate
    @State private var model: AppModel
    @State private var statusMenu: StatusMenu

    init() {
        let model = AppModel()
        LavdeckAppDelegate.model = model
        _statusMenu = State(initialValue: StatusMenu(model: model))
        _model = State(initialValue: model)
        Snapshot.runIfRequested(model: model)
    }

    var body: some Scene {
        MenuBarExtra {
            PopoverView().environment(model)
        } label: {
            MenuBarLabel(state: model.glyph)
        }
        .menuBarExtraStyle(.window)

        Window("Recordings", id: "recordings") {
            RecordingsView().environment(model)
        }
        .defaultSize(width: 1040, height: 640)

        Settings {
            SettingsView().environment(model)
        }
    }
}
