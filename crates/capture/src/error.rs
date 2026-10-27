//! Why a recording or the finished video failed. `Display` gives the Swift app's English text, which is
//! also the app's translation key.

use std::fmt;

use crate::capture::Source;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// Lavdeck doesn't hold the permission for this source, so it wasn't started.
    NotGranted(Source),
    /// ScreenCaptureKit refused: screen recording is off for this build.
    ScreenPermission,
    /// Camera access is off for this build.
    CameraPermission,
    NoCamera,
    NoDisplay,
    /// No frames of this kind arrived (`screen`, `camera`, `video`, `screen or camera`), or the reason they
    /// didn't.
    NothingRecorded(String),
    /// The file couldn't be written; the system's message.
    Writer(String),
    /// Any other system error, with its localized description.
    System(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::NotGranted(Source::Camera) => {
                f.write_str("Camera access is off for Lavdeck, so the camera wasn’t recorded.")
            }
            CaptureError::NotGranted(Source::Screen) => {
                f.write_str("Screen recording is off for Lavdeck, so the screen wasn’t recorded.")
            }
            CaptureError::ScreenPermission => f.write_str(
                "Screen recording is off for Lavdeck. Turn on Lavdeck in System Settings › Privacy & Security › Screen & System Audio Recording, then reopen Lavdeck.",
            ),
            CaptureError::CameraPermission => f.write_str(
                "Camera access is off for Lavdeck. Turn it on in System Settings › Privacy & Security › Camera.",
            ),
            CaptureError::NoCamera => f.write_str("No camera found."),
            CaptureError::NoDisplay => f.write_str("No display found to record."),
            CaptureError::NothingRecorded(what) => write!(f, "No {what} frames arrived, so nothing was saved."),
            CaptureError::Writer(message) => write!(f, "Couldn’t write the video: {message}"),
            CaptureError::System(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CaptureError {}

impl From<objc2::rc::Retained<objc2_foundation::NSError>> for CaptureError {
    fn from(error: objc2::rc::Retained<objc2_foundation::NSError>) -> Self {
        CaptureError::System(error.localizedDescription().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_match_the_swift_app() {
        assert_eq!(
            CaptureError::NothingRecorded("camera".into()).to_string(),
            "No camera frames arrived, so nothing was saved."
        );
        assert_eq!(
            CaptureError::Writer("disk full".into()).to_string(),
            "Couldn’t write the video: disk full"
        );
        assert_eq!(
            CaptureError::NotGranted(Source::Screen).to_string(),
            "Screen recording is off for Lavdeck, so the screen wasn’t recorded."
        );
    }
}
