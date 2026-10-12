import AVFoundation
import CoreImage
import Foundation
import LavdeckPluginKit

/// Makes a take's finished video, `<stem> PiP.mp4`: the screen full frame with the camera in a corner (or the
/// camera alone), and the mic audio, lined up by the offsets Lavdeck recorded. System audio recorded with the
/// screen is kept.
public enum VideoComposer {
    /// The file the finished video is written to.
    public static func outputURL(for take: TakeDescriptor) -> URL {
        URL(fileURLWithPath: take.folder).appendingPathComponent("\(take.stem) PiP.mp4")
    }

    /// Writes the finished video for `take`; throws when it has no screen or camera recording.
    public static func make(_ take: TakeDescriptor, settings: VideoSettings,
                            progress: @escaping @Sendable (Double) -> Void) async throws -> URL {
        let screen = take.files.first { $0.role == .capture && $0.source == "screen" }
        let camera = take.files.first { $0.role == .capture && $0.source == "camera" }
        guard screen != nil || camera != nil else { throw CaptureError.nothingRecorded("screen or camera") }
        let tracks = take.files.filter { $0.role == .track }
        let audio = tracks.count >= 2 ? tracks : (take.mainAudio.map { [$0] } ?? [])
        let out = outputURL(for: take)
        try await PiPExporter.export(screen: screen, camera: camera, audio: audio, to: out, settings: settings, progress: progress)
        return out
    }
}

enum PiPExporter {
    static func export(screen: TakeFile?, camera: TakeFile?, audio: [TakeFile], to out: URL, settings: VideoSettings,
                       progress: @escaping @Sendable (Double) -> Void) async throws {
        let composition = AVMutableComposition()
        var total = CMTime.zero

        // The take's timeline starts at the first audio sample.
        for file in audio {
            let asset = AVURLAsset(url: URL(fileURLWithPath: file.path))
            guard let track = try await asset.loadTracks(withMediaType: .audio).first else { continue }
            let duration = try await asset.load(.duration)
            let target = composition.addMutableTrack(withMediaType: .audio, preferredTrackID: kCMPersistentTrackID_Invalid)
            try target?.insertTimeRange(CMTimeRange(start: .zero, duration: duration), of: track, at: .zero)
            total = CMTimeMaximum(total, duration)
        }

        func addVideo(_ file: TakeFile?) async throws -> (id: CMPersistentTrackID, size: CGSize)? {
            guard let file else { return nil }
            let asset = AVURLAsset(url: URL(fileURLWithPath: file.path))
            guard let track = try await asset.loadTracks(withMediaType: .video).first else { return nil }
            let duration = try await asset.load(.duration)
            let size = try await track.load(.naturalSize)
            let offset = file.offset ?? 0
            let sourceStart = CMTime(seconds: max(0, -offset), preferredTimescale: 600)
            let at = CMTime(seconds: max(0, offset), preferredTimescale: 600)
            var length = duration - sourceStart
            if total > .zero { length = CMTimeMinimum(length, total - at) }
            guard length > .zero else { return nil }
            let target = composition.addMutableTrack(withMediaType: .video, preferredTrackID: kCMPersistentTrackID_Invalid)
            try target?.insertTimeRange(CMTimeRange(start: sourceStart, duration: length), of: track, at: at)
            // System audio, when it was recorded, rides in the screen file: keep it, lined up the same way.
            for sound in try await asset.loadTracks(withMediaType: .audio) {
                let lane = composition.addMutableTrack(withMediaType: .audio, preferredTrackID: kCMPersistentTrackID_Invalid)
                try lane?.insertTimeRange(CMTimeRange(start: sourceStart, duration: length), of: sound, at: at)
            }
            if total == .zero { total = at + length }
            return target.map { ($0.trackID, size) }
        }
        let screenTrack = try await addVideo(screen)
        let cameraTrack = try await addVideo(camera)
        guard screenTrack != nil || cameraTrack != nil else { throw CaptureError.nothingRecorded("video") }

        // Output size: the screen's aspect (or the camera's), at most 1920 wide.
        let base = screenTrack?.size ?? cameraTrack!.size
        let width = min(1920, base.width)
        var renderSize = CGSize(width: width, height: (base.height * width / base.width).rounded())
        renderSize.width -= renderSize.width.truncatingRemainder(dividingBy: 2)
        renderSize.height -= renderSize.height.truncatingRemainder(dividingBy: 2)

        let instruction = PiPInstruction(timeRange: CMTimeRange(start: .zero, duration: total),
                                         screen: screenTrack?.id, camera: cameraTrack?.id,
                                         layout: PiPLayout(settings: settings))
        let videoComposition = AVMutableVideoComposition()
        videoComposition.customVideoCompositorClass = PiPCompositor.self
        videoComposition.renderSize = renderSize
        videoComposition.frameDuration = CMTime(value: 1, timescale: 30)
        videoComposition.instructions = [instruction]

        try? FileManager.default.removeItem(at: out)
        let preset = settings.hevc ? AVAssetExportPresetHEVCHighestQuality : AVAssetExportPresetHighestQuality
        guard let session = AVAssetExportSession(asset: composition, presetName: preset) else {
            throw CaptureError.writer("no export session for \(preset)")
        }
        session.videoComposition = videoComposition
        let watcher = Task {
            for await state in session.states(updateInterval: 0.5) {
                if case .exporting(let p) = state { progress(p.fractionCompleted) }
            }
        }
        defer { watcher.cancel() }
        try await session.export(to: out, as: .mp4)
    }
}

struct PiPLayout: Sendable {
    var corner: String
    var fraction: CGFloat   // bubble size as a fraction of the output height
    var circle: Bool

    init(settings: VideoSettings) {
        corner = settings.bubbleCorner.rawValue
        fraction = [VideoSettings.BubbleSize.small: 0.2, .large: 0.38][settings.bubbleSize] ?? 0.28
        circle = settings.bubbleShape == .circle
    }
}

final class PiPInstruction: NSObject, AVVideoCompositionInstructionProtocol, @unchecked Sendable {
    let timeRange: CMTimeRange
    let enablePostProcessing = false
    let containsTweening = true
    let requiredSourceTrackIDs: [NSValue]?
    let passthroughTrackID = kCMPersistentTrackID_Invalid
    let screen: CMPersistentTrackID?
    let camera: CMPersistentTrackID?
    let layout: PiPLayout

    init(timeRange: CMTimeRange, screen: CMPersistentTrackID?, camera: CMPersistentTrackID?, layout: PiPLayout) {
        self.timeRange = timeRange
        self.screen = screen
        self.camera = camera
        self.layout = layout
        requiredSourceTrackIDs = [screen, camera].compactMap { $0 }.map { NSNumber(value: $0) }
    }
}

/// Draws the screen full-frame with the camera in a bubble (or the camera full-frame when there is no
/// screen recording), with Core Image.
final class PiPCompositor: NSObject, AVVideoCompositing, @unchecked Sendable {
    private let context = CIContext(options: [.cacheIntermediates: false])
    private let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!

    let sourcePixelBufferAttributes: [String: any Sendable]? = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
    let requiredPixelBufferAttributesForRenderContext: [String: any Sendable] = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]

    func renderContextChanged(_ newRenderContext: AVVideoCompositionRenderContext) {}

    func startRequest(_ request: AVAsynchronousVideoCompositionRequest) {
        guard let instruction = request.videoCompositionInstruction as? PiPInstruction,
              let output = request.renderContext.newPixelBuffer() else {
            request.finish(with: NSError(domain: "PiP", code: 1))
            return
        }
        let size = request.renderContext.size
        let frame = CGRect(origin: .zero, size: size)
        var image = CIImage(color: .black).cropped(to: frame)

        let screen = instruction.screen.flatMap { request.sourceFrame(byTrackID: $0) }.map { CIImage(cvPixelBuffer: $0) }
        let camera = instruction.camera.flatMap { request.sourceFrame(byTrackID: $0) }.map { CIImage(cvPixelBuffer: $0) }

        if let screen {
            image = fit(screen, in: frame).composited(over: image)
            if let camera { image = bubble(camera, layout: instruction.layout, in: size).composited(over: image) }
        } else if let camera {
            image = fit(camera, in: frame).composited(over: image)
        }
        context.render(image, to: output, bounds: frame, colorSpace: colorSpace)
        request.finish(withComposedVideoFrame: output)
    }

    /// Scales an image to fit the frame, centred.
    private func fit(_ image: CIImage, in frame: CGRect) -> CIImage {
        let scale = min(frame.width / image.extent.width, frame.height / image.extent.height)
        let scaled = image.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        let dx = (frame.width - scaled.extent.width) / 2 - scaled.extent.minX
        let dy = (frame.height - scaled.extent.height) / 2 - scaled.extent.minY
        return scaled.transformed(by: CGAffineTransform(translationX: dx, y: dy))
    }

    private func bubble(_ camera: CIImage, layout: PiPLayout, in size: CGSize) -> CIImage {
        let diameter = (size.height * layout.fraction).rounded()
        let margin = (size.height * 0.035).rounded()
        // Centre-crop to a square, then scale to the bubble.
        let side = min(camera.extent.width, camera.extent.height)
        let crop = CGRect(x: camera.extent.midX - side / 2, y: camera.extent.midY - side / 2, width: side, height: side)
        let scale = diameter / side
        var face = camera.cropped(to: crop)
            .transformed(by: CGAffineTransform(translationX: -crop.minX, y: -crop.minY))
            .transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        let x = layout.corner.hasSuffix("left") ? margin : size.width - diameter - margin
        let y = layout.corner.hasPrefix("top") ? size.height - diameter - margin : margin   // Core Image's origin is bottom-left
        face = face.transformed(by: CGAffineTransform(translationX: x, y: y))
        let rect = CGRect(x: x, y: y, width: diameter, height: diameter)
        let radius = layout.circle ? diameter / 2 : diameter * 0.18
        let mask = CIFilter(name: "CIRoundedRectangleGenerator", parameters: [
            "inputExtent": CIVector(cgRect: rect), "inputRadius": radius, "inputColor": CIColor.white,
        ])!.outputImage!
        let ring = CIFilter(name: "CIRoundedRectangleGenerator", parameters: [
            "inputExtent": CIVector(cgRect: rect.insetBy(dx: -3, dy: -3)), "inputRadius": radius + 3,
            "inputColor": CIColor(red: 1, green: 1, blue: 1, alpha: 0.9),
        ])!.outputImage!
        let masked = face.applyingFilter("CIBlendWithMask", parameters: [
            kCIInputBackgroundImageKey: CIImage.empty(), kCIInputMaskImageKey: mask,
        ])
        return masked.composited(over: ring)
    }
}
