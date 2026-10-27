// Renders the Lavdeck icon: the menu bar glyph while recording, drawn large. A graphite squircle,
// two white channel bars for TX1 and TX2, and a red record dot with a soft glow.
// Usage: swift scripts/make-icon.swift   → assets/AppIcon.icns, assets/logo.png
import AppKit

let red = NSColor(srgbRed: 1.0, green: 0.36, blue: 0.30, alpha: 1)

func draw(_ px: CGFloat) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(px), pixelsHigh: Int(px), bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                               bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let ctx = NSGraphicsContext.current!.cgContext
    ctx.scaleBy(x: px / 1024, y: px / 1024)
    let srgb = CGColorSpace(name: CGColorSpace.sRGB)!

    // Squircle body on the macOS icon grid (824 pt body centred in 1024).
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let path = CGPath(roundedRect: body, cornerWidth: 186, cornerHeight: 186, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 28, color: NSColor(white: 0, alpha: 0.35).cgColor)
    ctx.addPath(path); ctx.setFillColor(NSColor(srgbRed: 0.08, green: 0.08, blue: 0.10, alpha: 1).cgColor); ctx.fillPath()
    ctx.restoreGState()
    ctx.saveGState()
    ctx.addPath(path); ctx.clip()
    let bg = CGGradient(colorsSpace: srgb, colors: [
        NSColor(srgbRed: 0.17, green: 0.17, blue: 0.21, alpha: 1).cgColor,
        NSColor(srgbRed: 0.06, green: 0.06, blue: 0.08, alpha: 1).cgColor] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(bg, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])

    // The glyph is 18 × 14 pt in the menu bar; here one of its points is 32 px, centred on the body.
    // Coordinates below are the glyph's, with y running down as it does in MenuBarLabel.swift.
    let k: CGFloat = 32, x0: CGFloat = 216, y0: CGFloat = 272
    func rect(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> CGRect {
        CGRect(x: x0 + x * k, y: 1024 - (y0 + (y + h) * k), width: w * k, height: h * k)
    }

    // A pool of red light behind the dot.
    let dot = rect(11.5, 5.5, 6, 6)
    let centre = CGPoint(x: dot.midX, y: dot.midY)
    let glow = CGGradient(colorsSpace: srgb, colors: [
        red.withAlphaComponent(0.42).cgColor, red.withAlphaComponent(0).cgColor] as CFArray, locations: [0, 1])!
    ctx.drawRadialGradient(glow, startCenter: centre, startRadius: 0, endCenter: centre, endRadius: 330, options: [])

    // The two channel bars.
    ctx.setFillColor(NSColor(white: 0.95, alpha: 1).cgColor)
    for bar in [rect(1, 5, 3, 8), rect(6, 2, 3, 11)] {
        ctx.addPath(CGPath(roundedRect: bar, cornerWidth: 30, cornerHeight: 30, transform: nil))
        ctx.fillPath()
    }

    // The record dot.
    ctx.setFillColor(red.cgColor)
    ctx.fillEllipse(in: dot)

    ctx.restoreGState()
    NSGraphicsContext.restoreGraphicsState()
    return rep
}

let set = URL(fileURLWithPath: "assets/AppIcon.iconset")
try? FileManager.default.removeItem(at: set)
try! FileManager.default.createDirectory(at: set, withIntermediateDirectories: true)
for (pt, scale) in [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)] {
    let name = scale == 1 ? "icon_\(pt)x\(pt).png" : "icon_\(pt)x\(pt)@2x.png"
    let data = draw(CGFloat(pt * scale)).representation(using: .png, properties: [:])!
    try! data.write(to: set.appendingPathComponent(name))
}
try! draw(512).representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "assets/logo.png"))
let p = Process(); p.launchPath = "/usr/bin/iconutil"; p.arguments = ["-c", "icns", set.path, "-o", "assets/AppIcon.icns"]
p.launch(); p.waitUntilExit()
// The iconset is an intermediate build product of this script.
try? FileManager.default.removeItem(at: set)
print("assets/AppIcon.icns, assets/logo.png")
