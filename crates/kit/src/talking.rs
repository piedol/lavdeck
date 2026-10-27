//! Decides which transmitter someone is speaking into, from the receiver's per-mic meters.
//!
//! Only meaningful in stereo: in mono the receiver reports the same mixed level for both mics.
//! Measured on the Mobile RX: ~30-45 at rest, ~75 for loud speech; two mics side by side under the
//! same sound differ by under ~8, while speaking into one mic separates them far more.

use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct TalkingDetector {
    /// Minimum level for the louder mic to count as speech.
    pub speech_level: i32,
    /// Minimum lead over the other mic.
    pub margin: i32,
    /// How long a decision is held after the evidence stops, so the highlight doesn't flicker.
    pub hold: Duration,
    talking: Option<i32>,
    last_evidence: Option<Instant>,
}

impl Default for TalkingDetector {
    fn default() -> Self {
        Self {
            speech_level: 52,
            margin: 12,
            hold: Duration::from_secs_f64(1.5),
            talking: None,
            last_evidence: None,
        }
    }
}

impl TalkingDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// The mic currently judged to be talking (1 or 2).
    pub fn talking(&self) -> Option<i32> {
        self.talking
    }

    /// Feeds one pair of meter readings (TX1, TX2); returns the mic currently judged to be talking.
    pub fn update(&mut self, tx1: Option<i32>, tx2: Option<i32>) -> Option<i32> {
        self.update_at(tx1, tx2, Instant::now())
    }

    pub fn update_at(&mut self, tx1: Option<i32>, tx2: Option<i32>, now: Instant) -> Option<i32> {
        if let (Some(a), Some(b)) = (tx1, tx2)
            && a.max(b) >= self.speech_level
            && (a - b).abs() >= self.margin
        {
            self.talking = Some(if a > b { 1 } else { 2 });
            self.last_evidence = Some(now);
            return self.talking;
        }
        let held = self
            .last_evidence
            .is_some_and(|last| now.saturating_duration_since(last) <= self.hold);
        if !held {
            self.talking = None;
        }
        self.talking
    }

    pub fn reset(&mut self) {
        self.talking = None;
        self.last_evidence = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talking_detector_picks_the_louder_mic_and_holds() {
        let mut detector = TalkingDetector::new();
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        // At rest and under a shared sound the two mics stay close: nobody is "talking".
        assert_eq!(detector.update_at(Some(34), Some(31), t0), None);
        assert_eq!(detector.update_at(Some(65), Some(58), t0), None);
        // Speaking into TX2 separates the meters.
        assert_eq!(detector.update_at(Some(40), Some(74), at(1)), Some(2));
        // Held briefly through a pause, then released.
        assert_eq!(detector.update_at(Some(38), Some(40), at(2)), Some(2));
        assert_eq!(detector.update_at(Some(38), Some(40), at(3)), None);
        // Switching to TX1.
        assert_eq!(detector.update_at(Some(70), Some(41), at(4)), Some(1));
        // A missing reading counts as no evidence.
        assert_eq!(detector.update_at(None, Some(80), at(10)), None);
    }
}
