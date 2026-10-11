import AVFoundation
import Foundation

public enum RecordingFormat: String, CaseIterable, Sendable {
    case wav, m4a

    var fileType: AVFileType { self == .wav ? .wav : .m4a }

    var settings: [String: Any] {
        switch self {
        case .wav:
            [AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: 48_000, AVNumberOfChannelsKey: 2,
             AVLinearPCMBitDepthKey: 24, AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false]
        case .m4a:
            [AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 48_000, AVNumberOfChannelsKey: 2,
             AVEncoderBitRateKey: 256_000]
        }
    }
}

public enum RecorderError: Error, CustomStringConvertible {
    case deviceNotFound, permissionDenied, cannotAddInput(String), cannotAddOutput

    public var description: String {
        switch self {
        case .deviceNotFound: "The DJI receiver is not available as an audio input"
        case .permissionDenied: "Microphone access was denied"
        case .cannotAddInput(let reason): "Cannot use the receiver as input: \(reason)"
        case .cannotAddOutput: "Cannot create the audio file output"
        }
    }
}

/// Records the DJI receiver's USB audio input to a file.
public final class MicRecorder: NSObject, AVCaptureFileOutputRecordingDelegate, @unchecked Sendable {
    public static let deviceName = "Wireless Mic Rx"

    public var onFinish: (@Sendable (URL, Error?) -> Void)?
    public private(set) var isRecording = false
    public private(set) var currentURL: URL?

    private let session = AVCaptureSession()
    private let output = AVCaptureAudioFileOutput()

    public static func findDevice() -> AVCaptureDevice? {
        AVCaptureDevice.DiscoverySession(deviceTypes: [.microphone, .external], mediaType: .audio, position: .unspecified)
            .devices.first { $0.localizedName == deviceName }
    }

    public static func requestPermission() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: true
        case .notDetermined: await AVCaptureDevice.requestAccess(for: .audio)
        default: false
        }
    }

    public func start(to url: URL, format: RecordingFormat = .wav) throws {
        guard let device = Self.findDevice() else { throw RecorderError.deviceNotFound }
        session.beginConfiguration()
        session.inputs.forEach { session.removeInput($0) }
        do {
            let input = try AVCaptureDeviceInput(device: device)
            guard session.canAddInput(input) else { throw RecorderError.cannotAddInput("rejected by session") }
            session.addInput(input)
        } catch let error as RecorderError {
            session.commitConfiguration()
            throw error
        } catch {
            session.commitConfiguration()
            throw RecorderError.cannotAddInput(error.localizedDescription)
        }
        if !session.outputs.contains(output) {
            guard session.canAddOutput(output) else { session.commitConfiguration(); throw RecorderError.cannotAddOutput }
            session.addOutput(output)
        }
        output.audioSettings = format.settings
        session.commitConfiguration()
        session.startRunning()
        output.startRecording(to: url, outputFileType: format.fileType, recordingDelegate: self)
        currentURL = url
        isRecording = true
    }

    public func stop() {
        guard isRecording else { return }
        output.stopRecording()
    }

    /// Current average power per channel in dBFS (left, right), while recording.
    public func channelLevels() -> [Float] {
        output.connections.first?.audioChannels.map(\.averagePowerLevel) ?? []
    }

    public func fileOutput(_ output: AVCaptureFileOutput, didFinishRecordingTo url: URL,
                           from connections: [AVCaptureConnection], error: Error?) {
        isRecording = false
        session.stopRunning()
        onFinish?(url, error)
    }
}
