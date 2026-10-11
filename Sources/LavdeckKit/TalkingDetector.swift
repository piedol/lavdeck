import Foundation

/// Decides which transmitter someone is speaking into, from the receiver's per-mic meters.
///
/// Only meaningful in stereo: in mono the receiver reports the same mixed level for both mics.
/// Measured on the Mobile RX: ~30-45 at rest, ~75 for loud speech; two mics side by side under the
/// same sound differ by under ~8, while speaking into one mic separates them far more.
public struct TalkingDetector: Sendable {
    /// Minimum level for the louder mic to count as speech.
    public var speechLevel = 52
    /// Minimum lead over the other mic.
    public var margin = 12
    /// How long a decision is held after the evidence stops, so the highlight doesn't flicker.
    public var hold: TimeInterval = 1.5

    public private(set) var talking: Int?
    private var lastEvidence = Date.distantPast

    public init() {}

    /// Feeds one pair of meter readings (TX1, TX2); returns the mic currently judged to be talking.
    @discardableResult
    public mutating func update(tx1: Int?, tx2: Int?, at now: Date = Date()) -> Int? {
        if let a = tx1, let b = tx2 {
            let loud = max(a, b)
            if loud >= speechLevel, abs(a - b) >= margin {
                talking = a > b ? 1 : 2
                lastEvidence = now
                return talking
            }
        }
        if now.timeIntervalSince(lastEvidence) > hold { talking = nil }
        return talking
    }

    public mutating func reset() {
        talking = nil
        lastEvidence = .distantPast
    }
}
