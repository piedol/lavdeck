import AppKit
import LavdeckKit
import SwiftUI

/// Window actions captured from the SwiftUI scene, for use outside it (the right-click menu).
@MainActor
enum AppActions {
    static var openRecordings: (() -> Void)?
    static var openSettings: (() -> Void)?
}

/// Right-click (or Control-click) on the menu bar icon shows quick actions; a plain click still opens
/// the popover. MenuBarExtra has no menu of its own, so this watches for secondary clicks on Lavdeck's
/// status item window.
@MainActor
final class StatusMenu: NSObject {
    private let model: AppModel
    private var monitor: Any?

    init(model: AppModel) {
        self.model = model
        super.init()
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.rightMouseDown, .leftMouseDown]) { [weak self] event in
            guard let self, let window = event.window, Self.isStatusItemWindow(window) else { return event }
            let secondary = event.type == .rightMouseDown || event.modifierFlags.contains(.control)
            guard secondary, let view = window.contentView else { return event }
            let menu = self.makeMenu()
            menu.popUp(positioning: nil, at: NSPoint(x: 0, y: view.bounds.minY - 4), in: view)
            return nil
        }
    }

    private static func isStatusItemWindow(_ window: NSWindow) -> Bool {
        String(describing: type(of: window)).contains("StatusBarWindow")
    }

    private func makeMenu() -> NSMenu {
        let menu = NSMenu()
        menu.autoenablesItems = false

        if model.isRecording {
            menu.addItem(item("Stop Recording", key: "r", modifiers: [.option, .command]) { $0.stopRecording() })
        } else {
            let start = item(model.armed.isEmpty ? "Start Recording (no mic armed)" : "Start Recording",
                             key: "r", modifiers: [.option, .command]) { $0.startRecording() }
            start.isEnabled = model.isConnected && !model.armed.isEmpty
            menu.addItem(start)
        }

        if model.isConnected && !model.isRecording {
            menu.addItem(.separator())
            for mode in [ChannelMode.mono, .stereo] {
                let entry = item(mode == .mono ? "Mono" : "Stereo") { $0.setChannel(mode) }
                entry.state = model.receiver.channelMode == mode ? .on : .off
                menu.addItem(entry)
            }
        } else if !model.isConnected {
            let absent = NSMenuItem(title: "Receiver not connected", action: nil, keyEquivalent: "")
            absent.isEnabled = false
            menu.addItem(absent)
        }

        menu.addItem(.separator())
        menu.addItem(item("Recordings…", key: "", modifiers: []) { _ in AppActions.openRecordings?() })
        menu.addItem(item("Settings…", key: ",", modifiers: [.command]) { _ in AppActions.openSettings?() })
        menu.addItem(.separator())
        menu.addItem(item("Quit Lavdeck", key: "q", modifiers: [.command]) { _ in NSApp.terminate(nil) })
        return menu
    }

    private func item(_ title: String, key: String = "", modifiers: NSEvent.ModifierFlags = [],
                      _ action: @escaping @MainActor (AppModel) -> Void) -> NSMenuItem {
        let entry = ClosureMenuItem(title: title, key: key) { [model] in action(model) }
        entry.keyEquivalentModifierMask = modifiers
        return entry
    }
}

@MainActor
private final class ClosureMenuItem: NSMenuItem {
    private let handler: @MainActor () -> Void

    init(title: String, key: String, handler: @escaping @MainActor () -> Void) {
        self.handler = handler
        super.init(title: title, action: #selector(run), keyEquivalent: key)
        target = self
    }

    required init(coder: NSCoder) { fatalError("not used") }

    @objc private func run() { handler() }
}
