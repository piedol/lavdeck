import AVFoundation
import CoreMedia
import Foundation
import LavdeckPluginKit

/// Records the default camera to a .mov. The same capture session feeds the camera bubble.
final class CameraRecorder: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate, @unchecked Sendable {
    let session = AVCaptureSession()
    private let url: URL
    private let output = AVCaptureVideoDataOutput()
    private let queue = DispatchQueue(label: "lavdeck.capture.camera")
    private var writer: AVAssetWriter?
    private var input: AVAssetWriterInput?
    private var startPTS: CMTime?
    private var startHostTime: Double?
    private var lastPTS: CMTime?
    private var recording = false
    private var failure: String?
    private var hevc = true
    /// Called on the camera queue with the host time of the first frame written.
    var onStarted: (@Sendable (Double) -> Void)?
    private var frameRate = 30.0

    private init(url: URL) {
        self.url = url
    }

    static func make(url: URL, hevc: Bool) async throws -> CameraRecorder {
        // Lavdeck asks for camera access; this process only checks it.
        guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else { throw CaptureError.cameraPermission }
        let discovery = AVCaptureDevice.DiscoverySession(deviceTypes: [.builtInWideAngleCamera, .external],
                                                         mediaType: .video, position: .unspecified)
        guard let device = AVCaptureDevice.default(for: .video) ?? discovery.devices.first else { throw CaptureError.noCamera }
        let recorder = CameraRecorder(url: url)
        try recorder.configure(device: device, hevc: hevc)
        return recorder
    }

    private func configure(device: AVCaptureDevice, hevc: Bool) throws {
        session.beginConfiguration()
        let deviceInput = try AVCaptureDeviceInput(device: device)
        guard session.canAddInput(deviceInput) else { session.commitConfiguration(); throw CaptureError.noCamera }
        session.addInput(deviceInput)
        // Pick the camera's largest landscape format at its highest frame rate (the MacBook Pro camera tops out
        // at 1920x1080, 30 fps). A session preset alone only applies when the session starts, so reading the
        // format before then gets whatever mode the camera was last left in, often 720p.
        let landscape = device.formats.filter {
            let d = CMVideoFormatDescriptionGetDimensions($0.formatDescription)
            return d.width >= d.height && d.width <= 3840
        }
        func score(_ f: AVCaptureDevice.Format) -> (Int32, Double) {
            let d = CMVideoFormatDescriptionGetDimensions(f.formatDescription)
            return (d.width * d.height, f.videoSupportedFrameRateRanges.map(\.maxFrameRate).max() ?? 0)
        }
        if let best = landscape.max(by: { score($0) < score($1) }), (try? device.lockForConfiguration()) != nil {
            device.activeFormat = best
            let fps = min(60, best.videoSupportedFrameRateRanges.map(\.maxFrameRate).max() ?? 30)
            device.activeVideoMinFrameDuration = CMTime(value: 1, timescale: CMTimeScale(fps))
            device.activeVideoMaxFrameDuration = CMTime(value: 1, timescale: CMTimeScale(fps))
            device.unlockForConfiguration()
            frameRate = fps
        } else {
            session.sessionPreset = session.canSetSessionPreset(.hd1920x1080) ? .hd1920x1080 : .high
        }
        output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
        output.alwaysDiscardsLateVideoFrames = false
        output.setSampleBufferDelegate(self, queue: queue)
        guard session.canAddOutput(output) else { session.commitConfiguration(); throw CaptureError.noCamera }
        session.addOutput(output)
        session.commitConfiguration()
        self.hevc = hevc
        writer = try AVAssetWriter(outputURL: url, fileType: .mov)
    }

    /// The encoder is sized from the first frame the camera delivers, so the file is always full resolution.
    private func makeInput(width: Int, height: Int) -> AVAssetWriterInput {
        // Webcam video is noisy; too few bits turn it into mush. About 7.5 Mbps HEVC or 12 Mbps H.264 at 1080p30.
        let bitsPerPixel = hevc ? 0.12 : 0.2
        let settings: [String: Any] = [
            // Explicit settings: the "recommended" ones can come back without a codec (uncompressed 4:2:0).
            AVVideoCodecKey: hevc ? AVVideoCodecType.hevc : AVVideoCodecType.h264,
            AVVideoWidthKey: width,
            AVVideoHeightKey: height,
            AVVideoCompressionPropertiesKey: [
                AVVideoAverageBitRateKey: Int(Double(width * height) * frameRate * bitsPerPixel),
                AVVideoExpectedSourceFrameRateKey: Int(frameRate),
            ],
        ]
        let input = AVAssetWriterInput(mediaType: .video, outputSettings: settings)
        input.expectsMediaDataInRealTime = true
        return input
    }

    func start() {
        queue.async { self.recording = true }
        session.startRunning()
    }

    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        guard recording, let writer else { return }
        let pts = sampleBuffer.presentationTimeStamp
        if startPTS == nil {
            guard let frame = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
            let input = makeInput(width: CVPixelBufferGetWidth(frame), height: CVPixelBufferGetHeight(frame))
            guard writer.canAdd(input) else { failure = "camera encoder settings were refused"; recording = false; return }
            writer.add(input)
            self.input = input
            guard writer.startWriting() else { failure = writer.error?.localizedDescription; recording = false; return }
            writer.startSession(atSourceTime: pts)
            startPTS = pts
            let host = CMClockGetHostTimeClock()
            let converted = CMSyncConvertTime(pts, from: session.synchronizationClock ?? host, to: host)
            startHostTime = converted.isValid ? converted.seconds : nil
            onStarted?(startHostTime ?? HostClock.now())
        }
        if let input, input.isReadyForMoreMediaData, input.append(sampleBuffer) { lastPTS = pts }
    }

    func stop() async -> Result<RecordedFile, Error> {
        session.stopRunning()
        return await withCheckedContinuation { continuation in
            queue.async {
                self.recording = false
                guard let writer = self.writer, self.startPTS != nil else {
                    self.writer?.cancelWriting()
                    continuation.resume(returning: .failure(CaptureError.nothingRecorded(self.failure ?? "camera")))
                    return
                }
                self.input?.markAsFinished()
                if let last = self.lastPTS { writer.endSession(atSourceTime: last) }
                writer.finishWriting {
                    if writer.status == .completed {
                        continuation.resume(returning: .success(RecordedFile(url: self.url, source: "camera", startHostTime: self.startHostTime)))
                    } else {
                        continuation.resume(returning: .failure(CaptureError.writer(writer.error?.localizedDescription ?? "unknown")))
                    }
                }
            }
        }
    }
}
