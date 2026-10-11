import AppKit
import SwiftUI

/// Black scribble strip naming a channel; turns red and names its file while recording.
struct NamePlate: View {
    let title: String
    let subtitle: String
    var armed = false
    var dimmed = false
    /// A channel left out of the current take: drawn hollow so it stays readable.
    var hollow = false

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(title).font(Theme.labelBold).tracking(0.5).foregroundStyle(hollow ? Theme.ink : Theme.plateText)
            Text(subtitle).font(Theme.mono).foregroundStyle(hollow ? Theme.mute : armed ? Color(hex: 0xFFE1DE) : Theme.plateSub)
                .lineLimit(1).truncationMode(.middle)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 7).padding(.vertical, 5)
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
    private let segments = 12
    @State private var peak = 0.0
    @State private var peakTime = Date.distantPast
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let lit = Int((level * Double(segments)).rounded(.down))
        let peakIndex = Int((peak * Double(segments)).rounded(.down)) - 1
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
        .animation(reduceMotion ? nil : .easeOut(duration: 0.08), value: lit)
        .onChange(of: level) { _, new in
            if new >= peak || Date().timeIntervalSince(peakTime) > 1.2 { peak = new; peakTime = Date() }
        }
        .accessibilityElement()
        .accessibilityLabel("Level")
        .accessibilityValue("\(Int(level * 100)) percent")
    }

    private func color(for index: Int) -> Color {
        index >= 10 ? Theme.red : index >= 8 ? Theme.amber : Theme.green
    }
}

/// DJI reports battery as 0-7; shown as seven cells and labelled as an estimate.
struct BatteryCells: View {
    let level: Int?

    var body: some View {
        HStack(spacing: 2) {
            ForEach(0..<7, id: \.self) { i in
                RoundedRectangle(cornerRadius: 1.5)
                    .fill(level.map { i < $0 } == true ? Theme.ink : Theme.ledOff)
                    .frame(width: 5, height: 10)
            }
            Text("est").font(.system(size: 10, weight: .medium)).foregroundStyle(Theme.mute).padding(.leading, 2)
        }
        .help(level.map { "Battery \($0) of 7 (receiver estimate)" } ?? "Battery unknown")
        .accessibilityElement()
        .accessibilityLabel("Battery")
        .accessibilityValue(level.map { "\($0) of 7, estimate" } ?? "unknown")
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
        .help(locked ? "Stop recording to change which mics record" : armed ? "Recording this mic. Click to leave it out." : "Not recording this mic. Click to include it.")
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
