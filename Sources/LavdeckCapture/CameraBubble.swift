import AppKit
import AVFoundation

/// A draggable camera preview that floats above other windows while recording. Double-click cycles its size:
/// a round bubble, a large 16:9 view of the whole frame, and full screen (the screen's usable area, so the
/// menu bar stays reachable). The preview is mirrored like a mirror; the recorded file is not. ScreenRecorder
/// leaves this window out, so it never ends up in the screen recording at any size.
@MainActor
final class CameraBubble {
    enum Size: String, CaseIterable {
        case bubble, large, full
        var next: Size { Self.allCases[(Self.allCases.firstIndex(of: self)! + 1) % Self.allCases.count] }
        var hint: String {
            switch self {
            case .bubble: "Double-click to enlarge"
            case .large: "Double-click for full screen"
            case .full: "Double-click to shrink"
            }
        }
    }

    private let panel: NSPanel
    private let surface: PreviewView
    private var size: Size
    private static let positionKey = "bubbleOrigin"
    private let diameter: CGFloat = 180

    var windowNumber: Int { panel.windowNumber }

    init(session: AVCaptureSession, size: Size = .bubble) {
        self.size = size
        panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 180, height: 180),
                        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = .floating
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.hidesOnDeactivate = false
        surface = PreviewView(session: session)
        panel.contentView = surface
        surface.onDoubleClick = { [weak self] in self?.cycle() }
        apply(size, animate: false)
    }

    func show() {
        panel.orderFrontRegardless()
        surface.flashHint(size.hint)
    }

    func close() {
        if size == .bubble { UserDefaults.standard.set(NSStringFromPoint(panel.frame.origin), forKey: Self.positionKey) }
        panel.orderOut(nil)
    }

    private func cycle() {
        if size == .bubble { UserDefaults.standard.set(NSStringFromPoint(panel.frame.origin), forKey: Self.positionKey) }
        size = size.next
        apply(size, animate: true)
        surface.flashHint(size.hint)
    }

    private func apply(_ size: Size, animate: Bool) {
        let screen = (panel.screen ?? NSScreen.main)?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        var frame: NSRect
        switch size {
        case .bubble:
            var origin = NSPoint(x: screen.maxX - diameter - 32, y: screen.minY + 32)
            if let saved = UserDefaults.standard.string(forKey: Self.positionKey) {
                let p = NSPointFromString(saved)
                if screen.insetBy(dx: -diameter, dy: -diameter).contains(p) { origin = p }
            }
            frame = NSRect(origin: origin, size: NSSize(width: diameter, height: diameter))
        case .large:
            let width = min(720, screen.width * 0.45)
            let height = (width * 9 / 16).rounded()
            frame = NSRect(x: screen.maxX - width - 32, y: screen.minY + 32, width: width, height: height)
        case .full:
            frame = screen
        }
        panel.setFrame(frame, display: true, animate: animate)
        surface.style(round: size == .bubble, fill: size == .bubble, cornerRadius: size == .full ? 0 : 12)
    }
}

/// The preview layer plus a short-lived hint label. Drag moves the panel; double-click reports up.
private final class PreviewView: NSView {
    var onDoubleClick: (() -> Void)?
    private let preview: AVCaptureVideoPreviewLayer
    private let hint = NSTextField(labelWithString: "")
    private var hintTask: Task<Void, Never>?

    init(session: AVCaptureSession) {
        preview = AVCaptureVideoPreviewLayer(session: session)
        super.init(frame: NSRect(x: 0, y: 0, width: 180, height: 180))
        wantsLayer = true
        layer?.masksToBounds = true
        layer?.backgroundColor = NSColor.black.cgColor
        layer?.borderColor = NSColor.white.withAlphaComponent(0.9).cgColor
        preview.frame = bounds
        preview.autoresizingMask = [.layerWidthSizable, .layerHeightSizable]
        preview.connection?.automaticallyAdjustsVideoMirroring = false
        preview.connection?.isVideoMirrored = true
        layer?.addSublayer(preview)

        hint.font = .systemFont(ofSize: 11, weight: .semibold)
        hint.textColor = .white
        hint.alignment = .center
        hint.wantsLayer = true
        hint.layer?.backgroundColor = NSColor.black.withAlphaComponent(0.55).cgColor
        hint.layer?.cornerRadius = 8
        hint.alphaValue = 0
        hint.translatesAutoresizingMaskIntoConstraints = false
        addSubview(hint)
        NSLayoutConstraint.activate([
            hint.centerXAnchor.constraint(equalTo: centerXAnchor),
            hint.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -22),
            hint.widthAnchor.constraint(lessThanOrEqualTo: widthAnchor, constant: -24),
        ])
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    /// Round with aspect-fill for the bubble; the whole frame (letterboxed) when large or full screen.
    func style(round: Bool, fill: Bool, cornerRadius: CGFloat) {
        layer?.cornerRadius = round ? bounds.width / 2 : cornerRadius
        layer?.borderWidth = round ? 3 : 0
        preview.videoGravity = fill ? .resizeAspectFill : .resizeAspect
    }

    override func layout() {
        super.layout()
        if layer?.borderWidth == 3 { layer?.cornerRadius = bounds.width / 2 }
    }

    func flashHint(_ text: String) {
        hint.stringValue = "  \(text)  "
        hintTask?.cancel()
        NSAnimationContext.runAnimationGroup { $0.duration = 0.2; hint.animator().alphaValue = 1 }
        hintTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled, let self else { return }
            await NSAnimationContext.runAnimationGroup { $0.duration = 0.4; self.hint.animator().alphaValue = 0 }
        }
    }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { onDoubleClick?() } else { window?.performDrag(with: event) }
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}
