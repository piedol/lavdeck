import AppKit
import SwiftUI

/// Channel Strip palette. Light: anodized deck, white strip faces, black scribble strips.
enum Theme {
    static let deck = dynamic(light: 0xE3E5E8, dark: 0x1C1E21)
    static let face = dynamic(light: 0xF7F8F9, dark: 0x26292D)
    static let well = dynamic(light: 0xFFFFFF, dark: 0x16181B)
    static let hairline = dynamic(light: 0xCDD2D7, dark: 0x3A3F45, highContrastLight: 0x6B737B, highContrastDark: 0x9AA3AC)
    static let ink = dynamic(light: 0x121416, dark: 0xF1F3F5)
    /// Text on an `ink` fill (selected segments).
    static let onInk = dynamic(light: 0xFFFFFF, dark: 0x121416)
    static let mute = dynamic(light: 0x5A636B, dark: 0xA9B1B9)
    static let plate = Color(hex: 0x121416)
    static let plateText = Color.white
    static let plateSub = Color(hex: 0xC9D0D6)
    static let ledOff = dynamic(light: 0xD5D9DD, dark: 0x34393F)
    static let green = Color(hex: 0x18A957)
    static let amber = Color(hex: 0xE4A100)
    static let red = Color(hex: 0xD32F2F)
    static let rec = Color(hex: 0xC4261F)
    /// Record red for text: lifted in dark appearance to clear 4.5:1 on the deck and wells.
    static let recText = dynamic(light: 0xC4261F, dark: 0xFF6B61)
    static let warnFill = dynamic(light: 0xFBF0DC, dark: 0x3A2E14)
    static let warnText = dynamic(light: 0x6B4300, dark: 0xFFD27A)

    static let label = Font.system(size: 12)
    static let labelBold = Font.system(size: 12, weight: .semibold)
    static let caps = Font.system(size: 11, weight: .bold)
    static let mono = Font.system(size: 11, weight: .medium, design: .monospaced)

    private static func dynamic(light: UInt32, dark: UInt32,
                                highContrastLight: UInt32? = nil, highContrastDark: UInt32? = nil) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            switch appearance.bestMatch(from: [.aqua, .darkAqua, .accessibilityHighContrastAqua, .accessibilityHighContrastDarkAqua]) {
            case .darkAqua?: NSColor(hex: dark)
            case .accessibilityHighContrastDarkAqua?: NSColor(hex: highContrastDark ?? dark)
            case .accessibilityHighContrastAqua?: NSColor(hex: highContrastLight ?? light)
            default: NSColor(hex: light)
            }
        })
    }
}

extension Color {
    init(hex: UInt32) { self.init(nsColor: NSColor(hex: hex)) }
}

extension NSColor {
    convenience init(hex: UInt32) {
        self.init(srgbRed: CGFloat(hex >> 16 & 0xff) / 255, green: CGFloat(hex >> 8 & 0xff) / 255,
                  blue: CGFloat(hex & 0xff) / 255, alpha: 1)
    }
}

extension View {
    /// Caps label used for section names ("GAIN").
    func capsLabel() -> some View {
        font(Theme.caps).tracking(0.9).foregroundStyle(Theme.mute)
    }
}
