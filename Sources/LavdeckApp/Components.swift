import AppKit
import SwiftUI

/// Black scribble strip naming a channel; turns red and names its file while recording.
/// One line (the mic's name, with its slot as a small tag) unless a second line carries live info.
struct NamePlate: View {
    let title: String
    var subtitle: String? = nil
    /// Small trailing tag, e.g. the slot ("TX1") when the mic has a custom name.
    var tag: String? = nil
    var armed = false
    var dimmed = false
    /// A channel left out of the current take: drawn hollow so it stays readable.
    var hollow = false

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(title).font(Theme.labelBold).tracking(0.5)
                    .foregroundStyle(hollow ? Theme.ink : Theme.plateText)
                    .lineLimit(1).truncationMode(.tail)
                Spacer(minLength: 0)
                if let tag {
                    Text(tag).font(.system(size: 10, weight: .semibold, design: .monospaced))
                        .foregroundStyle(hollow ? Theme.mute : Theme.plateSub)
                }
            }
            if let subtitle {
                Text(subtitle).font(Theme.mono)
                    .foregroundStyle(hollow ? Theme.mute : armed ? Color(hex: 0xFFE1DE) : Theme.plateSub)
                    .lineLimit(1).truncationMode(.middle)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 7).padding(.vertical, 6)
        .background(RoundedRectangle(cornerRadius: 5).fill(hollow ? Theme.well : armed ? Theme.rec : Theme.plate))
        .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(Theme.hairline, lineWidth: 0.5))
        .opacity(dimmed ? 0.45 : 1)
    }
}

/// Link indicator: solid green when linked, hollow amber when a known mic dropped, off otherwise.
struct LinkLED: View {
    enum State { case linked, dropped, off }
    let state: State

    var body: some View {
        ZStack {
            switch state {
            case .linked:
                Circle().fill(Theme.green).frame(width: 9, height: 9)
                    .background(Circle().fill(Theme.green.opacity(0.18)).frame(width: 13, height: 13))
            case .dropped:
                Circle().strokeBorder(Theme.amber, lineWidth: 2).frame(width: 10, height: 10)
            case .off:
                Circle().fill(Theme.ledOff).frame(width: 9, height: 9)
            }
        }
        .frame(width: 14, height: 14)
        .accessibilityLabel(state == .linked ? "Linked" : state == .dropped ? "Disconnected" : "Not linked")
    }
}

/// Vertical 12-segment LED ladder with a short peak hold.
struct LEDMeter: View {
    let level: Double
    /// Mic left out of the take: segments dark and the well dimmed.
    var inactive = false
    private let segments = 12
    @State private var peak = 0.0
    @State private var peakTime = Date.distantPast
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let lit = inactive ? 0 : Int((level * Double(segments)).rounded(.down))
        let peakIndex = inactive ? -1 : Int((peak * Double(segments)).rounded(.down)) - 1
        VStack(spacing: 2) {
            ForEach((0..<segments).reversed(), id: \.self) { i in
                RoundedRectangle(cornerRadius: 1.5)
                    .fill(i < lit || i == peakIndex ? color(for: i) : Theme.ledOff)
                    .frame(width: 26, height: 7)
            }
        }
        .padding(4)
        .background(RoundedRectangle(cornerRadius: 5).fill(Theme.well)
            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(Theme.hairline, lineWidth: 0.5)))
        .opacity(inactive ? 0.5 : 1)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.08), value: lit)
        .onChange(of: level) { _, new in
            if new >= peak || Date().timeIntervalSince(peakTime) > 1.2 { peak = new; peakTime = Date() }
        }
        .onChange(of: inactive) { _, now in if now { peak = 0 } }
        .accessibilityElement()
        .accessibilityLabel("Level")
        .accessibilityValue(inactive ? "not armed" : "\(Int(level * 100)) percent")
    }

    private func color(for index: Int) -> Color {
        index >= 10 ? Theme.red : index >= 8 ? Theme.amber : Theme.green
    }
}

/// Charging state and the receiver's battery code for a transmitter. The Mobile RX reports battery as a
/// coarse code (bits 10-12 of the transmitter status), not a percentage. Code 1 has been confirmed as
/// "high" (DJI Mimo showed both mics at 100% while it read 1); other codes aren't mapped yet, so they're
/// shown as the raw code and logged when they change.
struct ChargeState: View {
    let charging: Bool?
    let code: Int?

    static func label(for code: Int?) -> String? {
        switch code {
        case 1?: "High"
        case let c?: "Level \(c)"
        case nil: nil
        }
    }

    var body: some View {
        HStack(spacing: 3) {
            if charging == true {
                Image(systemName: "bolt.fill").font(.system(size: 9, weight: .bold))
                Text("Charging")
            } else if let label = Self.label(for: code), charging != nil {
                Image(systemName: code == 1 ? "battery.100percent" : "battery.50percent").font(.system(size: 10, weight: .medium))
                Text(label)
            } else {
                Text("—")
            }
        }
        .font(.system(size: 10, weight: .semibold))
        .foregroundStyle(charging == true ? Theme.green : Theme.mute)
        .frame(height: 12)
        .help(code == 1 || code == nil
              ? "Battery as the receiver reports it: a coarse level, not a percentage."
              : "The receiver reports battery code \(code!), which isn’t mapped to a level yet. Changes are logged to ~/Library/Logs/Lavdeck/battery.log.")
        .accessibilityElement()
        .accessibilityLabel("Battery")
        .accessibilityValue(charging == true ? "charging" : Self.label(for: code) ?? "unknown")
    }
}

/// Vertical fader with five detents (−12…+12 dB). The cap follows the receiver's confirmed gain.
struct GainFader: View {
    let steps: [Int]
    let value: Int?
    let pending: Bool
    let onSet: (Int) -> Void
    var height: CGFloat = 132
    @State private var dragIndex: Int?

    var body: some View {
        let ordered = steps.sorted(by: >)
        HStack(alignment: .center, spacing: 18) {
            GeometryReader { geo in
                let h = geo.size.height
                let index = dragIndex ?? value.flatMap { ordered.firstIndex(of: $0) } ?? ordered.count / 2
                let y = ordered.count > 1 ? h * CGFloat(index) / CGFloat(ordered.count - 1) : h / 2
                ZStack(alignment: .top) {
                    Capsule().fill(Theme.ledOff).frame(width: 6).frame(maxHeight: .infinity)
                    RoundedRectangle(cornerRadius: 4)
                        .fill(LinearGradient(colors: [Color(hex: 0x2A2E32), Color(hex: 0x121416)], startPoint: .top, endPoint: .bottom))
                        .overlay(Capsule().fill(.white).frame(height: 2).padding(.horizontal, 5))
                        .frame(width: 34, height: 16)
                        .shadow(color: .black.opacity(0.3), radius: 2, y: 2)
                        .opacity(pending ? 0.55 : 1)
                        .offset(y: y - 8)
                }
                .frame(width: 34)
                .contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0)
                    .onChanged { g in
                        let i = Int((g.location.y / max(h, 1) * CGFloat(ordered.count - 1)).rounded())
                        let clamped = min(max(i, 0), ordered.count - 1)
                        if clamped != dragIndex {
                            dragIndex = clamped
                            NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .now)
                        }
                    }
                    .onEnded { _ in
                        if let i = dragIndex { onSet(ordered[i]) }
                        dragIndex = nil
                    })
            }
            .frame(width: 34)
            VStack(alignment: .leading) {
                ForEach(Array(ordered.enumerated()), id: \.offset) { i, step in
                    Button { onSet(step) } label: {
                        Text(step == 0 ? "0 dB" : step > 0 ? "+\(step)" : "−\(-step)")
                            .font(step == value ? Theme.labelBold : Theme.label)
                            .monospacedDigit()
                            .foregroundStyle(step == value ? Theme.ink : Theme.mute)
                    }
                    .buttonStyle(.plain)
                    if i < ordered.count - 1 { Spacer(minLength: 0) }
                }
            }
        }
        .frame(height: height)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Receiver gain")
        .accessibilityValue(value.map { "\($0) decibels" } ?? "unknown")
        .accessibilityAdjustableAction { direction in
            guard let value, let i = ordered.firstIndex(of: value) else { return }
            let next = direction == .increment ? i - 1 : i + 1
            if ordered.indices.contains(next) { onSet(ordered[next]) }
        }
    }
}

/// Two-position MONO | STEREO switch.
struct ModeSwitch: View {
    let mode: ChannelModeDisplay
    let disabled: Bool
    let onSet: (Bool) -> Void

    var body: some View {
        HStack(spacing: 0) {
            segment("MONO", on: mode == .mono) { onSet(false) }
            segment("STEREO", on: mode == .stereo) { onSet(true) }
        }
        .clipShape(RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.hairline, lineWidth: 0.5))
        .allowsHitTesting(!disabled)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Channel mode")
    }

    private func segment(_ title: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).font(Theme.caps).tracking(0.7)
                .frame(maxWidth: .infinity).padding(.vertical, 6)
                .foregroundStyle(on ? Theme.onInk : Theme.mute)
                .background(on ? Theme.ink : Theme.well)
                .opacity(disabled && !on ? 0.45 : 1)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

enum ChannelModeDisplay { case mono, stereo, unknown }

/// Per-strip record-arm button. Armed-but-idle is neutral ink; red is reserved for a live take.
struct ArmButton: View {
    let armed: Bool
    let live: Bool
    let action: () -> Void

    var body: some View {
        let locked = live
        let red = armed && live
        Button(action: action) {
            HStack(spacing: 5) {
                Circle()
                    .fill(red ? Theme.rec : armed ? Theme.ink : Color.clear)
                    .overlay(Circle().strokeBorder(armed ? Color.clear : Theme.mute, lineWidth: 1))
                    .frame(width: 8, height: 8)
                Text(red ? "REC" : armed ? "ARMED" : "OFF").font(Theme.caps).tracking(0.6)
                    .foregroundStyle(red ? Theme.recText : armed ? Theme.ink : Theme.mute)
            }
            .frame(maxWidth: .infinity).padding(.vertical, 4)
            .background(RoundedRectangle(cornerRadius: 6).fill(Theme.well))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(red ? Theme.rec.opacity(0.6) : Theme.hairline, lineWidth: red ? 1 : 0.5))
        }
        .buttonStyle(.plain)
        .allowsHitTesting(!locked)
        .help(locked ? "Stop recording to change which mics record"
              : armed ? "This mic will be recorded. Click to leave it out of Lavdeck’s recordings."
              : "Left out of Lavdeck’s recordings. The receiver still sends this mic to other apps; click to include it.")
        .accessibilityLabel("Record this mic")
        .accessibilityValue(armed ? "on" : "off")
    }
}

/// Record / Stop with its shortcut.
struct RecordButton: View {
    let recording: Bool
    var disabled = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                ZStack {
                    if recording {
                        RoundedRectangle(cornerRadius: 6).fill(Theme.ink).frame(width: 22, height: 22)
                    } else {
                        Circle().fill(Theme.rec).frame(width: 26, height: 26)
                            .overlay(Circle().strokeBorder(.black.opacity(0.15), lineWidth: 1))
                    }
                }
                .frame(width: 26, height: 26)
                VStack(alignment: .leading, spacing: 1) {
                    Text(recording ? "Stop" : "Record").font(Theme.labelBold).foregroundStyle(Theme.ink)
                    Text("⌥⌘R").font(Theme.label).foregroundStyle(Theme.mute)
                }
                Spacer(minLength: 0)
            }
            .padding(.top, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.45 : 1)
        .help(disabled ? "Arm at least one mic to record" : "")
        .overlay(alignment: .top) { Rectangle().fill(Theme.hairline).frame(height: 0.5) }
        .keyboardShortcut("r", modifiers: [.command, .option])
        .accessibilityLabel(recording ? "Stop recording" : "Start recording")
    }
}

/// Quick on/off switches for capture sources (screen, camera) that record alongside the audio.
/// On is ink like a selected mode segment; a source capturing in the live take turns record red.
struct CaptureSwitch: View {
    struct Item: Identifiable {
        let id: String
        let source: String
        let on: Bool
        let live: Bool
    }
    let items: [Item]
    /// Locked while recording: capture settings are read when a take starts.
    let locked: Bool
    let onSet: (Item, Bool) -> Void

    var body: some View {
        HStack(spacing: 6) {
            ForEach(items) { item in button(item) }
        }
        .allowsHitTesting(!locked)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Capture")
    }

    private func button(_ item: Item) -> some View {
        let red = locked && item.live
        let filled = item.on && !locked
        let foreground = red ? Theme.recText : filled ? Theme.onInk : item.on ? Theme.ink : Theme.mute
        return Button { onSet(item, !item.on) } label: {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 3) {
                    Image(systemName: Self.symbol(item.source, on: item.on)).font(.system(size: 9, weight: .bold))
                    Text(Self.title(item.source).uppercased()).font(.system(size: 10, weight: .bold)).tracking(0.3)
                }
                Text(Self.title(item.source).uppercased()).font(.system(size: 10, weight: .bold)).tracking(0.3)
                Image(systemName: Self.symbol(item.source, on: item.on)).font(.system(size: 12, weight: .semibold))
            }
            .lineLimit(1)
            .foregroundStyle(foreground)
            .frame(maxWidth: .infinity).padding(.vertical, 6)
            .background(RoundedRectangle(cornerRadius: 7).fill(filled ? Theme.ink : Theme.well))
            .overlay(RoundedRectangle(cornerRadius: 7)
                .strokeBorder(red ? Theme.rec.opacity(0.6) : Theme.hairline, lineWidth: red ? 1 : 0.5))
            .opacity(locked && !item.on ? 0.45 : 1)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(locked ? "Stop recording to change what’s captured"
              : item.on ? "\(Self.title(item.source)) records with each take. Click to turn it off."
              : "Click to record the \(Self.title(item.source).lowercased()) with each take.")
        .accessibilityLabel("Record \(Self.title(item.source).lowercased())")
        .accessibilityValue(item.on ? "on" : "off")
        .accessibilityAddTraits(.isToggle)
    }

    static func title(_ source: String) -> String {
        switch source {
        case "screen": "Screen"
        case "camera": "Camera"
        case "system-audio": "System audio"
        default: source.capitalized
        }
    }

    private static func symbol(_ source: String, on: Bool) -> String {
        switch source {
        case "screen": "display"
        case "camera": on ? "video.fill" : "video.slash"
        case "system-audio": on ? "speaker.wave.2.fill" : "speaker.slash"
        default: "record.circle"
        }
    }
}
