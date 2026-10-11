import AVFoundation
import Foundation

/// Splits a stereo recording into one mono file per channel (left = TX1, right = TX2 in stereo mode).
public enum TrackSplitter {
    public enum SplitError: Error { case notStereo, formatUnavailable }

    /// Writes one channel (0 = left/TX1, 1 = right/TX2) of a stereo file as a mono file.
    public static func extract(_ source: URL, channel: Int, to destination: URL) throws {
        let scratch = destination.deletingLastPathComponent().appendingPathComponent(".\(UUID().uuidString).wav")
        if channel == 0 {
            try split(source, left: destination, right: scratch)
        } else {
            try split(source, left: scratch, right: destination)
        }
        try? FileManager.default.removeItem(at: scratch)
    }

    /// Writes `left` and `right` as mono files in the source's format family (WAV stays 24-bit PCM).
    public static func split(_ source: URL, left: URL, right: URL) throws {
        let input = try AVAudioFile(forReading: source)
        let format = input.processingFormat
        guard format.channelCount == 2 else { throw SplitError.notStereo }
        guard let mono = AVAudioFormat(commonFormat: format.commonFormat, sampleRate: format.sampleRate,
                                       channels: 1, interleaved: false) else { throw SplitError.formatUnavailable }
        var settings = input.fileFormat.settings
        settings[AVNumberOfChannelsKey] = 1
        settings.removeValue(forKey: AVChannelLayoutKey)
        let outLeft = try AVAudioFile(forWriting: left, settings: settings, commonFormat: format.commonFormat, interleaved: false)
        let outRight = try AVAudioFile(forWriting: right, settings: settings, commonFormat: format.commonFormat, interleaved: false)

        let frames: AVAudioFrameCount = 48_000
        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames),
              let l = AVAudioPCMBuffer(pcmFormat: mono, frameCapacity: frames),
              let r = AVAudioPCMBuffer(pcmFormat: mono, frameCapacity: frames) else { throw SplitError.formatUnavailable }
        while input.framePosition < input.length {
            try input.read(into: buffer, frameCount: frames)
            let count = Int(buffer.frameLength)
            guard count > 0, let src = buffer.floatChannelData, let dl = l.floatChannelData, let dr = r.floatChannelData else { break }
            dl[0].update(from: src[0], count: count)
            dr[0].update(from: src[1], count: count)
            l.frameLength = buffer.frameLength
            r.frameLength = buffer.frameLength
            try outLeft.write(from: l)
            try outRight.write(from: r)
        }
    }
}
