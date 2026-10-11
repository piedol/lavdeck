import SwiftUI

@main
struct LavdeckApp: App {
    @State private var model: AppModel

    init() {
        let model = AppModel()
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
