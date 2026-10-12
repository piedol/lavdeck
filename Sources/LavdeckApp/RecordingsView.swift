import AVFoundation
import AppKit
import LavdeckKit
import LavdeckPluginKit
import SwiftUI

/// Recordings manager: browse, filter, sort, search, play, transcribe and delete takes.
struct RecordingsView: View {
    @Environment(AppModel.self) private var model
    @State private var takes: [Take] = []
    @State private var transcripts: [String: TranscriptDocument] = [:]
    @State private var selection: Take.ID?
    @State private var search = ""
    @State private var filter: Filter = .all
    @State private var sortOrder = [KeyPathComparator(\Row.started, order: .reverse)]
    @State private var pendingDelete: Take?
    @StateObject private var player = TakePlayer()

    enum Filter: String, CaseIterable, Identifiable {
        case all = "All", transcribed = "Transcribed", untranscribed = "Not transcribed", twoSpeaker = "Two speakers"
        var id: String { rawValue }
    }

    struct Row: Identifiable {
        let take: Take
        var id: String { take.id }
        var started: Date { take.descriptor.started ?? .distantPast }
        var duration: Double { take.descriptor.duration ?? 0 }
        var mode: String {
            if take.descriptor.isTwoSpeaker { return "Two speakers" }
            if take.descriptor.files.contains(where: { $0.role == .stereo }) { return "Stereo" }
            if take.descriptor.files.contains(where: { $0.role == .mix }) { return "Mono" }
            return "Single mic"
        }
        var speakers: String { take.descriptor.speakers.map(\.name).joined(separator: ", ") }
        var transcribed: Int { take.hasTranscript ? 1 : 0 }
    }

    private var rows: [Row] {
        let query = search.trimmingCharacters(in: .whitespaces).lowercased()
        return takes.map(Row.init).filter { row in
            switch filter {
            case .all: break
            case .transcribed: if !row.take.hasTranscript { return false }
            case .untranscribed: if row.take.hasTranscript { return false }
            case .twoSpeaker: if !row.take.descriptor.isTwoSpeaker { return false }
            }
            guard !query.isEmpty else { return true }
            if row.speakers.lowercased().contains(query) || row.take.id.contains(query) { return true }
            return transcripts[row.id]?.plainText.lowercased().contains(query) == true
        }.sorted(using: sortOrder)
    }

    private var selectedTake: Take? { takes.first { $0.id == selection } }

    var body: some View {
        HSplitView {
            table
                .frame(minWidth: 420, idealWidth: 480)
            Group {
                if let take = selectedTake {
                    TakeDetail(take: take, transcript: transcripts[take.id], search: search, player: player,
                               onDelete: { pendingDelete = take })
                } else {
                    ContentUnavailableView(takes.isEmpty ? "No recordings yet" : "Select a recording",
                                           systemImage: "waveform",
                                           description: Text(takes.isEmpty ? "Takes you record with Lavdeck appear here." : ""))
                }
            }
            .frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 900, minHeight: 560)
        .searchable(text: $search, placement: .toolbar, prompt: "Search transcripts and names")
        .toolbar {
            ToolbarItem(placement: .navigation) {
                Menu {
                    Picker("Show", selection: $filter) {
                        ForEach(Filter.allCases) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.inline)
                } label: {
                    Label(filter.rawValue, systemImage: filter == .all ? "line.3.horizontal.decrease.circle" : "line.3.horizontal.decrease.circle.fill")
                        .labelStyle(.titleAndIcon)
                }
                .fixedSize()
                .help("Filter recordings")
            }
            ToolbarItem {
                Button { reload() } label: { Label("Refresh", systemImage: "arrow.clockwise") }
            }
            ToolbarItem {
                Button { model.revealRecordings() } label: { Label("Show in Finder", systemImage: "folder") }
            }
        }
        .navigationTitle("Recordings")
        .navigationSubtitle("\(rows.count) of \(takes.count)")
        .onAppear(perform: reload)
        .onChange(of: model.libraryRevision) { reload() }
        .onChange(of: model.plugins.outputsRevision) { reload() }
        .confirmationDialog("Move this recording to the Trash?", isPresented: Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } }),
                            presenting: pendingDelete) { take in
            Button("Move \(take.descriptor.files.count) files to Trash", role: .destructive) { delete(take) }
        } message: { take in
            Text("All files of the \(take.descriptor.stem) take, including transcripts, go to the Trash. You can restore them from there.")
        }
    }

    private var table: some View {
        Table(rows, selection: $selection, sortOrder: $sortOrder) {
            TableColumn("Recorded", value: \.started) { row in
                VStack(alignment: .leading, spacing: 1) {
                    Text(row.started.formatted(.dateTime.month(.abbreviated).day().hour().minute()))
                    if let job = model.plugins.job(for: row.id) {
                        ProgressView(value: job.fraction) { Text(job.message ?? job.title).font(.caption2) }
                            .controlSize(.mini)
                    }
                }
            }
            .width(min: 120, ideal: 140)
            TableColumn("Length", value: \.duration) { row in
                Text(Self.length(row.duration)).monospacedDigit()
            }
            .width(min: 56, ideal: 64)
            TableColumn("Mode", value: \.mode) { row in Text(row.mode) }
                .width(min: 80, ideal: 100)
            TableColumn("Mics", value: \.speakers) { row in Text(row.speakers.isEmpty ? "—" : row.speakers) }
            TableColumn("Transcript", value: \.transcribed) { row in
                Image(systemName: row.take.hasTranscript ? "text.quote" : "minus")
                    .foregroundStyle(row.take.hasTranscript ? Color.primary : Color.secondary)
                    .accessibilityLabel(row.take.hasTranscript ? "Has transcript" : "No transcript")
            }
            .width(min: 64, ideal: 72)
        }
        .contextMenu(forSelectionType: Take.ID.self) { ids in
            if let id = ids.first, let take = takes.first(where: { $0.id == id }) {
                Button("Show in Finder") { reveal(take) }
                ForEach(model.plugins.actions, id: \.action.id) { item in
                    Button(item.action.title) { model.plugins.run(item.action, of: item.plugin, on: take) }
                }
                Divider()
                Button("Move to Trash…", role: .destructive) { pendingDelete = take }
            }
        }
        .onDeleteCommand { if let take = selectedTake { pendingDelete = take } }
    }

    private func reload() {
        takes = model.scanTakes()
        var docs: [String: TranscriptDocument] = [:]
        for take in takes where take.hasTranscript {
            if let doc = try? TranscriptDocument.load(take.transcriptURL) { docs[take.id] = doc }
        }
        transcripts = docs
        if selection == nil || !takes.contains(where: { $0.id == selection }) { selection = takes.first?.id }
    }

    private func delete(_ take: Take) {
        player.stop()
        do { try TakeLibrary.trash(take) } catch {
            NSAlert(error: error).runModal()
        }
        pendingDelete = nil
        model.noteLibraryChanged()
        reload()
    }

    private func reveal(_ take: Take) {
        NSWorkspace.shared.activateFileViewerSelecting(take.audioFiles.map { URL(fileURLWithPath: $0.path) })
    }

    static func length(_ seconds: Double) -> String {
        let s = Int(seconds.rounded())
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }
}

// MARK: - Detail

private struct TakeDetail: View {
    @Environment(AppModel.self) private var model
    let take: Take
    let transcript: TranscriptDocument?
    let search: String
    @ObservedObject var player: TakePlayer
    let onDelete: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(16)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    files
                    transcriptSection
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .onChange(of: take.id) { player.stop() }
    }

    private var header: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 4) {
                Text(take.descriptor.started?.formatted(date: .complete, time: .shortened) ?? take.id)
                    .font(.title3.weight(.semibold))
                HStack(spacing: 6) {
                    Text(RecordingsView.length(take.descriptor.duration ?? 0)).monospacedDigit()
                    ForEach(take.descriptor.speakers, id: \.id) { speaker in
                        NamePlate(title: speaker.name, tag: speaker.name == "TX\(speaker.mic)" ? nil : "TX\(speaker.mic)")
                            .fixedSize()
                    }
                }
                .font(.callout)
                .foregroundStyle(.secondary)
            }
            Spacer()
            HStack(spacing: 8) {
                Button {
                    player.toggle(take.descriptor.mainAudio.map { URL(fileURLWithPath: $0.path) })
                } label: {
                    Label(player.isPlaying ? "Pause" : "Play", systemImage: player.isPlaying ? "pause.fill" : "play.fill")
                }
                .keyboardShortcut(.space, modifiers: [])
                ForEach(model.plugins.appPlugins) { plugin in
                    Button {
                        model.plugins.open(take, in: plugin)
                    } label: {
                        Label(plugin.manifest.app?.actionTitle ?? "Open in \(plugin.manifest.name)", systemImage: "film.stack")
                    }
                }
                Menu {
                    Button("Show in Finder") {
                        NSWorkspace.shared.activateFileViewerSelecting(take.allURLs)
                    }
                    ForEach(model.plugins.actions, id: \.action.id) { item in
                        Button(item.action.title) { model.plugins.run(item.action, of: item.plugin, on: take) }
                    }
                    Divider()
                    Button("Move to Trash…", role: .destructive, action: onDelete)
                } label: {
                    Label("More", systemImage: "ellipsis.circle")
                }
                .fixedSize()
            }
        }
    }

    private var files: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("FILES").capsLabel()
            ForEach(take.descriptor.files.filter { $0.role != .metadata }, id: \.path) { file in
                HStack {
                    Image(systemName: file.kind == "transcript" ? "doc.text" : file.kind == "video" ? "film" : "waveform")
                        .foregroundStyle(.secondary).frame(width: 16)
                    Text((file.path as NSString).lastPathComponent).lineLimit(1).truncationMode(.middle)
                    Spacer()
                    Text(Self.label(for: file)).font(.caption).foregroundStyle(.secondary)
                }
                .font(.callout)
                .contentShape(Rectangle())
                .onTapGesture(count: 2) { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: file.path)]) }
            }
        }
    }

    @ViewBuilder private var transcriptSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("TRANSCRIPT").capsLabel()
                Spacer()
                if let job = model.plugins.job(for: take.id) {
                    ProgressView(value: job.fraction).frame(width: 120).controlSize(.small)
                    Text(job.message ?? job.title).font(.caption).foregroundStyle(.secondary)
                } else if transcript != nil, let item = transcribeAction {
                    Button("Regenerate") { model.plugins.run(item.action, of: item.plugin, on: take) }
                        .controlSize(.small)
                }
            }
            if let transcript {
                TranscriptList(transcript: transcript, take: take, search: search, player: player)
            } else if model.plugins.job(for: take.id) == nil {
                VStack(alignment: .leading, spacing: 8) {
                    Text("No transcript for this take yet.").foregroundStyle(.secondary)
                    if let item = transcribeAction {
                        Button {
                            model.plugins.run(item.action, of: item.plugin, on: take)
                        } label: { Label(item.action.title, systemImage: "text.quote") }
                    } else {
                        Text("Turn on a transcription plugin in Settings › Plugins to transcribe recordings.")
                            .font(.callout).foregroundStyle(.secondary)
                        SettingsLink { Text("Open Settings") }
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 8).fill(Theme.face))
            }
            if let job = model.plugins.jobs.last(where: { $0.takeID == take.id }), case .failed(let reason) = job.state {
                Text("\(job.pluginName) failed: \(reason)").font(.callout).foregroundStyle(Theme.warnText)
            }
        }
    }

    private var transcribeAction: (plugin: InstalledPlugin, action: PluginAction)? {
        model.plugins.actions.first { $0.plugin.manifest.produces?.contains("transcript") == true }
    }

    static func label(for file: TakeFile) -> String {
        switch file.role {
        case .stereo: "Stereo (TX1 left, TX2 right)"
        case .mix: "Mono mix"
        case .track: "TX\(file.mic ?? 0) track"
        case .derived:
            file.source == "pip" ? "Picture-in-picture video"
                : file.kind == "transcript" ? "Transcript" : (file.kind ?? "Plugin output")
        case .metadata: "Metadata"
        case .capture:
            switch file.source {
            case "screen": "Screen recording"
            case "camera": "Camera"
            case "system-audio": "System audio"
            default: "Captured \(file.kind ?? "media")"
            }
        }
    }
}

private struct TranscriptList: View {
    let transcript: TranscriptDocument
    let take: Take
    let search: String
    @ObservedObject var player: TakePlayer

    var body: some View {
        let multi = transcript.speakers.count > 1
        let query = search.trimmingCharacters(in: .whitespaces)
        let long = (transcript.segments.last?.end ?? 0) >= 3600
        VStack(alignment: .leading, spacing: 2) {
            ForEach(transcript.segments) { segment in
                let hit = !query.isEmpty && segment.text.localizedCaseInsensitiveContains(query)
                Button {
                    player.play(url(for: segment.speaker), from: segment.start)
                } label: {
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Text(long ? TranscriptDocument.timestamp(segment.start) : RecordingsView.length(segment.start.rounded(.down)))
                            .font(Theme.mono).frame(minWidth: 36, alignment: .trailing).foregroundStyle(.secondary)
                        if multi {
                            Text(transcript.speakerName(segment.speaker))
                                .font(Theme.labelBold).frame(width: 64, alignment: .leading).lineLimit(1)
                        }
                        Text(highlighted(segment.text, query))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .multilineTextAlignment(.leading)
                    }
                    .padding(.vertical, 4).padding(.horizontal, 6)
                    .background(RoundedRectangle(cornerRadius: 6).fill(hit ? Theme.amber.opacity(0.14) :
                        player.isPlaying && player.isInside(segment) ? Theme.green.opacity(0.10) : .clear))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Play from \(TranscriptDocument.timestamp(segment.start))")
            }
            Text("\(transcript.model)\(transcript.language.map { " · \($0)" } ?? "")")
                .font(.caption).foregroundStyle(.tertiary).padding(.top, 6)
        }
        .textSelection(.enabled)
    }

    private func url(for speaker: String) -> URL? {
        if let track = take.descriptor.speakers.first(where: { $0.id == speaker })?.file { return URL(fileURLWithPath: track) }
        return take.descriptor.mainAudio.map { URL(fileURLWithPath: $0.path) }
    }

    private func highlighted(_ text: String, _ query: String) -> AttributedString {
        var out = AttributedString(text)
        guard !query.isEmpty else { return out }
        var searchStart = out.startIndex
        while let range = out[searchStart...].range(of: query, options: .caseInsensitive) {
            out[range].font = .body.weight(.semibold)
            out[range].backgroundColor = Theme.amber.opacity(0.35)
            searchStart = range.upperBound
        }
        return out
    }
}

/// Plays one take file, from a given time.
@MainActor
final class TakePlayer: ObservableObject {
    @Published private(set) var isPlaying = false
    @Published private(set) var currentTime: Double = 0
    private var player: AVAudioPlayer?
    private var url: URL?
    private var timer: Timer?

    func toggle(_ url: URL?) {
        if isPlaying { pause() } else { play(url, from: player?.currentTime ?? 0) }
    }

    func play(_ url: URL?, from time: Double) {
        guard let url else { return }
        if self.url != url || player == nil {
            player = try? AVAudioPlayer(contentsOf: url)
            self.url = url
        }
        player?.currentTime = time
        player?.play()
        isPlaying = player?.isPlaying ?? false
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self, let p = self.player else { return }
                self.currentTime = p.currentTime
                if !p.isPlaying { self.isPlaying = false; self.timer?.invalidate() }
            }
        }
    }

    func pause() {
        player?.pause()
        isPlaying = false
    }

    func stop() {
        player?.stop()
        player = nil
        url = nil
        isPlaying = false
        timer?.invalidate()
    }

    func isInside(_ segment: TranscriptDocument.Segment) -> Bool {
        currentTime >= segment.start && currentTime < segment.end
    }
}
