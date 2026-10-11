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

        Settings {
            SettingsView().environment(model)
        }
    }
}
