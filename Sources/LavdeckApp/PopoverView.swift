import LavdeckKit
import SwiftUI

/// The menu bar popover: two channel strips and a master section on a console deck.
struct PopoverView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            if model.identifying { IdentifyBanner() }
            if model.isConnected {
                HStack(alignment: .top, spacing: 6) {
                    ChannelStrip(index: 1)
                    ChannelStrip(index: 2)
                    MasterSection()
                }
                footer
                notes
            } else {
                absent
            }
        }
        .padding(10)
        .frame(width: 372)
        .background(Theme.deck)
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.16), value: model.isRecording)
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            Text("Wireless Mic Rx").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.ink)
            Spacer()
            if model.isRecording {
                HStack(spacing: 5) {
                    Circle().fill(Theme.rec).frame(width: 8, height: 8)
                    Text("REC").font(Theme.caps).foregroundStyle(Theme.recText)
                    if let started = model.recordingStarted {
                        Text(timerInterval: started...Date.distantFuture, countsDown: false)
                            .font(Theme.mono).foregroundStyle(Theme.recText)
                    }
                    ForEach(model.captureSources, id: \.self) { source in
                        Text("· \(source == "screen" ? "Screen" : source == "camera" ? "Camera" : source.capitalized)")
                            .font(Theme.label).foregroundStyle(Theme.mute)
                    }
                    // The mixer has its own Stop; when the receiver drops the mixer is replaced, so keep one here.
                    if !model.isConnected {
                    Button { model.stopRecording() } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "stop.fill").font(.system(size: 9, weight: .bold))
                            Text("Stop").font(Theme.labelBold)
                        }
                        .foregroundStyle(Theme.onInk)
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background(Capsule().fill(Theme.rec))
                    }
                    .buttonStyle(.plain)
                    .fixedSize()
                    .help("Stop recording (⌥⌘R)")
                    }
                }
            } else if model.isConnected && !model.identifying {
                Button { model.startIdentify() } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "waveform").font(.system(size: 11, weight: .semibold))
                        Text("Which mic is which?").font(Theme.labelBold)
                    }
                    .foregroundStyle(Theme.ink)
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(Capsule().fill(Theme.face))
                    .overlay(Capsule().strokeBorder(Theme.hairline, lineWidth: 0.5))
                }
                .buttonStyle(.plain)
                .help("Talk into a mic to see which strip it is. Receiver: Mobile RX, firmware \(model.receiver.identity.firmware ?? "unknown")")
            }
        }
        .padding(.horizontal, 4)
    }

    private var footer: some View {
        HStack {
            HStack(spacing: 5) {
                Image(systemName: "lock").font(.system(size: 10, weight: .semibold))
                Text("Noise cancel").foregroundStyle(Theme.mute)
                Text(model.noiseCancelLabel)
                    .fontWeight(.semibold).foregroundStyle(Theme.ink)
            }
            .help("Set with the O/L/H switch on the receiver")
            Spacer()
            Button(action: showRecordings) {
                HStack(spacing: 4) {
                    Text(model.format == .wav ? "WAV 24-bit" : "M4A").fontWeight(.semibold).foregroundStyle(Theme.ink)
                    if model.splitTracks { Text("· split tracks").foregroundStyle(Theme.mute) }
                    Image(systemName: "rectangle.stack").font(.system(size: 11)).foregroundStyle(Theme.mute)
                }
            }
            .buttonStyle(.plain)
            .help("Open Recordings")
            SettingsLink { Image(systemName: "gearshape").foregroundStyle(Theme.mute) }
                .buttonStyle(.plain)
                .help("Settings")
        }
        .font(Theme.label)
        .foregroundStyle(Theme.mute)
        .padding(.horizontal, 4)
    }

    private func showRecordings() {
        NSApp.activate()
        openWindow(id: "recordings")
    }

    @ViewBuilder private var notes: some View {
        ForEach(model.plugins.runningJobs) { job in
            JobNote(job: job, open: showRecordings)
        }
        if let failed = model.plugins.jobs.last(where: { if case .failed = $0.state { true } else { false } }),
           case .failed(let reason) = failed.state, reason != "Cancelled" {
            Note(text: "\(failed.pluginName) failed on the \(failed.takeID.split(separator: "/").last ?? "") take: \(reason)",
                 symbol: "exclamationmark.triangle.fill")
        }
        if let error = model.lastError {
            Note(text: error, symbol: "exclamationmark.triangle.fill")
        }
        ForEach(model.droppedTransmitters, id: \.self) { index in
            Note(text: "TX\(index) disconnected from the receiver. It reconnects on its own when back in range or powered on.",
                 symbol: "antenna.radiowaves.left.and.right.slash")
        }
        ForEach(model.lowBattery, id: \.self) { index in
            Note(text: "TX\(index) battery looks low (receiver estimate). Charge it soon.", symbol: "battery.25percent")
        }
        if model.soloMic != nil, !model.isRecording, model.receiver.channelMode != .stereo {
            Note(text: "Only TX\(model.soloMic!) is armed. Recording will switch the receiver to stereo so that mic records alone.",
                 symbol: "info.circle.fill")
        }
        ForEach(model.outdatedFirmware, id: \.index) { item in
            Note(text: "TX\(item.index) firmware \(item.firmware) is behind TX\(item.newestIndex) (\(item.newest)). Update it in DJI Mimo.",
                 symbol: "arrow.down.circle.fill")
        }
    }

    private var absent: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Receiver not connected").font(Theme.labelBold).foregroundStyle(Theme.ink)
            Text("Plug the DJI Mic receiver into this Mac. A hub needs to carry USB data, not just power.")
                .font(Theme.label).foregroundStyle(Theme.mute).fixedSize(horizontal: false, vertical: true)
            Button("Open Recordings", action: showRecordings)
                .buttonStyle(.link).font(Theme.label).padding(.top, 2)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 9).fill(Theme.face))
    }
}

private struct IdentifyBanner: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: "waveform").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.green)
            VStack(alignment: .leading, spacing: 3) {
                Text(model.talkingMic.map { "That’s \(model.displayName($0)) (TX\($0))." } ?? "Talk into one mic. Its strip lights up.")
                    .font(Theme.labelBold).foregroundStyle(Theme.ink)
                Text("Click a strip’s name to label it. Stereo is on while this is open; your setting returns when you’re done.")
                    .font(Theme.label).foregroundStyle(Theme.mute).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
            Button("Done") { model.stopIdentify() }
                .buttonStyle(.plain)
                .font(Theme.labelBold).foregroundStyle(Theme.onInk)
                .padding(.horizontal, 10).padding(.vertical, 4)
                .background(Capsule().fill(Theme.ink))
                .keyboardShortcut(.cancelAction)
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 9).fill(Theme.face)
            .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.green.opacity(0.6), lineWidth: 1)))
        .animation(.easeOut(duration: 0.16), value: model.talkingMic)
    }
}

private struct Note: View {
    let text: String
    let symbol: String

    var body: some View {
        HStack(alignment: .top, spacing: 7) {
            Image(systemName: symbol).font(.system(size: 11))
            Text(text).fixedSize(horizontal: false, vertical: true)
        }
        .font(Theme.label)
        .foregroundStyle(Theme.warnText)
        .padding(.horizontal, 9).padding(.vertical, 7)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 7).fill(Theme.warnFill))
    }
}

private struct ChannelStrip: View {
    @Environment(AppModel.self) private var model
    let index: Int
    @State private var editing = false
    @State private var draft = ""
    @FocusState private var fieldFocused: Bool

    var body: some View {
        let tx = model.transmitter(index)
        let linked = model.isLinked(index)
                let outdated = model.outdatedFirmware.contains { $0.index == index }
        let armed = model.armed.contains(index)
        let name = model.displayName(index)
        let named = name != "TX\(index)"
        let talking = model.identifying && model.talkingMic == index
        VStack(spacing: 8) {
            if model.isRecording, let file = model.destination(for: index) {
                NamePlate(title: model.recordingMode == .stereo ? "\(name) · \(index == 1 ? "L" : "R")" : name,
                          subtitle: "→ \(file)", armed: true, dimmed: !linked)
            } else if model.isRecording {
                NamePlate(title: name, tag: named ? "TX\(index)" : nil, hollow: true)
            } else if editing {
                TextField("TX\(index)", text: $draft)
                    .textFieldStyle(.plain)
                    .font(Theme.labelBold)
                    .foregroundStyle(Theme.plateText)
                    .focused($fieldFocused)
                    .onSubmit { commit() }
                    .onExitCommand { editing = false }
                    .padding(.horizontal, 7).padding(.vertical, 6)
                    .background(RoundedRectangle(cornerRadius: 5).fill(Theme.plate))
                    .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(Theme.green, lineWidth: 1.5))
                    .onAppear { fieldFocused = true }
                    .onChange(of: fieldFocused) { _, focused in if !focused { commit() } }
            } else {
                Button {
                    draft = named ? name : ""
                    editing = true
                } label: {
                    NamePlate(title: name, tag: named ? "TX\(index)" : nil, dimmed: !linked)
                }
                .buttonStyle(.plain)
                .help("Click to name this mic (the name follows this physical mic)")
                .disabled(tx?.identity.bluetoothID == nil)
            }
            ArmButton(armed: armed, live: model.isRecording) { model.toggleArm(index) }
            LinkLED(state: linked ? .linked : model.knownTransmitters.contains(index) ? .dropped : .off)
            LEDMeter(level: model.meter(index), inactive: !armed && !model.identifying)
                .overlay(alignment: .bottom) {
                    if armed, linked, model.metersAreMix, !model.identifying {
                        Text("mono mix").font(.system(size: 10, weight: .semibold)).foregroundStyle(Theme.mute).fixedSize()
                            .padding(.horizontal, 4).background(Theme.face).offset(y: 9)
                    }
                }
                .padding(.bottom, armed && linked && model.metersAreMix && !model.identifying ? 10 : 0)
                .help(!armed ? "Not armed: this mic is left out of recordings"
                      : model.metersAreMix ? "Mono mixes both mics, so this shows the mix. Switch to stereo for each mic’s own level."
                      : "This mic’s level, reported by the receiver")
            BatteryCells(level: linked ? tx?.batteryLevel : nil)
            Group {
                if model.isRecording, !armed {
                    Text("not rec")
                } else if let db = model.dBFS(index) {
                    Text(db <= -90 ? "silent" : "\(db) dBFS")
                } else if let fw = tx?.identity.firmware {
                    Text(fw.split(separator: ".").prefix(3).joined(separator: "."))
                        .padding(.horizontal, outdated ? 5 : 0).padding(.vertical, outdated ? 2 : 0)
                        .background(RoundedRectangle(cornerRadius: 4).fill(outdated ? Theme.warnFill : .clear))
                        .foregroundStyle(outdated ? Theme.warnText : Theme.mute)
                        .help("Firmware \(fw)")
                } else {
                    Text(linked ? "…" : "not linked")
                }
            }
            .font(Theme.mono)
            .foregroundStyle(Theme.mute)
            .lineLimit(1)
            .fixedSize()
        }
        .padding(8)
        .frame(width: 100)
        .background(RoundedRectangle(cornerRadius: 9).fill(talking ? Theme.green.opacity(0.12) : Theme.face)
            .overlay(RoundedRectangle(cornerRadius: 9)
                .strokeBorder(talking ? Theme.green : Theme.hairline, lineWidth: talking ? 2 : 0.5)))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(name), transmitter \(index)\(talking ? ", talking" : "")")
    }

    private func commit() {
        guard editing else { return }
        model.rename(index, to: draft)
        editing = false
    }
}

private struct MasterSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if model.isRecording, let started = model.recordingStarted {
                TimelineView(.periodic(from: started, by: 1)) { context in
                    let elapsed = Int(context.date.timeIntervalSince(started))
                    VStack(spacing: 1) {
                        Text(String(format: "%02d:%02d:%02d", elapsed / 3600, elapsed / 60 % 60, elapsed % 60))
                            .font(.system(size: 20, weight: .semibold)).monospacedDigit().foregroundStyle(Theme.ink)
                        Text("started \(started.formatted(date: .omitted, time: .shortened))").font(Theme.label).foregroundStyle(Theme.mute)
                    }
                    .frame(maxWidth: .infinity)
                }
            } else {
                Text("GAIN").capsLabel()
            }
            GainFader(steps: [-12, -6, 0, 6, 12], value: model.displayedGain, pending: model.pendingGain != nil,
                      onSet: model.setGain, height: model.isRecording ? 84 : 132)
            ModeSwitch(mode: display(model.displayedChannel), disabled: model.isRecording || model.pendingChannel != nil) { stereo in
                model.setChannel(stereo ? .stereo : .mono)
            }
            .help(model.isRecording ? "Stop recording to change the channel mode" : "Mono mixes both mics; stereo puts TX1 left and TX2 right")
            RecordButton(recording: model.isRecording, disabled: !model.isRecording && model.armed.isEmpty,
                         action: model.toggleRecording)
        }
        .padding(8)
        .frame(maxWidth: .infinity)
        .background(RoundedRectangle(cornerRadius: 9).fill(Theme.face)
            .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.hairline, lineWidth: 0.5)))
    }

    private func display(_ mode: ChannelMode?) -> ChannelModeDisplay {
        switch mode { case .mono: .mono; case .stereo: .stereo; default: .unknown }
    }
}

/// A running plugin job, e.g. a transcript being generated for the last take.
private struct JobNote: View {
    @Environment(AppModel.self) private var model
    let job: PluginHost.Job
    let open: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            ProgressView(value: job.fraction).progressViewStyle(.circular).controlSize(.small)
            VStack(alignment: .leading, spacing: 1) {
                Text(job.title).font(Theme.labelBold).foregroundStyle(Theme.ink)
                Text(job.message ?? "Working").font(Theme.label).foregroundStyle(Theme.mute).lineLimit(1)
            }
            Spacer()
            Button("Show", action: open).buttonStyle(.link).font(Theme.label)
            Button { model.plugins.cancel(job) } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(Theme.mute) }
                .buttonStyle(.plain).help("Cancel")
        }
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 8).fill(Theme.face))
    }
}
