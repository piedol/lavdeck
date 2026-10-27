import CoreGraphics
import LavdeckCapture
import LavdeckKit
import AVFoundation
import AppKit
import SwiftUI

/// Development aid: `Lavdeck --snapshot <dir>` renders the live popover (light and dark, idle and
/// recording) plus the menu bar glyphs to PNGs, then quits. It uses the real receiver and recorder, or
/// made-up mics with `--demo`.
@MainActor
enum Snapshot {
    /// `--demo`: a run for screenshots, with made-up mics and no menu bar item, receiver or shortcut.
    nonisolated static let isDemo = CommandLine.arguments.contains("--demo")

    static func runIfRequested(model: AppModel) {
        let args = CommandLine.arguments
        if let i = args.firstIndex(of: "--permission-report"), i + 1 < args.count {
            // Development aid: write what this process sees for each permission, then quit.
            let report: [String: Any] = [
                "screenPreflight": CGPreflightScreenCaptureAccess(),
                "camera": AVCaptureDevice.authorizationStatus(for: .video).rawValue,
                "microphone": AVCaptureDevice.authorizationStatus(for: .audio).rawValue,
                "pid": getpid(),
            ]
            if let data = try? JSONSerialization.data(withJSONObject: report, options: .prettyPrinted) {
                try? data.write(to: URL(fileURLWithPath: args[i + 1]))
            }
            exit(0)
        }
        if let i = args.firstIndex(of: "--window-classes"), i + 1 < args.count {
            // Development aid: list this app's window classes (to find the status item window).
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(3))
                let lines = NSApp.windows.map { "\(type(of: $0)) \($0.frame)" }.joined(separator: "\n")
                try? lines.write(toFile: args[i + 1], atomically: true, encoding: .utf8)
                NSApp.terminate(nil)
            }
            return
        }
        if let i = args.firstIndex(of: "--make-video"), i + 2 < args.count {
            // Development aid: `--make-video <day folder> <stem>` writes that take's finished video, then quits.
            Task { @MainActor in
                guard let take = TakeLibrary.take(folder: URL(fileURLWithPath: args[i + 1]), stem: args[i + 2]) else {
                    print("no take"); exit(1)
                }
                do {
                    let url = try await VideoComposer.make(take.descriptor, settings: model.video) { _ in }
                    print("made \(url.path)")
                    exit(0)
                } catch {
                    print("failed: \(error)")
                    exit(1)
                }
            }
            return
        }
        if args.contains("--quit-while-recording") {
            // Development aid: start a take, then quit mid-recording like macOS's "Quit & Reopen" does.
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(3))
                model.startRecording()
                try? await Task.sleep(for: .seconds(5))
                NSApp.terminate(nil)
            }
            return
        }
        if let i = args.firstIndex(of: "--recordings-window") {
            showRecordings(model: model, dark: args.contains("--dark"), selectAll: args.contains("--select-all"),
                           row: args.firstIndex(of: "--select-row").flatMap { Int(args[safe: $0 + 1] ?? "") },
                           seconds: Double(args[safe: i + 1] ?? "") ?? 20)
            return
        }
        guard let i = args.firstIndex(of: "--snapshot"), i + 1 < args.count else { return }
        let dir = URL(fileURLWithPath: args[i + 1], isDirectory: true)
        model.debugForceLowBattery = args.contains("--force-low-battery")
        Task { @MainActor in
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            if args.contains("--demo") {
                try? await Task.sleep(for: .seconds(1))
                model.loadDemo(recording: false)
                render(model: model, name: "idle", dir: dir)
                model.loadDemo(recording: true)
                try? await Task.sleep(for: .milliseconds(300))
                render(model: model, name: "recording", dir: dir)
                model.loadDemoIdentify()
                try? await Task.sleep(for: .milliseconds(300))
                render(model: model, name: "identify", dir: dir)
            } else {
                try? await Task.sleep(for: .seconds(4))
                render(model: model, name: "idle", dir: dir)
                if args.contains("--identify") {
                    model.startIdentify()
                    try? await Task.sleep(for: .seconds(3))
                    render(model: model, name: "identify", dir: dir)
                    await model.endIdentify()
                    try? await Task.sleep(for: .seconds(1))
                    render(model: model, name: "after-identify", dir: dir)
                }
            }
            if args.contains("--record") && !args.contains("--demo") {
                model.startRecording()
                for _ in 0..<300 where !model.isRecording { try? await Task.sleep(for: .milliseconds(100)) }
                try? await Task.sleep(for: .seconds(4))
                render(model: model, name: "recording", dir: dir)
                model.stopRecording()
                try? await Task.sleep(for: .seconds(2))
                render(model: model, name: "after", dir: dir)
            }
            for state in [GlyphState.healthy, .recording, .attention(.dropped), .attention(.lowBattery), .attention(.error), .absent] {
                let name = String(describing: state).replacingOccurrences(of: "attention(", with: "attention-")
                    .replacingOccurrences(of: ")", with: "").replacingOccurrences(of: "LavdeckApp.AttentionKind.", with: "")
                    .replacingOccurrences(of: ".", with: "")
                for (suffix, appearance) in [("light", NSAppearance(named: .aqua)!), ("dark", NSAppearance(named: .darkAqua)!)] {
                    // Draw the way the menu bar does: template images take the bar's label colour.
                    let source = MenuBarLabel.image(for: state)
                    let size = NSSize(width: 54, height: 42)
                    let out = NSImage(size: size)
                    appearance.performAsCurrentDrawingAppearance {
                        out.lockFocus()
                        (suffix == "dark" ? NSColor(hex: 0x2B2D31) : NSColor(hex: 0xE8E9EB)).setFill()
                        NSRect(origin: .zero, size: size).fill()
                        if source.isTemplate {
                            let tinted = NSImage(size: source.size, flipped: false) { rect in
                                source.draw(in: rect)
                                (suffix == "dark" ? NSColor.white : NSColor.black).set()
                                rect.fill(using: .sourceAtop)
                                return true
                            }
                            tinted.draw(in: NSRect(x: 9, y: 7, width: 36, height: 28))
                        } else {
                            source.draw(in: NSRect(x: 9, y: 7, width: 36, height: 28))
                        }
                        out.unlockFocus()
                    }
                    if let tiff = out.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) {
                        try? rep.representation(using: .png, properties: [:])?.write(to: dir.appendingPathComponent("glyph-\(name)-\(suffix).png"))
                    }
                }
            }
            NSApp.terminate(nil)
        }
    }

    private static func render(model: AppModel, name: String, dir: URL) {
        for (suffix, appearance) in [("light", NSAppearance(named: .aqua)!), ("dark", NSAppearance(named: .darkAqua)!),
                                     ("hc-light", NSAppearance(named: .accessibilityHighContrastAqua)!)] {
            NSApp.appearance = appearance
            let view = PopoverView().environment(model).environment(\.colorScheme, suffix == "dark" ? .dark : .light)
            let renderer = ImageRenderer(content: view)
            renderer.scale = 2
            if let image = renderer.nsImage, let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) {
                try? rep.representation(using: .png, properties: [:])?.write(to: dir.appendingPathComponent("\(name)-\(suffix).png"))
            }
        }
        NSApp.appearance = nil
    }

    nonisolated(unsafe) private static var window: NSWindow?

    /// `Lavdeck --recordings-window [seconds] [--dark] [--select-all] [--size WxH]` opens the Recordings window on
    /// its own and prints its window number, so it can be captured with `screencapture -l`. The window sits behind
    /// everything at the right edge of the screen, with 2 points showing, and Lavdeck doesn't activate, so a capture
    /// never takes focus from the app you're using. `--select-all` focuses the table and selects every take, to show
    /// the multi-selection pane; `--select-row n` selects one row instead.
    private static func showRecordings(model: AppModel, dark: Bool, selectAll: Bool, row: Int?, seconds: Double) {
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(500))
            NSApp.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
            let args = CommandLine.arguments
            let size = args.firstIndex(of: "--size").flatMap { args[safe: $0 + 1] }
                .map { $0.split(separator: "x").compactMap { Double($0) } }
                .flatMap { $0.count == 2 ? NSSize(width: $0[0], height: $0[1]) : nil } ?? NSSize(width: 1040, height: 640)
            let w = SnapshotWindow(contentRect: NSRect(origin: .zero, size: size),
                                   styleMask: [.titled, .closable, .resizable, .miniaturizable, .fullSizeContentView],
                                   backing: .buffered, defer: false)
            w.contentViewController = NSHostingController(rootView: NavigationStack { RecordingsView() }.environment(model))
            w.setContentSize(size)
            w.title = "Recordings"
            w.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.desktopWindow)))
            if let screen = NSScreen.screens.first?.frame { w.setFrameOrigin(NSPoint(x: screen.maxX - 2, y: screen.minY + 40)) }
            window = w
            w.orderFrontRegardless()
            try? await Task.sleep(for: .seconds(1))
            let table = w.contentView.flatMap(firstTable)
            if let table { w.makeFirstResponder(table) }
            if let row { table?.selectRowIndexes([row], byExtendingSelection: false) }
            // What Edit › Select All (⌘A) does in the focused table.
            if selectAll { table?.selectAll(nil) }
            try? await Task.sleep(for: .milliseconds(500))
            // A focused table in the key window draws its selection in the accent colour.
            table?.enumerateAvailableRowViews { rowView, _ in rowView.isEmphasized = true }
            try? await Task.sleep(for: .milliseconds(200))
            print("window \(w.windowNumber)")
            fflush(stdout)
            try? await Task.sleep(for: .seconds(seconds))
            NSApp.terminate(nil)
        }
    }
}

/// Draws as the key window of the active app without Lavdeck activating, so a snapshot shows the window as it
/// looks in use. It also stays where it's put, even off the screen's edge.
private final class SnapshotWindow: NSWindow {
    override var isKeyWindow: Bool { true }
    override var isMainWindow: Bool { true }
    @objc func _hasActiveAppearance() -> Bool { true }
    @objc func _hasActiveAppearanceIgnoringKeyFocus() -> Bool { true }
    @objc func _hasActiveControls() -> Bool { true }
    @objc func _hasActiveAppearanceForStandardWindowButton(_ button: Int) -> Bool { true }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}

@MainActor private func firstTable(in view: NSView) -> NSTableView? {
    if let table = view as? NSTableView { return table }
    for sub in view.subviews { if let found = firstTable(in: sub) { return found } }
    return nil
}

private extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}
