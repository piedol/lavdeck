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
    /// ⌘-click adds or removes a take, ⇧-click selects a range, ⌘A selects every visible take.
    @State private var selection: Set<Take.ID> = []
    @State private var search = ""
    @State private var filter: Filter = .all
    @State private var sortOrder = [KeyPathComparator(\Row.started, order: .reverse)]
    @State private var pendingDelete: [Take]?
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

    /// Selected takes the current filter and search show, in table order. Hidden takes stay selected
    /// but are never acted on, so a filter can't hide what Move to Trash will remove.
    private var selectedTakes: [Take] { rows.filter { selection.contains($0.id) }.map(\.take) }

    var body: some View {
        HSplitView {
            table
                .frame(minWidth: 420, idealWidth: 480)
            Group {
                let selected = selectedTakes
                if selected.count == 1, let take = selected.first {
                    TakeDetail(take: take, transcript: transcripts[take.id], search: search, player: player,
                               onDelete: { pendingDelete = [take] })
                } else if selected.count > 1 {
                    SelectionDetail(takes: selected, transcripts: transcripts,
                                    onSelect: { selection = [$0.id] },
                                    onCopyTranscripts: { copyTranscripts(selected) },
                                    onDelete: { pendingDelete = selected })
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
        .navigationSubtitle(selectedTakes.count > 1 ? "\(selectedTakes.count) selected · \(rows.count) of \(takes.count)" : "\(rows.count) of \(takes.count)")
        .onAppear(perform: reload)
        .onChange(of: model.libraryRevision) { reload() }
        .onChange(of: model.plugins.outputsRevision) { reload() }
        .confirmationDialog(pendingDelete?.count ?? 0 > 1 ? "Move \(pendingDelete?.count ?? 0) recordings to the Trash?" : "Move this recording to the Trash?",
                            isPresented: Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } }),
                            presenting: pendingDelete) { doomed in
            Button("Move \(doomed.reduce(0) { $0 + $1.descriptor.files.count }) files to Trash", role: .destructive) { delete(doomed) }
        } message: { doomed in
            Text(doomed.count > 1
                 ? "All files of these \(doomed.count) takes, including transcripts, go to the Trash. You can restore them from there."
                 : "All files of the \(doomed.first?.descriptor.stem ?? "") take, including transcripts, go to the Trash. You can restore them from there.")
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
        // Right-clicking inside the selection acts on all of it; right-clicking elsewhere acts on that row.
        .contextMenu(forSelectionType: Take.ID.self) { ids in
            let chosen = rows.filter { ids.contains($0.id) }.map(\.take)
            if !chosen.isEmpty {
                let suffix = chosen.count > 1 ? " (\(chosen.count))" : ""
                Button("Show in Finder") { reveal(chosen) }
                if chosen.contains(where: \.hasTranscript) {
                    Button("Copy Transcript\(chosen.count > 1 ? "s" : "")") { copyTranscripts(chosen) }
                }
                if !model.plugins.actions.isEmpty { Divider() }
                ForEach(model.plugins.actions, id: \.action.id) { item in
                    Button(item.action.title + suffix) { model.plugins.run(item.action, of: item.plugin, on: chosen) }
                }
                Divider()
                Button("Move to Trash…", role: .destructive) { pendingDelete = chosen }
            }
        } primaryAction: { ids in
            // Double-click plays a single take.
            if ids.count == 1, let take = rows.first(where: { ids.contains($0.id) })?.take {
                player.play(take.descriptor.mainAudio.map { URL(fileURLWithPath: $0.path) }, from: 0)
            }
        }
        .onDeleteCommand { if !selectedTakes.isEmpty { pendingDelete = selectedTakes } }
        .onExitCommand { if selection.count > 1, let first = selectedTakes.first { selection = [first.id] } }
    }

    private func reload() {
        takes = model.scanTakes()
        var docs: [String: TranscriptDocument] = [:]
        for take in takes where take.hasTranscript {
            if let doc = try? TranscriptDocument.load(take.transcriptURL) { docs[take.id] = doc }
        }
        transcripts = docs
        selection = selection.filter { id in takes.contains { $0.id == id } }
        if selection.isEmpty, let first = rows.first { selection = [first.id] }
    }

    private func delete(_ doomed: [Take]) {
        player.stop()
        // Afterwards select the row that took the first removed take's place, so the list keeps its spot.
        let ids = Set(doomed.map(\.id))
        let visible = rows.map(\.id)
        let next = visible.drop { !ids.contains($0) }.first { !ids.contains($0) }
            ?? visible.last { !ids.contains($0) }
        for take in doomed {
            do { try TakeLibrary.trash(take) } catch {
                NSAlert(error: error).runModal()
                break
            }
        }
        pendingDelete = nil
        selection = next.map { [$0] } ?? []
        model.noteLibraryChanged()
        reload()
    }

    private func reveal(_ chosen: [Take]) {
        NSWorkspace.shared.activateFileViewerSelecting(chosen.flatMap { $0.audioFiles.map { URL(fileURLWithPath: $0.path) } })
    }

    /// Puts the takes' transcripts on the clipboard as text, oldest first, each under its date.
    private func copyTranscripts(_ chosen: [Take]) {
        let ordered = chosen.sorted { ($0.descriptor.started ?? .distantPast) < ($1.descriptor.started ?? .distantPast) }
        let parts = ordered.compactMap { take -> String? in
            guard let doc = transcripts[take.id] else { return nil }
            let multi = doc.speakers.count > 1
            let lines = doc.segments.map { segment in
                "[\(TranscriptDocument.timestamp(segment.start))] \(multi ? doc.speakerName(segment.speaker) + ": " : "")\(segment.text)"
            }
            let title = take.descriptor.started?.formatted(date: .complete, time: .shortened) ?? take.id
            return ordered.count > 1 ? "\(title)\n\n" + lines.joined(separator: "\n") : lines.joined(separator: "\n")
        }
        guard !parts.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(parts.joined(separator: "\n\n"), forType: .string)
    }

    static func length(_ seconds: Double) -> String {
        let s = Int(seconds.rounded())
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }
}

// MARK: - Detail

private struct TakeDetail: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openSettings) private var openSettings
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

    /// Date and actions side by side when they fit; actions drop below the date in a narrow pane.
    private var header: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: 16) {
                info
                Spacer(minLength: 0)
                buttons
            }
            VStack(alignment: .leading, spacing: 10) {
                info
                buttons
            }
        }
    }

    private var info: some View {
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
        .fixedSize()
    }

    private var buttons: some View {
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
                if transcript != nil {
                    Button("Copy Transcript") { copyTranscript() }
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
        .fixedSize()
    }

    private func copyTranscript() {
        guard let transcript else { return }
        let multi = transcript.speakers.count > 1
        let text = transcript.segments.map { segment in
            "[\(TranscriptDocument.timestamp(segment.start))] \(multi ? transcript.speakerName(segment.speaker) + ": " : "")\(segment.text)"
        }.joined(separator: "\n")
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
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
                        Button("Open Settings") { NSApp.activate(); openSettings() }
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

// MARK: - Several takes

/// Detail pane for a multi-selection: what's selected, and actions that run on all of it.
private struct SelectionDetail: View {
    @Environment(AppModel.self) private var model
    let takes: [Take]
    let transcripts: [String: TranscriptDocument]
    let onSelect: (Take) -> Void
    let onCopyTranscripts: () -> Void
    let onDelete: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header.padding(16)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if !activeJobs.isEmpty { progress }
                    actions
                    list
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private var transcribed: Int { takes.filter(\.hasTranscript).count }
    private var untranscribed: [Take] { takes.filter { !$0.hasTranscript } }
    private var activeJobs: [PluginHost.Job] {
        let ids = Set(takes.map(\.id))
        return model.plugins.jobs.filter { ids.contains($0.takeID) && [.running, .queued].contains($0.state) }
    }
    private var transcribeAction: (plugin: InstalledPlugin, action: PluginAction)? {
        model.plugins.actions.first { $0.plugin.manifest.produces?.contains("transcript") == true }
    }

    private var header: some View {
        let dates = takes.compactMap(\.descriptor.started).sorted()
        let total = takes.reduce(0) { $0 + ($1.descriptor.duration ?? 0) }
        var span = ""
        if let first = dates.first, let last = dates.last {
            let day = Date.FormatStyle.dateTime.month(.abbreviated).day()
            span = Calendar.current.isDate(first, inSameDayAs: last)
                ? first.formatted(day) : "\(first.formatted(day)) – \(last.formatted(day))"
        }
        return VStack(alignment: .leading, spacing: 4) {
            Text("\(takes.count) recordings").font(.title3.weight(.semibold))
            Text([span, "\(RecordingsView.length(total)) total", "\(transcribed) of \(takes.count) transcribed"]
                .filter { !$0.isEmpty }.joined(separator: " · "))
                .font(.callout).foregroundStyle(.secondary)
        }
    }

    private var progress: some View {
        let running = activeJobs.filter { $0.state == .running }
        let waiting = activeJobs.count - running.count
        return HStack(spacing: 10) {
            ProgressView(value: running.first?.fraction ?? 0).progressViewStyle(.circular).controlSize(.small)
            VStack(alignment: .leading, spacing: 1) {
                Text(running.first.map { "\($0.title): \($0.message ?? "working")" } ?? "Waiting to start")
                    .font(Theme.labelBold).lineLimit(1)
                if waiting > 0 { Text("\(waiting) more queued").font(Theme.label).foregroundStyle(.secondary) }
            }
            Spacer()
            Button("Cancel All") { activeJobs.forEach(model.plugins.cancel) }.controlSize(.small)
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 8).fill(Theme.face))
    }

    private var actions: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("ACTIONS").capsLabel()
            VStack(spacing: 0) {
                if let item = transcribeAction, !untranscribed.isEmpty, untranscribed.count < takes.count {
                    ActionRow(title: "Transcribe the \(untranscribed.count) without a transcript", symbol: "text.quote") {
                        model.plugins.run(item.action, of: item.plugin, on: untranscribed)
                    }
                    Divider().padding(.leading, 36)
                }
                ForEach(model.plugins.actions, id: \.action.id) { item in
                    ActionRow(title: item.action.title, detail: "All \(takes.count)",
                              symbol: item.plugin.manifest.produces?.contains("transcript") == true ? "text.quote" : "puzzlepiece.extension") {
                        model.plugins.run(item.action, of: item.plugin, on: takes)
                    }
                    Divider().padding(.leading, 36)
                }
                if transcribed > 0 {
                    ActionRow(title: "Copy transcripts", detail: "\(transcribed) as text", symbol: "doc.on.doc", action: onCopyTranscripts)
                    Divider().padding(.leading, 36)
                }
                ActionRow(title: "Show in Finder", detail: "\(takes.reduce(0) { $0 + $1.audioFiles.count }) audio files", symbol: "folder") {
                    NSWorkspace.shared.activateFileViewerSelecting(takes.flatMap { $0.audioFiles.map { URL(fileURLWithPath: $0.path) } })
                }
                Divider().padding(.leading, 36)
                ActionRow(title: "Move to Trash…", detail: "\(takes.reduce(0) { $0 + $1.descriptor.files.count }) files",
                          symbol: "trash", destructive: true, action: onDelete)
            }
            .background(RoundedRectangle(cornerRadius: 8).fill(Theme.face))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.hairline, lineWidth: 0.5))
        }
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("SELECTED").capsLabel()
            ForEach(takes) { take in
                Button { onSelect(take) } label: {
                    HStack(spacing: 10) {
                        Text(take.descriptor.started?.formatted(.dateTime.month(.abbreviated).day().hour().minute()) ?? take.id)
                            .frame(width: 130, alignment: .leading)
                        Text(RecordingsView.length(take.descriptor.duration ?? 0)).monospacedDigit().foregroundStyle(.secondary)
                            .frame(width: 48, alignment: .leading)
                        Text(take.descriptor.speakers.map(\.name).joined(separator: ", ")).foregroundStyle(.secondary).lineLimit(1)
                        Spacer()
                        if let job = model.plugins.job(for: take.id) {
                            Text(job.state == .queued ? "Queued" : job.message ?? "Working").font(.caption).foregroundStyle(.secondary)
                        } else if take.hasTranscript, let words = transcripts[take.id]?.plainText.split(separator: " ").count {
                            Text("\(words) word\(words == 1 ? "" : "s")").font(.caption).foregroundStyle(.secondary)
                        }
                        Image(systemName: take.hasTranscript ? "text.quote" : "minus").foregroundStyle(.secondary).frame(width: 16)
                    }
                    .font(.callout)
                    .padding(.vertical, 5).padding(.horizontal, 8)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Show only this recording")
            }
        }
    }
}

/// One full-width row in an action list: icon, title, and a quiet detail on the right.
private struct ActionRow: View {
    let title: String
    var detail: String? = nil
    let symbol: String
    var destructive = false
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: symbol).frame(width: 16)
                    .foregroundStyle(destructive ? Theme.recText : Color.secondary)
                Text(title).foregroundStyle(destructive ? Theme.recText : Color.primary)
                Spacer()
                if let detail { Text(detail).font(.caption).foregroundStyle(.secondary) }
            }
            .font(.callout)
            .padding(.vertical, 8).padding(.horizontal, 10)
            .background(hovering ? Color.primary.opacity(0.06) : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
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
