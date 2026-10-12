import AVFoundation
import CoreMedia
import Foundation
import LavdeckPluginKit
import ScreenCaptureKit

/// Records the main display (and optionally system audio) to a .mov with ScreenCaptureKit.
/// Sample buffers are stamped on the host clock, so the first frame's PTS is its host time.
final class ScreenRecorder: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private let url: URL
    private let fps: Int
    private let hevc: Bool
    private let systemAudio: Bool
    private let queue = DispatchQueue(label: "lavdeck.capture.screen")
    private var stream: SCStream?
    private var writer: AVAssetWriter?
    private var videoInput: AVAssetWriterInput?
    private var audioInput: AVAssetWriterInput?
    private var startPTS: CMTime?
    private var lastPTS: CMTime?
    private var failure: String?
    /// Called on the capture queue with the host time of the first frame written.
    var onStarted: (@Sendable (Double) -> Void)?

    init(url: URL, fps: Int, hevc: Bool, systemAudio: Bool) {
        self.url = url
        self.fps = fps
        self.hevc = hevc
        self.systemAudio = systemAudio
    }

    func start(excludingWindowNumbers: [Int]) async throws {
        var content: SCShareableContent
        do {
            content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            // The camera bubble was just shown; wait until the window server lists it, or it can't be left out.
            var tries = 0
            while !excludingWindowNumbers.isEmpty, tries < 15,
                  !content.windows.contains(where: { excludingWindowNumbers.contains(Int($0.windowID)) }) {
                try await Task.sleep(for: .milliseconds(100))
                content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
                tries += 1
            }
        } catch {
            throw CaptureError.screenPermission
        }
        guard let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() }) ?? content.displays.first else {
            throw CaptureError.noDisplay
        }
        // Leave Lavdeck's own windows (the popover, Recordings, the camera preview) out of the recording.
        let isLavdeck: (SCRunningApplication?) -> Bool = { $0?.bundleIdentifier == "io.github.piedol.lavdeck" }
        let filter: SCContentFilter
        if content.applications.contains(where: { $0.processID == getpid() }) {
            let apps = content.applications.filter { isLavdeck($0) || $0.processID == getpid() }
            filter = SCContentFilter(display: display, excludingApplications: apps, exceptingWindows: [])
        } else {
            // Not listed as an app (e.g. a development build): leave out the preview and Lavdeck's windows by window.
            let windows = content.windows.filter {
                excludingWindowNumbers.contains(Int($0.windowID)) || isLavdeck($0.owningApplication)
            }
            filter = SCContentFilter(display: display, excludingWindows: windows)
        }

        let scale = CGFloat(filter.pointPixelScale)
        var width = Int(filter.contentRect.width * scale), height = Int(filter.contentRect.height * scale)
        let maxWidth = 3840
        if width > maxWidth {
            height = Int(Double(height) * Double(maxWidth) / Double(width))
            width = maxWidth
        }
        width -= width % 2
        height -= height % 2

        let config = SCStreamConfiguration()
        config.width = width
        config.height = height
        config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        config.colorSpaceName = CGColorSpace.sRGB
        config.showsCursor = true
        config.queueDepth = 8
        if systemAudio {
            config.capturesAudio = true
            config.sampleRate = 48_000
            config.channelCount = 2
            config.excludesCurrentProcessAudio = true
        }

        let writer = try AVAssetWriter(outputURL: url, fileType: .mov)
        let bitsPerPixel = hevc ? 0.06 : 0.1
        let video = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: hevc ? AVVideoCodecType.hevc : AVVideoCodecType.h264,
            AVVideoWidthKey: width,
            AVVideoHeightKey: height,
            AVVideoCompressionPropertiesKey: [
                AVVideoAverageBitRateKey: Int(Double(width * height * fps) * bitsPerPixel),
                AVVideoExpectedSourceFrameRateKey: fps,
            ],
        ])
        video.expectsMediaDataInRealTime = true
        writer.add(video)
        if systemAudio {
            let audio = AVAssetWriterInput(mediaType: .audio, outputSettings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 48_000, AVNumberOfChannelsKey: 2, AVEncoderBitRateKey: 192_000,
            ])
            audio.expectsMediaDataInRealTime = true
            writer.add(audio)
            audioInput = audio
        }
        self.writer = writer
        videoInput = video

        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        if systemAudio { try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue) }
        do { try await stream.startCapture() } catch {
            throw (error as NSError).code == SCStreamError.userDeclined.rawValue ? CaptureError.screenPermission : error
        }
        self.stream = stream
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard sampleBuffer.isValid, let writer else { return }
        switch type {
        case .screen:
            guard let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
                  let raw = attachments.first?[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete else { return }
            let pts = sampleBuffer.presentationTimeStamp
            if startPTS == nil {
                guard writer.startWriting() else { failure = writer.error?.localizedDescription; return }
                writer.startSession(atSourceTime: pts)
                startPTS = pts
                onStarted?(pts.seconds)
            }
            if videoInput?.isReadyForMoreMediaData == true, videoInput?.append(sampleBuffer) == true { lastPTS = pts }
        case .audio:
            guard startPTS != nil, audioInput?.isReadyForMoreMediaData == true else { return }
            audioInput?.append(sampleBuffer)
        default:
            break
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        queue.async { self.failure = error.localizedDescription }
    }

    func stop() async -> Result<RecordedFile, Error> {
        try? await stream?.stopCapture()
        stream = nil
        return await withCheckedContinuation { continuation in
            queue.async {
                guard let writer = self.writer, let start = self.startPTS else {
                    self.writer?.cancelWriting()
                    continuation.resume(returning: .failure(CaptureError.nothingRecorded(self.failure ?? "screen")))
                    return
                }
                self.videoInput?.markAsFinished()
                self.audioInput?.markAsFinished()
                if let last = self.lastPTS { writer.endSession(atSourceTime: last) }
                writer.finishWriting {
                    if writer.status == .completed {
                        continuation.resume(returning: .success(RecordedFile(url: self.url, source: "screen", startHostTime: start.seconds)))
                    } else {
                        continuation.resume(returning: .failure(CaptureError.writer(writer.error?.localizedDescription ?? "unknown")))
                    }
                }
            }
        }
    }
}
