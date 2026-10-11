import AppKit
import SwiftUI

/// Menu bar glyph: two channel bars and a master stub. Healthy is a template image (follows the
/// menu bar's appearance); recording turns it red with a dot; a dropped mic outlines its bar in amber;
/// no receiver shows it hollow.
struct MenuBarLabel: View {
    let state: GlyphState

    var body: some View {
        Image(nsImage: Self.image(for: state))
            .accessibilityLabel(accessibility)
    }

    private var accessibility: String {
        switch state {
        case .healthy: "Lavdeck, all mics linked"
        case .recording: "Lavdeck, recording"
        case .attention(.dropped): "Lavdeck, a mic disconnected"
        case .attention(.lowBattery): "Lavdeck, a mic battery is low"
        case .attention(.error): "Lavdeck, something needs attention"
        case .absent: "Lavdeck, receiver not connected"
        }
    }

    static func image(for state: GlyphState) -> NSImage {
        let image = NSImage(size: NSSize(width: 18, height: 14), flipped: true) { _ in
            // Drawn at display time, so the neutral parts can follow the menu bar's own appearance.
            let onDark = NSAppearance.currentDrawing().bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            let neutral = onDark ? NSColor.white : NSColor.black
            let amber = NSColor(hex: 0xE4A100)
            let ink = state == .recording ? NSColor(hex: 0xC4261F) : neutral
            func bar(_ rect: NSRect, fill: Bool, color: NSColor) {
                let path = NSBezierPath(roundedRect: rect, xRadius: 1, yRadius: 1)
                if fill { color.setFill(); path.fill() } else { color.setStroke(); path.lineWidth = 1; path.stroke() }
            }
            switch state {
            case .healthy:
                bar(NSRect(x: 1, y: 5, width: 3, height: 8), fill: true, color: ink)
                bar(NSRect(x: 6, y: 2, width: 3, height: 11), fill: true, color: ink)
                bar(NSRect(x: 12, y: 7, width: 5, height: 3), fill: true, color: ink)
            case .recording:
                bar(NSRect(x: 1, y: 5, width: 3, height: 8), fill: true, color: ink)
                bar(NSRect(x: 6, y: 2, width: 3, height: 11), fill: true, color: ink)
                ink.setFill(); NSBezierPath(ovalIn: NSRect(x: 11.5, y: 5.5, width: 6, height: 6)).fill()
            case .attention(.dropped):
                bar(NSRect(x: 1, y: 5, width: 3, height: 8), fill: true, color: neutral)
                bar(NSRect(x: 6.5, y: 2.5, width: 2, height: 10), fill: false, color: amber)
                bar(NSRect(x: 12, y: 7, width: 5, height: 3), fill: true, color: neutral)
            case .attention(.lowBattery):
                bar(NSRect(x: 1, y: 5, width: 3, height: 8), fill: true, color: neutral)
                bar(NSRect(x: 6, y: 2, width: 3, height: 11), fill: true, color: neutral)
                bar(NSRect(x: 11.5, y: 5.5, width: 6, height: 5), fill: false, color: amber)
                bar(NSRect(x: 12.5, y: 6.5, width: 1.5, height: 3), fill: true, color: amber)
            case .attention(.error):
                // Warning triangle: a different shape from the recording dot, not just a different colour.
                bar(NSRect(x: 1, y: 5, width: 3, height: 8), fill: true, color: neutral)
                bar(NSRect(x: 6, y: 2, width: 3, height: 11), fill: true, color: neutral)
                let tri = NSBezierPath()
                tri.move(to: NSPoint(x: 14.5, y: 3.5)); tri.line(to: NSPoint(x: 18, y: 12.5)); tri.line(to: NSPoint(x: 11, y: 12.5)); tri.close()
                amber.setFill(); tri.fill()
                (onDark ? NSColor.black : NSColor.white).setFill()
                NSBezierPath(rect: NSRect(x: 14, y: 6.5, width: 1, height: 3)).fill()
                NSBezierPath(rect: NSRect(x: 14, y: 10.5, width: 1, height: 1)).fill()
            case .absent:
                bar(NSRect(x: 1.5, y: 5.5, width: 2, height: 7), fill: false, color: neutral)
                bar(NSRect(x: 6.5, y: 2.5, width: 2, height: 10), fill: false, color: neutral)
                bar(NSRect(x: 12.5, y: 7.5, width: 4, height: 2), fill: false, color: neutral)
            }
            return true
        }
        // Pure-ink glyphs are templates (the menu bar tints them); coloured ones pick black or white
        // from the drawing appearance above.
        image.isTemplate = state == .healthy || state == .absent
        return image
    }
}
