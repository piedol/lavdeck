import AVFoundation
import AVKit
import LavdeckKit
import LavdeckPluginKit
import SwiftUI

/// Something in a take that can be played: its audio, its finished video, or a raw capture.
struct TakeMedia: Identifiable, Hashable {
    enum Kind: Hashable { case video, screen, camera, audio, track(Int) }
    let kind: Kind
    let file: TakeFile
    var id: String { file.path }
    var isVideo: Bool { file.kind == "video" }
    static func == (a: Self, b: Self) -> Bool { a.id == b.id }
    func hash(into hasher: inout Hasher) { hasher.combine(id) }

    var title: String {
        switch kind {
        case .video: "Video"
        case .screen: "Screen"
        case .camera: "Camera"
        case .audio: "Audio"
        case .track(let mic): "TX\(mic)"
        }
    }

    /// What a take offers, best first: the finished video, raw captures, the main audio, then each mic's track.
    static func all(in take: Take) -> [TakeMedia] {
        let files = take.descriptor.files
        var out: [TakeMedia] = []
        if let pip = files.first(where: { $0.source == "pip" }) { out.append(.init(kind: .video, file: pip)) }
        if let screen = files.first(where: { $0.role == .capture && $0.source == "screen" && $0.kind == "video" }) {
            out.append(.init(kind: .screen, file: screen))
        }
        if let camera = files.first(where: { $0.role == .capture && $0.source == "camera" && $0.kind == "video" }) {
            out.append(.init(kind: .camera, file: camera))
        }
        if let main = take.descriptor.mainAudio { out.append(.init(kind: .audio, file: main)) }
        if take.descriptor.isTwoSpeaker {
            for speaker in take.descriptor.speakers {
                if let path = speaker.file, let file = files.first(where: { $0.path == path }), file.path != take.descriptor.mainAudio?.path {
                    out.append(.init(kind: .track(speaker.mic), file: file))
                }
            }
        }
        return out
    }
}

/// Plays one take's media with AVPlayer. Raw screen and camera recordings have no mic audio, so they play as a
/// composition with the take's audio placed at the offset Lavdeck recorded, staying in sync.
@MainActor
final class TakePlayer: ObservableObject {
    let player = AVPlayer()
    @Published private(set) var isPlaying = false
    @Published private(set) var currentTime: Double = 0
    @Published private(set) var duration: Double = 0
    @Published private(set) var media: TakeMedia?
    /// Why the loaded media can't play (moved, deleted or unreadable).
    @Published private(set) var error: String?
    private var takeID: String?
    private var loading: Task<Void, Never>?
    private var observer: Any?
    private var rateObservation: NSKeyValueObservation?

    init() {
        observer = player.addPeriodicTimeObserver(forInterval: CMTime(value: 1, timescale: 10), queue: .main) { [weak self] time in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.currentTime = time.seconds.isFinite ? time.seconds : 0
                if let d = self.player.currentItem?.duration.seconds, d.isFinite { self.duration = d }
            }
        }
        rateObservation = player.observe(\.timeControlStatus) { [weak self] player, _ in
            let playing = player.timeControlStatus != .paused
            Task { @MainActor in self?.isPlaying = playing }
        }
    }

    /// Loads `media` from `take`, keeping the playhead when switching between a take's sources.
    func load(_ media: TakeMedia, of take: Take, autoplay: Bool = false) {
        guard media != self.media || take.id != takeID else { if autoplay { player.play() }; return }
        let sameTake = take.id == takeID
        let keep = sameTake ? currentTime : 0
        let start = autoplay || (sameTake && isPlaying)
        // Stop the previous take now rather than when the new one has loaded.
        if !sameTake { player.pause() }
        loading?.cancel()
        self.media = media
        takeID = take.id
        error = nil
        duration = take.descriptor.duration ?? 0
        currentTime = keep
        loading = Task {
            let asset = await Self.asset(for: media, of: take)
            guard !Task.isCancelled else { return }
            let item = AVPlayerItem(asset: asset)
            player.replaceCurrentItem(with: item)
            // A paused player shows black until a frame is decoded for it: wait until the item is ready, then
            // seek once so the first (or current) frame appears without pressing Play.
            for await status in item.publisher(for: \.status).values {
                if Task.isCancelled { return }
                if status == .failed {
                    error = item.error?.localizedDescription ?? "This file can’t be played."
                    return
                }
                if status == .readyToPlay { break }
            }
            guard !Task.isCancelled else { return }
            await player.seek(to: CMTime(seconds: keep, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
            if start { player.play() } else { _ = await player.preroll(atRate: 1) }
        }
    }

    func toggle() { isPlaying ? player.pause() : player.play() }

    func play(from seconds: Double) {
        player.seek(to: CMTime(seconds: seconds, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
        currentTime = seconds
        player.play()
    }

    func seek(to seconds: Double) {
        player.seek(to: CMTime(seconds: seconds, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
        currentTime = seconds
    }

    func stop() {
        loading?.cancel()
        player.pause()
        player.replaceCurrentItem(with: nil)
        media = nil
        takeID = nil
        error = nil
        currentTime = 0
    }

    func isInside(_ segment: TranscriptDocument.Segment) -> Bool {
        currentTime >= segment.start && currentTime < segment.end
    }

    private static func asset(for media: TakeMedia, of take: Take) async -> AVAsset {
        let url = URL(fileURLWithPath: media.file.path)
        guard media.kind == .screen || media.kind == .camera, let audio = take.descriptor.mainAudio else {
            return AVURLAsset(url: url)
        }
        // Raw capture + mic audio on the take's timeline (time zero = the first audio sample).
        let composition = AVMutableComposition()
        let audioAsset = AVURLAsset(url: URL(fileURLWithPath: audio.path))
        let videoAsset = AVURLAsset(url: url)
        guard let audioTrack = try? await audioAsset.loadTracks(withMediaType: .audio).first,
              let videoTrack = try? await videoAsset.loadTracks(withMediaType: .video).first,
              let audioLength = try? await audioAsset.load(.duration),
              let videoLength = try? await videoAsset.load(.duration) else { return AVURLAsset(url: url) }
        if let target = composition.addMutableTrack(withMediaType: .audio, preferredTrackID: kCMPersistentTrackID_Invalid) {
            try? target.insertTimeRange(CMTimeRange(start: .zero, duration: audioLength), of: audioTrack, at: .zero)
        }
        let offset = media.file.offset ?? 0
        let sourceStart = CMTime(seconds: max(0, -offset), preferredTimescale: 600)
        let at = CMTime(seconds: max(0, offset), preferredTimescale: 600)
        let length = CMTimeMinimum(videoLength - sourceStart, audioLength - at)
        if length > .zero, let target = composition.addMutableTrack(withMediaType: .video, preferredTrackID: kCMPersistentTrackID_Invalid) {
            try? target.insertTimeRange(CMTimeRange(start: sourceStart, duration: length), of: videoTrack, at: at)
            target.preferredTransform = (try? await videoTrack.load(.preferredTransform)) ?? .identity
        }
        return composition
    }
}

/// AVKit's player view with inline controls.
struct VideoSurface: NSViewRepresentable {
    let player: AVPlayer

    func makeNSView(context: Context) -> AVPlayerView {
        let view = AVPlayerView()
        view.player = player
        view.controlsStyle = .inline
        view.showsFullScreenToggleButton = true
        view.allowsPictureInPicturePlayback = true
        view.videoGravity = .resizeAspect
        return view
    }

    func updateNSView(_ view: AVPlayerView, context: Context) {
        if view.player !== player { view.player = player }
    }
}
