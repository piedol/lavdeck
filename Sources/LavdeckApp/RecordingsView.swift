import AVFoundation
import AppKit
import LavdeckKit
import LavdeckPluginKit
import SwiftUI

/// Recordings manager: browse, filter, sort, search, watch, play, transcribe and delete takes.
struct RecordingsView: View {
    @Environment(AppModel.self) private var model
    @State private var takes: [Take] = []
    @State private var transcripts: [String: TranscriptDocument] = [:]
    /// Parsed transcripts by file path and modification date, so a reload only re-reads what changed.
    @State private var transcriptCache: [String: CachedTranscript] = [:]
    @State private var loaded = false
    /// ⌘-click adds or removes a take, ⇧-click selects a range, ⌘A selects every visible take.
    @State private var selection: Set<Take.ID> = []
    @State private var search = ""
    @State private var filter: Filter = .all
    @State private var sortOrder = [KeyPathComparator(\Row.started, order: .reverse)]
    @State private var pendingDelete: [Take]?
    @StateObject private var player = TakePlayer()
    /// The filtered, sorted list, rebuilt when takes, transcripts, search, filter or sort change (not per render).
    @State private var rows: [Row] = []
    /// This window, to ignore other windows becoming key and to scope the Space key to it.
    @State private var windowNumber: Int?
    @State private var keyMonitor: Any?

    struct CachedTranscript: Sendable {
        let modified: Date
        let doc: TranscriptDocument
    }

    enum Filter: String, CaseIterable, Identifiable {
        case all = "All", video = "With video", transcribed = "Transcribed", untranscribed = "Not transcribed",
             twoSpeaker = "Two speakers"
        var id: String { rawValue }
    }

    struct Row: Identifiable {
        let take: Take
        let snippet: String
        var id: String { take.id }
        var started: Date { take.descriptor.started ?? .distantPast }
        var duration: Double { take.descriptor.duration ?? 0 }
        var speakers: String { take.descriptor.speakers.map(\.name).joined(separator: ", ") }
        var media: Int { MediaBadge.rank(take) }
    }

    private func refreshRows() {
        let query = search.trimmingCharacters(in: .whitespaces).lowercased()
        rows = takes.map { Row(take: $0, snippet: Self.snippet(transcripts[$0.id])) }.filter { row in
            switch filter {
            case .all: break
            case .video: if row.media == 0 { return false }
            case .transcribed: if !row.take.hasTranscript { return false }
            case .untranscribed: if row.take.hasTranscript { return false }
            case .twoSpeaker: if !row.take.descriptor.isTwoSpeaker { return false }
            }
            guard !query.isEmpty else { return true }
            if row.speakers.lowercased().contains(query) || row.take.id.contains(query) { return true }
            if Self.dateText(row.started).lowercased().contains(query) { return true }
            return transcripts[row.id]?.plainText.lowercased().contains(query) == true
        }.sorted(using: sortOrder)
    }

    /// Selected takes the current filter and search show, in table order. Hidden takes stay selected
    /// but are never acted on, so a filter can't hide what Move to Trash will remove.
    private var selectedTakes: [Take] { rows.filter { selection.contains($0.id) }.map(\.take) }

    var body: some View {
        HSplitView {
            table
                .frame(minWidth: 460, idealWidth: 540)
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
                } else if loaded && takes.isEmpty {
                    ContentUnavailableView {
                        Label("No recordings yet", systemImage: "waveform")
                    } description: {
                        Text("Press ⌥⌘R, or Record in the menu bar, with the receiver plugged in. Takes appear here as soon as they stop.")
                    }
                } else {
                    ContentUnavailableView("Select a recording", systemImage: "waveform")
                }
            }
            .frame(minWidth: 460, maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 940, minHeight: 580)
        .searchable(text: $search, placement: .toolbar, prompt: "Search transcripts, names and dates")
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
        .background(WindowReader { windowNumber = $0?.windowNumber })
        .onAppear {
            reload()
            installSpaceKey()
        }
        .onDisappear {
            if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
            keyMonitor = nil
            player.stop()
        }
        .onChange(of: model.libraryRevision) { reload() }
        .onChange(of: model.plugins.outputsRevision) { reload() }
        .onChange(of: model.focusTakeID) { focusRequestedTake() }
        .onChange(of: search) { refreshRows() }
        .onChange(of: filter) { refreshRows() }
        .onChange(of: sortOrder) { refreshRows() }
        // Files can change in Finder while the window is in the background.
        .onReceive(NotificationCenter.default.publisher(for: NSWindow.didBecomeKeyNotification)) { note in
            if let window = note.object as? NSWindow, window.windowNumber == windowNumber { reload() }
        }
        .confirmationDialog(pendingDelete?.count ?? 0 > 1 ? "Move \(pendingDelete?.count ?? 0) recordings to the Trash?" : "Move this recording to the Trash?",
                            isPresented: Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } }),
                            presenting: pendingDelete) { doomed in
            Button("Move \(doomed.reduce(0) { $0 + $1.descriptor.files.count }) files to Trash", role: .destructive) { delete(doomed) }
        } message: { doomed in
            let busy = doomed.contains { model.plugins.job(for: $0.id) != nil }
            Text((doomed.count > 1
                  ? "All files of these \(doomed.count) takes, including transcripts and videos, go to the Trash."
                  : "All files of the \(doomed.first?.descriptor.stem ?? "") take, including transcripts and videos, go to the Trash.")
                 + (busy ? " Work still running on them is cancelled." : "") + " You can restore them from there.")
        }
    }

    private var table: some View {
        Table(rows, selection: $selection, sortOrder: $sortOrder) {
            TableColumn("Recorded", value: \.started) { row in
                VStack(alignment: .leading, spacing: 1) {
                    Text(row.started.formatted(.dateTime.month(.abbreviated).day().hour().minute()))
                    if let job = model.plugins.job(for: row.id) {
                        ProgressView(value: job.state == .queued ? 0 : job.fraction) {
                            Text(job.state == .queued ? "\(job.title): queued" : job.message ?? job.title).font(.caption2)
                        }
                        .controlSize(.mini)
                        .accessibilityLabel(job.state == .queued ? "\(job.title), queued" : "\(job.title), \(Int(job.fraction * 100)) percent")
                    }
                }
            }
            .width(min: 120, ideal: 136)
            TableColumn("Length", value: \.duration) { row in
                Text(Self.length(row.duration)).monospacedDigit()
            }
            .width(min: 48, ideal: 56)
            TableColumn("Video", value: \.media) { row in MediaBadge(take: row.take) }
                .width(min: 40, ideal: 44)
            TableColumn("Mics", value: \.speakers) { row in Text(row.speakers.isEmpty ? "—" : row.speakers).lineLimit(1) }
                .width(min: 50, ideal: 80)
            TableColumn("Transcript", value: \.snippet) { row in
                Text(row.snippet.isEmpty ? "—" : row.snippet)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .help(row.snippet)
            }
        }
        .overlay {
            if loaded && !takes.isEmpty && rows.isEmpty {
                if search.isEmpty {
                    ContentUnavailableView {
                        Label("Nothing matches “\(filter.rawValue)”", systemImage: "line.3.horizontal.decrease.circle")
                    } actions: {
                        Button("Show All") { filter = .all }
                    }
                } else {
                    ContentUnavailableView.search(text: search)
                }
            }
        }
        // Right-clicking inside the selection acts on all of it; right-clicking elsewhere acts on that row.
        .contextMenu(forSelectionType: Take.ID.self) { ids in
            let chosen = rows.filter { ids.contains($0.id) }.map(\.take)
            if !chosen.isEmpty {
                if chosen.count == 1, let take = chosen.first, let url = Self.shareURL(take) {
                    Button("Open \(url.pathExtension == "mp4" ? "Video" : "Audio")") { NSWorkspace.shared.open(url) }
                }
                Button("Show in Finder") { reveal(chosen) }
                if chosen.contains(where: \.hasTranscript) {
                    Button("Copy Transcript\(chosen.count > 1 ? "s" : "")") { copyTranscripts(chosen) }
                }
                let actions = model.plugins.actions.compactMap { item -> ApplicableAction? in
                    let fit = chosen.filter { item.action.applies(to: $0.descriptor) }
                    return fit.isEmpty ? nil : ApplicableAction(plugin: item.plugin, action: item.action, takes: fit)
                }
                if !actions.isEmpty { Divider() }
                ForEach(actions) { entry in
                    let busy = entry.takes.allSatisfy { model.plugins.job(for: $0.id, plugin: entry.plugin.id) != nil }
                    Button(entry.action.title + (busy ? " (in progress)" : chosen.count > 1 ? " (\(entry.takes.count))" : "")) {
                        model.plugins.run(entry.action, of: entry.plugin, on: entry.takes)
                    }
                    .disabled(busy)
                }
                Divider()
                Button("Move to Trash…", role: .destructive) { pendingDelete = chosen }
            }
        } primaryAction: { ids in
            // Double-click or Return plays a single take.
            if ids.count == 1, let take = rows.first(where: { ids.contains($0.id) })?.take,
               let media = TakeMedia.all(in: take).first {
                player.load(media, of: take, autoplay: true)
            }
        }
        .onDeleteCommand { if !selectedTakes.isEmpty { pendingDelete = selectedTakes } }
        .onExitCommand { if selection.count > 1, let first = selectedTakes.first { selection = [first.id] } }
    }

    private func reload() {
        let root = model.recordingsRoot, names = model.micNames, cache = transcriptCache
        Task {
            let (scanned, fresh) = await Task.detached(priority: .userInitiated) { () -> ([Take], [String: CachedTranscript]) in
                let scanned = TakeLibrary.scan(root: root, micNames: names)
                var fresh: [String: CachedTranscript] = [:]
                for take in scanned where take.hasTranscript {
                    let path = take.transcriptURL.path
                    let modified = (try? FileManager.default.attributesOfItem(atPath: path)[.modificationDate] as? Date) ?? .distantPast
                    if let hit = cache[path], hit.modified == modified { fresh[path] = hit; continue }
                    if let doc = try? TranscriptDocument.load(take.transcriptURL) { fresh[path] = CachedTranscript(modified: modified, doc: doc) }
                }
                return (scanned, fresh)
            }.value
            takes = scanned
            transcriptCache = fresh
            var docs: [String: TranscriptDocument] = [:]
            for take in scanned { if let hit = fresh[take.transcriptURL.path] { docs[take.id] = hit.doc } }
            transcripts = docs
            refreshRows()
            selection = selection.filter { id in scanned.contains { $0.id == id } }
            // Start on the newest take; later reloads leave an empty selection alone.
            if !loaded, selection.isEmpty, let first = rows.first { selection = [first.id] }
            loaded = true
            focusRequestedTake()
        }
    }

    /// Space plays or pauses the selected take anywhere in this window, except while typing in a text field.
    private func installSpaceKey() {
        guard keyMonitor == nil else { return }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            guard event.keyCode == 49, event.modifierFlags.intersection([.command, .option, .control, .shift]).isEmpty,
                  let window = event.window, window.windowNumber == windowNumber,
                  !(window.firstResponder is NSText) else { return event }
            if player.media != nil {
                player.toggle()
            } else if selectedTakes.count == 1, let take = selectedTakes.first, let best = TakeMedia.all(in: take).first {
                player.load(best, of: take, autoplay: true)
            }
            return nil
        }
    }

    private func focusRequestedTake() {
        guard let id = model.focusTakeID, takes.contains(where: { $0.id == id }) else { return }
        filter = .all
        search = ""
        refreshRows()
        selection = [id]
        model.focusTakeID = nil
    }

    private func delete(_ doomed: [Take]) {
        let ids = Set(doomed.map(\.id))
        // A job still writing into the take's folder would leave an orphan file behind.
        model.plugins.cancelJobs(for: ids)
        player.stop()
        // Afterwards select the row that took the first removed take's place, so the list keeps its spot.
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
        NSWorkspace.shared.activateFileViewerSelecting(chosen.compactMap { Self.shareURL($0) })
    }

    /// Puts the takes' transcripts on the clipboard as text, oldest first, each under its date.
    private func copyTranscripts(_ chosen: [Take]) {
        let ordered = chosen.sorted { ($0.descriptor.started ?? .distantPast) < ($1.descriptor.started ?? .distantPast) }
        let parts = ordered.compactMap { take -> String? in
            guard let doc = transcripts[take.id] else { return nil }
            let text = Self.transcriptText(doc)
            return ordered.count > 1 ? "\(Self.dateText(take.descriptor.started ?? .distantPast))\n\n\(text)" : text
        }
        guard !parts.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(parts.joined(separator: "\n\n"), forType: .string)
    }

    static func transcriptText(_ doc: TranscriptDocument) -> String {
        let multi = doc.speakers.count > 1
        return doc.segments.map { segment in
            "[\(TranscriptDocument.timestamp(segment.start))] \(multi ? doc.speakerName(segment.speaker) + ": " : "")\(segment.text)"
        }.joined(separator: "\n")
    }

    /// The file to share, open or reveal for a take: its finished video, else its main audio.
    static func shareURL(_ take: Take) -> URL? {
        (take.descriptor.files.first { $0.source == "pip" } ?? take.descriptor.mainAudio).map { URL(fileURLWithPath: $0.path) }
    }

    static func snippet(_ doc: TranscriptDocument?) -> String {
        guard let doc else { return "" }
        return doc.segments.prefix(3).map(\.text).joined(separator: " ").trimmingCharacters(in: .whitespaces)
    }

    static func dateText(_ date: Date) -> String {
        date.formatted(date: .complete, time: .shortened)
    }

    static func length(_ seconds: Double) -> String {
        let s = Int(seconds.rounded())
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }
}

/// Reports the NSWindow a SwiftUI view lives in.
private struct WindowReader: NSViewRepresentable {
    let onWindow: (NSWindow?) -> Void

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { onWindow(view.window) }
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {}
}

/// A plugin action and the takes in a selection it applies to.
private struct ApplicableAction: Identifiable {
    let plugin: InstalledPlugin
    let action: PluginAction
    let takes: [Take]
    var id: String { "\(plugin.id)/\(action.id)" }
}

/// One glyph saying what video a take has: the finished video, or only raw screen/camera recordings.
private struct MediaBadge: View {
    let take: Take

    /// 3: finished video, 2: screen and camera, 1: one raw capture, 0: audio only.
    static func rank(_ take: Take) -> Int {
        let files = take.descriptor.files
        if files.contains(where: { $0.source == "pip" }) { return 3 }
        return files.filter { $0.role == .capture && $0.kind == "video" }.count >= 2 ? 2 : take.descriptor.has("video") ? 1 : 0
    }

    var body: some View {
        let files = take.descriptor.files
        let screen = files.contains { $0.role == .capture && $0.source == "screen" }
        let camera = files.contains { $0.role == .capture && $0.source == "camera" }
        let finished = files.contains { $0.source == "pip" }
        Group {
            if finished {
                Image(systemName: "play.rectangle.fill").foregroundStyle(Color.primary)
            } else if screen || camera {
                Image(systemName: screen ? "display" : "video").foregroundStyle(.secondary)
            } else {
                Text("")
            }
        }
        .help(finished ? "Video ready" : screen && camera ? "Screen and camera recorded; video not made" : screen ? "Screen recorded" : camera ? "Camera recorded" : "Audio only")
        .accessibilityLabel(finished ? "Video ready" : screen || camera ? "Raw video only" : "Audio only")
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
    /// The transcript scrolls with playback until the user scrolls it themselves.
    @State private var following = true

    private var media: [TakeMedia] { TakeMedia.all(in: take) }
    private var videoPlugin: InstalledPlugin? { model.plugins.plugin(producing: "video") }
    private var videoJob: PluginHost.Job? { videoPlugin.flatMap { model.plugins.job(for: take.id, plugin: $0.id) } }
    private var remakeAction: (plugin: InstalledPlugin, action: PluginAction)? {
        model.plugins.actions(for: take).first { $0.plugin.id == videoPlugin?.id }
    }
    private var transcribeAction: (plugin: InstalledPlugin, action: PluginAction)? {
        model.plugins.actions(for: take).first { $0.plugin.manifest.produces?.contains("transcript") == true }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(16)
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 18) {
                        mediaArea
                        files
                        transcriptSection
                    }
                    .padding(16)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onScrollPhaseChange { _, phase in if phase == .interacting { following = false } }
                .onChange(of: currentSegment?.id) { _, id in
                    // Follow playback through long transcripts, unless the user is reading elsewhere.
                    if following, player.isPlaying, let id { withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(id, anchor: .center) } }
                }
                .overlay(alignment: .bottomTrailing) {
                    if !following, player.isPlaying, transcript != nil, let id = currentSegment?.id {
                        Button {
                            following = true
                            withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(id, anchor: .center) }
                        } label: {
                            Label("Follow Along", systemImage: "text.line.first.and.arrowtriangle.forward")
                        }
                        .controlSize(.small)
                        .padding(12)
                        .help("Scroll the transcript with playback again")
                    }
                }
            }
        }
        .onAppear(perform: loadDefault)
        .onChange(of: take.id) { following = true; loadDefault() }
        .onChange(of: media.map(\.id)) { loadDefault() }
    }

    /// The finished video if there is one, else a raw recording, else the audio. Keeps a source the user
    /// picked from the files list unless a finished video has just appeared.
    private func loadDefault() {
        guard let best = media.first else { player.stop(); return }
        let current = player.media.flatMap { m in media.first { $0 == m } }
        if current == nil || (best.kind == .video && current?.kind != .video && current?.isVideo == true) {
            player.load(best, of: take)
        }
    }

    private var currentSegment: TranscriptDocument.Segment? {
        // Only once playback has started, so a freshly loaded take doesn't look like it's playing line one.
        guard player.media != nil, player.isPlaying || player.currentTime > 0 else { return nil }
        return transcript?.segments.last { $0.start <= player.currentTime && player.currentTime < $0.end }
    }

    // MARK: Header

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

    private var mode: String {
        if take.descriptor.isTwoSpeaker { return "Two speakers" }
        if take.descriptor.files.contains(where: { $0.role == .stereo }) { return "Stereo" }
        if take.descriptor.files.contains(where: { $0.role == .mix }) { return "Mono" }
        return "Single mic"
    }

    private var info: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(RecordingsView.dateText(take.descriptor.started ?? .distantPast))
                .font(.title3.weight(.semibold))
            HStack(spacing: 6) {
                Text("\(RecordingsView.length(take.descriptor.duration ?? 0)) · \(mode)").monospacedDigit()
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
                if player.media == nil, let best = media.first { player.load(best, of: take, autoplay: true) } else { player.toggle() }
            } label: {
                Label(player.isPlaying ? "Pause" : "Play", systemImage: player.isPlaying ? "pause.fill" : "play.fill")
            }
            .help("Play or pause (Space)")
            if let url = RecordingsView.shareURL(take) {
                ShareLink(item: url) { Label("Share", systemImage: "square.and.arrow.up") }
                    .disabled(videoJob != nil)
                    .help(videoJob != nil ? "Wait for the video to finish" : "Share the \(url.pathExtension == "mp4" ? "video" : "audio")")
            }
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
                if let transcript {
                    Button("Copy Transcript") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(RecordingsView.transcriptText(transcript), forType: .string)
                    }
                }
                let actions = model.plugins.actions(for: take)
                if !actions.isEmpty { Divider() }
                ForEach(actions, id: \.action.id) { item in
                    let busy = model.plugins.job(for: take.id, plugin: item.plugin.id) != nil
                    Button(item.action.title + (busy ? " (in progress)" : "")) { model.plugins.run(item.action, of: item.plugin, on: take) }
                        .disabled(busy)
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

    // MARK: Media

    @ViewBuilder private var mediaArea: some View {
        VStack(alignment: .leading, spacing: 8) {
            if player.media?.isVideo == true {
                VideoSurface(player: player.player)
                    .aspectRatio(16 / 9, contentMode: .fit)
                    .frame(maxWidth: .infinity, maxHeight: 360)
                    .background(Color.black)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
            } else if player.media != nil {
                AudioTransport(player: player)
            }
            if let error = player.error {
                Label("Can’t play \((player.media?.file.path as NSString?)?.lastPathComponent ?? "this file"): \(error)", systemImage: "exclamationmark.triangle.fill")
                    .font(.callout).foregroundStyle(Theme.warnText)
            }
            videoStatus
        }
    }

    /// What the video shows, and whether it's being made, failed or missing.
    @ViewBuilder private var videoStatus: some View {
        let finished = take.descriptor.files.contains { $0.source == "pip" }
        let raw = take.descriptor.has("video")
        if let job = videoJob {
            HStack(spacing: 8) {
                ProgressView(value: job.state == .queued ? 0 : job.fraction).frame(width: 140)
                Text(job.state == .queued ? "Video queued" : "Making the video… \(Int(job.fraction * 100))%")
                    .font(.callout).foregroundStyle(.secondary)
                Spacer()
                Button("Cancel") { model.plugins.cancel(job) }.controlSize(.small)
            }
        } else if let failed = videoPlugin.flatMap({ model.plugins.failure(for: take.id, plugin: $0.id) }) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Theme.warnText)
                Text("The video couldn’t be made: \(failed.failure ?? "")").font(.callout).foregroundStyle(Theme.warnText)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer()
                if let remake = remakeAction {
                    Button("Try Again") { model.plugins.run(remake.action, of: remake.plugin, on: take) }.controlSize(.small)
                }
                Button { model.plugins.dismiss(failed) } label: { Image(systemName: "xmark") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Dismiss")
            }
        } else if raw && !finished {
            HStack(spacing: 8) {
                Text("This take has no finished video yet, so this is the raw recording with the mic audio.")
                    .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                Spacer()
                if let remake = remakeAction {
                    Button("Make Video") { model.plugins.run(remake.action, of: remake.plugin, on: take) }.controlSize(.small)
                }
            }
        } else if player.media?.kind == .video {
            Text(Self.videoDescription(take)).font(.callout).foregroundStyle(.secondary)
        } else if player.media?.kind == .screen || player.media?.kind == .camera {
            Text("The raw \(player.media?.kind == .camera ? "camera" : "screen") recording, with the mic audio lined up.")
                .font(.callout).foregroundStyle(.secondary)
        }
    }

    static func videoDescription(_ take: Take) -> String {
        let files = take.descriptor.files
        let screen = files.contains { $0.role == .capture && $0.source == "screen" }
        let camera = files.contains { $0.role == .capture && $0.source == "camera" }
        switch (screen, camera) {
        case (true, true): return "The finished video: your screen with the camera in a corner, and the mic audio."
        case (true, false): return "The finished video: your screen with the mic audio."
        case (false, true): return "The finished video: the camera with the mic audio."
        default: return "The finished video, with the mic audio."
        }
    }

    // MARK: Files

    private var files: some View {
        let all = take.descriptor.files.filter { $0.role != .metadata }
        let transcriptFiles = all.filter { $0.kind == "transcript" }
        let others = all.filter { $0.kind != "transcript" }
        return VStack(alignment: .leading, spacing: 2) {
            Text("FILES").capsLabel().padding(.bottom, 4)
            ForEach(others, id: \.path) { file in
                let playable = media.first { $0.file.path == file.path }
                FileRow(name: (file.path as NSString).lastPathComponent, label: Self.label(for: file),
                        symbol: file.kind == "video" ? "film" : "waveform", urls: [URL(fileURLWithPath: file.path)],
                        loaded: playable != nil && player.media == playable, playing: player.isPlaying,
                        onClick: playable.map { m in { player.load(m, of: take, autoplay: true) } })
            }
            if !transcriptFiles.isEmpty {
                FileRow(name: "\(take.descriptor.stem).transcript",
                        label: "Transcript · " + transcriptFiles.map { ($0.path as NSString).pathExtension.uppercased() }.joined(separator: ", "),
                        symbol: "doc.text", urls: transcriptFiles.map { URL(fileURLWithPath: $0.path) }, onClick: nil)
            }
        }
    }

    static func label(for file: TakeFile) -> String {
        switch file.role {
        case .stereo: "Stereo audio (TX1 left, TX2 right)"
        case .mix: "Mono audio (both mics)"
        case .track: "TX\(file.mic ?? 0) audio"
        case .derived:
            file.source == "pip" ? "Finished video"
                : file.kind == "transcript" ? "Transcript" : (file.kind ?? "Plugin output")
        case .metadata: "Metadata"
        case .capture:
            switch file.source {
            case "screen": "Screen recording (raw)"
            case "camera": "Camera recording (raw)"
            case "system-audio": "System audio"
            default: "Captured \(file.kind ?? "media")"
            }
        }
    }

    // MARK: Transcript

    @ViewBuilder private var transcriptSection: some View {
        let job = transcribeAction.flatMap { model.plugins.job(for: take.id, plugin: $0.plugin.id) }
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("TRANSCRIPT").capsLabel()
                Spacer()
                if let job {
                    ProgressView(value: job.state == .queued ? 0 : job.fraction).frame(width: 120).controlSize(.small)
                    Text(job.state == .queued ? "Queued" : job.message ?? job.title).font(.caption).foregroundStyle(.secondary)
                    Button("Cancel") { model.plugins.cancel(job) }.controlSize(.small)
                } else if transcript != nil, let item = transcribeAction {
                    Button("Regenerate") { model.plugins.run(item.action, of: item.plugin, on: take) }
                        .controlSize(.small)
                }
            }
            if let failed = transcribeAction.flatMap({ model.plugins.failure(for: take.id, plugin: $0.plugin.id) }) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("\(failed.pluginName) failed: \(failed.failure ?? "")").font(.callout).foregroundStyle(Theme.warnText)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer()
                    Button { model.plugins.dismiss(failed) } label: { Image(systemName: "xmark") }
                        .buttonStyle(.plain).foregroundStyle(.secondary).help("Dismiss")
                }
            }
            if let transcript {
                TranscriptList(transcript: transcript, search: search, current: currentSegment?.id) { segment in
                    following = true
                    if player.media == nil, let best = media.first { player.load(best, of: take) }
                    player.play(from: segment.start)
                }
            } else if job == nil {
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
        }
    }
}

/// A file in a take: click plays it when it's media; right-click opens, reveals or copies; drag it out to
/// another app. A missing file (moved or deleted in Finder) says so.
private struct FileRow: View {
    let name: String
    let label: String
    let symbol: String
    let urls: [URL]
    /// This file is in the player (highlighted); `playing` adds the speaker glyph while it plays.
    var loaded = false
    var playing = false
    let onClick: (() -> Void)?
    @State private var hovering = false

    var body: some View {
        let missing = !urls.contains { FileManager.default.fileExists(atPath: $0.path) }
        HStack {
            Image(systemName: loaded && playing ? "speaker.wave.2.fill" : symbol)
                .foregroundStyle(loaded ? Color.accentColor : .secondary).frame(width: 16)
            Text(name).lineLimit(1).truncationMode(.middle)
                .foregroundStyle(missing ? .secondary : .primary)
                .strikethrough(missing)
            Spacer()
            Text(missing ? "Missing" : label).font(.caption).foregroundStyle(missing ? Theme.warnText : .secondary)
        }
        .font(.callout)
        .padding(.vertical, 4).padding(.horizontal, 6)
        .background(RoundedRectangle(cornerRadius: 6).fill(loaded ? Color.accentColor.opacity(0.12) : hovering && onClick != nil ? Color.primary.opacity(0.05) : .clear))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture { if !missing { onClick?() } }
        .onDrag { NSItemProvider(contentsOf: urls[0]) ?? NSItemProvider() }
        .contextMenu {
            if urls.count == 1 {
                Button("Open") { NSWorkspace.shared.open(urls[0]) }
                Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting(urls) }
                Button("Copy Path") { copy(urls[0].path) }
            } else {
                ForEach(urls, id: \.self) { url in
                    Button("Open .\(url.pathExtension)") { NSWorkspace.shared.open(url) }
                }
                Divider()
                Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting(urls) }
            }
        }
        .help(missing ? "This file was moved or deleted outside Lavdeck" : onClick != nil ? "Click to play. Drag to another app, or right-click for more." : "Drag to another app, or right-click for more.")
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(onClick != nil ? .isButton : [])
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

/// Play/pause, a scrubber and elapsed / total time, for takes without video.
private struct AudioTransport: View {
    @ObservedObject var player: TakePlayer
    @State private var scrubbing: Double?

    var body: some View {
        let total = max(player.duration, 0.1)
        HStack(spacing: 10) {
            Button { player.toggle() } label: {
                Image(systemName: player.isPlaying ? "pause.fill" : "play.fill").frame(width: 16)
            }
            .buttonStyle(.borderless)
            .accessibilityLabel(player.isPlaying ? "Pause" : "Play")
            Text(RecordingsView.length(scrubbing ?? player.currentTime)).font(Theme.mono).foregroundStyle(.secondary)
            Slider(value: Binding(get: { scrubbing ?? min(player.currentTime, total) }, set: { scrubbing = $0 }), in: 0...total) { editing in
                if !editing, let to = scrubbing { player.seek(to: to); scrubbing = nil }
            }
            .controlSize(.small)
            .accessibilityLabel("Position")
            Text(RecordingsView.length(total)).font(Theme.mono).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 10).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 8).fill(Theme.face))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.hairline, lineWidth: 0.5))
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
        let videos = takes.filter { MediaBadge.rank($0) > 0 }.count
        return VStack(alignment: .leading, spacing: 4) {
            Text("\(takes.count) recordings").font(.title3.weight(.semibold))
            Text([span, "\(RecordingsView.length(total)) total", "\(transcribed) of \(takes.count) transcribed",
                  videos > 0 ? "\(videos) with video" : ""]
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
        let pluginActions = model.plugins.actions.compactMap { item -> ApplicableAction? in
            let fit = takes.filter { item.action.applies(to: $0.descriptor) }
            return fit.isEmpty ? nil : ApplicableAction(plugin: item.plugin, action: item.action, takes: fit)
        }
        return VStack(alignment: .leading, spacing: 6) {
            Text("ACTIONS").capsLabel()
            VStack(spacing: 0) {
                if let item = transcribeAction, !untranscribed.isEmpty, untranscribed.count < takes.count {
                    ActionRow(title: "Transcribe the \(untranscribed.count) without a transcript", symbol: "text.quote") {
                        model.plugins.run(item.action, of: item.plugin, on: untranscribed)
                    }
                    Divider().padding(.leading, 36)
                }
                ForEach(pluginActions) { entry in
                    ActionRow(title: entry.action.title,
                              detail: entry.takes.count == takes.count ? "All \(takes.count)" : "\(entry.takes.count) of \(takes.count)",
                              symbol: entry.plugin.manifest.produces?.contains("transcript") == true ? "text.quote"
                                  : entry.plugin.manifest.produces?.contains("video") == true ? "film" : "puzzlepiece.extension") {
                        model.plugins.run(entry.action, of: entry.plugin, on: entry.takes)
                    }
                    Divider().padding(.leading, 36)
                }
                if transcribed > 0 {
                    ActionRow(title: "Copy transcripts", detail: "\(transcribed) as text", symbol: "doc.on.doc", action: onCopyTranscripts)
                    Divider().padding(.leading, 36)
                }
                ActionRow(title: "Show in Finder", detail: "\(takes.count) takes", symbol: "folder") {
                    NSWorkspace.shared.activateFileViewerSelecting(takes.compactMap { RecordingsView.shareURL($0) })
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
                let snippet = RecordingsView.snippet(transcripts[take.id])
                Button { onSelect(take) } label: {
                    HStack(spacing: 10) {
                        Text(take.descriptor.started?.formatted(.dateTime.month(.abbreviated).day().hour().minute()) ?? take.id)
                            .frame(width: 130, alignment: .leading)
                        Text(RecordingsView.length(take.descriptor.duration ?? 0)).monospacedDigit().foregroundStyle(.secondary)
                            .frame(width: 48, alignment: .leading)
                        MediaBadge(take: take).frame(width: 18)
                        Text(snippet.isEmpty ? take.descriptor.speakers.map(\.name).joined(separator: ", ") : snippet)
                            .foregroundStyle(.secondary).lineLimit(1)
                        Spacer()
                        if let job = model.plugins.job(for: take.id) {
                            Text(job.state == .queued ? "Queued" : job.message ?? "Working").font(.caption).foregroundStyle(.secondary)
                        }
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

/// Transcript lines; click one to play from there. Lazily built, so hour-long takes stay quick.
private struct TranscriptList: View {
    let transcript: TranscriptDocument
    let search: String
    let current: TranscriptDocument.Segment.ID?
    let onPlay: (TranscriptDocument.Segment) -> Void

    var body: some View {
        let multi = transcript.speakers.count > 1
        let query = search.trimmingCharacters(in: .whitespaces)
        let long = (transcript.segments.last?.end ?? 0) >= 3600
        LazyVStack(alignment: .leading, spacing: 2) {
            ForEach(transcript.segments) { segment in
                let hit = !query.isEmpty && segment.text.localizedCaseInsensitiveContains(query)
                let stamp = long ? TranscriptDocument.timestamp(segment.start) : RecordingsView.length(segment.start.rounded(.down))
                Button { onPlay(segment) } label: {
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Text(stamp).font(Theme.mono).frame(minWidth: 36, alignment: .trailing).foregroundStyle(.secondary)
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
                        current == segment.id ? Theme.green.opacity(0.12) : .clear))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .id(segment.id)
                .help("Play from \(TranscriptDocument.timestamp(segment.start))")
                .accessibilityLabel("\(stamp)\(multi ? ", \(transcript.speakerName(segment.speaker))" : ""), \(segment.text)")
                .accessibilityHint("Plays from here")
            }
            Text("\(transcript.model)\(transcript.language.map { " · \($0)" } ?? "")")
                .font(.caption).foregroundStyle(.tertiary).padding(.top, 6)
        }
        .textSelection(.enabled)
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
