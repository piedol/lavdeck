//! The kit's and capture's errors in the interface's words. Those crates return typed errors
//! (their `Display` is the Swift app's English, for logs and the CLI); everything shown here
//! goes through `tr`, so it translates.

use lavdeck_capture::{CaptureError, Source};
use lavdeck_kit::audio::SplitError;
use lavdeck_kit::protocol::ManifestError;
use lavdeck_kit::{
    CaptureProblem, CaptureProblemKind, InstallError, MicError, PluginProblem, PluginRunError,
    RecorderError,
};

use crate::i18n::{tr, trf};

pub fn mic(e: &MicError) -> String {
    match e {
        MicError::NotConnected => tr("Receiver is not connected"),
        MicError::NotAllowed => tr("Command is outside the verified allowlist"),
        MicError::Timeout => tr("Receiver did not answer"),
        MicError::Rejected { code } => trf(
            "Receiver rejected the command (0x{code})",
            &[("code", &format!("{code:02x}"))],
        ),
        MicError::NotApplied => tr("Receiver accepted the command but its status did not change"),
        MicError::Transport(m) => trf("USB error: {message}", &[("message", m)]),
    }
}

pub fn recorder(e: &RecorderError) -> String {
    match e {
        RecorderError::DeviceNotFound => tr("The DJI receiver is not available as an audio input"),
        RecorderError::PermissionDenied => tr("Microphone access was denied"),
        RecorderError::CannotAddInput(reason) => trf(
            "Cannot use the receiver as input: {reason}",
            &[("reason", reason)],
        ),
        RecorderError::CannotAddOutput => tr("Cannot create the audio file output"),
    }
}

pub fn split(e: &SplitError) -> String {
    match e {
        SplitError::NotStereo => tr("The take isn’t stereo."),
        SplitError::FormatUnavailable => tr("The audio format isn’t available."),
        SplitError::File(m) => m.clone(),
    }
}

pub fn plugin_run(e: &PluginRunError) -> String {
    match e {
        PluginRunError::Launch(m) => trf("Couldn’t start the plugin: {message}", &[("message", m)]),
        PluginRunError::Reported(Some(m)) => m.clone(),
        PluginRunError::Reported(None) => tr("The plugin reported an error"),
        PluginRunError::Exited { status, log_tail } => {
            let mut s = trf(
                "The plugin exited with status {status}.",
                &[("status", &status.to_string())],
            );
            if !log_tail.is_empty() {
                s.push('\n');
                s.push_str(&log_tail.join("\n"));
            }
            s
        }
        PluginRunError::Cancelled => tr("Cancelled"),
    }
}

pub fn manifest(e: &ManifestError) -> String {
    match e {
        ManifestError::Read(io) if io.kind() == std::io::ErrorKind::NotFound => {
            tr("The file “plugin.json” couldn’t be opened because there is no such file.")
        }
        ManifestError::Read(io) if io.kind() == std::io::ErrorKind::PermissionDenied => tr(
            "The file “plugin.json” couldn’t be opened because you don’t have permission to view it.",
        ),
        ManifestError::Read(_) => tr("The file “plugin.json” couldn’t be opened."),
        ManifestError::Decode(m)
            if m.starts_with("missing field") || m.starts_with("invalid type: null") =>
        {
            tr("The data couldn’t be read because it is missing.")
        }
        ManifestError::Decode(_) => {
            tr("The data couldn’t be read because it isn’t in the correct format.")
        }
    }
}

pub fn install(e: &InstallError) -> String {
    match e {
        InstallError::Manifest(m) => manifest(m),
        InstallError::Io(io) => io.to_string(),
    }
}

pub fn plugin_problem(p: &PluginProblem) -> String {
    match p {
        PluginProblem::UnsupportedProtocol { name, protocol } => trf(
            "{plugin}: needs plugin protocol {protocol}, Lavdeck speaks up to {supported}",
            &[
                ("plugin", name),
                ("protocol", &protocol.to_string()),
                (
                    "supported",
                    &lavdeck_kit::protocol::PROTOCOL_VERSION.to_string(),
                ),
            ],
        ),
        PluginProblem::BadManifest { entry, error } => trf(
            "{plugin}: {reason}",
            &[("plugin", entry), ("reason", &manifest(error))],
        ),
    }
}

/// What was recorded, in a sentence: `screen`, `camera`, `video`, `screen or camera`.
fn frames(what: &str) -> String {
    match what {
        "screen" => tr("No screen frames arrived, so nothing was saved."),
        "camera" => tr("No camera frames arrived, so nothing was saved."),
        "screen or camera" => tr("No screen or camera frames arrived, so nothing was saved."),
        _ => tr("No video frames arrived, so nothing was saved."),
    }
}

pub fn capture(e: &CaptureError) -> String {
    match e {
        CaptureError::NotGranted(Source::Camera) => {
            tr("Camera access is off for Lavdeck, so the camera wasn’t recorded.")
        }
        CaptureError::NotGranted(Source::Screen) => {
            tr("Screen recording is off for Lavdeck, so the screen wasn’t recorded.")
        }
        CaptureError::ScreenPermission => tr(
            "Screen recording is off for Lavdeck. Turn on Lavdeck in System Settings › Privacy & Security › Screen & System Audio Recording, then reopen Lavdeck.",
        ),
        CaptureError::CameraPermission => tr(
            "Camera access is off for Lavdeck. Turn it on in System Settings › Privacy & Security › Camera.",
        ),
        CaptureError::NoCamera => tr("No camera found."),
        CaptureError::NoDisplay => tr("No display found to record."),
        CaptureError::NothingRecorded(what) => frames(what),
        CaptureError::Writer(m) => trf("Couldn’t write the video: {message}", &[("message", m)]),
        CaptureError::System(m) => m.clone(),
    }
}

/// Lavdeck's own capture reports its problems as the capture error's English text; these are
/// the ones it can report, to word them again.
fn native_capture_errors() -> Vec<CaptureError> {
    let mut all = vec![
        CaptureError::NotGranted(Source::Camera),
        CaptureError::NotGranted(Source::Screen),
        CaptureError::ScreenPermission,
        CaptureError::CameraPermission,
        CaptureError::NoCamera,
        CaptureError::NoDisplay,
    ];
    for what in ["screen", "camera", "video", "screen or camera"] {
        all.push(CaptureError::NothingRecorded(what.into()));
    }
    all
}

/// A capture source's problem, with its plugin's name: `Video: No camera found.`
pub fn capture_problem(p: &CaptureProblem) -> String {
    let message = match &p.kind {
        CaptureProblemKind::Launch(m) => trf("Couldn’t start: {message}", &[("message", m)]),
        CaptureProblemKind::Timeout { seconds } => trf(
            "Didn’t finish writing within {seconds} seconds and was stopped.",
            &[("seconds", &seconds.to_string())],
        ),
        CaptureProblemKind::Exited { status, log_tail } => trf(
            "Exited with status {status}. {log}",
            &[
                ("status", &status.to_string()),
                ("log", &log_tail.join(" ")),
            ],
        ),
        CaptureProblemKind::Reported(m) => {
            let writer = CaptureError::Writer(String::new()).to_string();
            if let Some(known) = native_capture_errors()
                .into_iter()
                .find(|e| e.to_string() == *m)
            {
                capture(&known)
            } else if let Some(rest) = m.strip_prefix(writer.as_str()) {
                capture(&CaptureError::Writer(rest.to_string()))
            } else {
                m.clone()
            }
        }
    };
    let plugin = if p.plugin_name == "Video" {
        tr("Video")
    } else {
        p.plugin_name.clone()
    };
    trf(
        "{plugin}: {message}",
        &[("plugin", &plugin), ("message", &message)],
    )
}

/// The camera preview's hints, for the capture crate's preview (its keys are listed here so the
/// catalog has them).
pub fn preview_hint(key: &str) -> String {
    match key {
        "Double-click to enlarge" => tr("Double-click to enlarge"),
        "Double-click for full screen" => tr("Double-click for full screen"),
        "Double-click to shrink" => tr("Double-click to shrink"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_capture_problems_are_reworded() {
        let p = CaptureProblem::new("Video", &CaptureError::NoCamera.to_string());
        assert_eq!(capture_problem(&p), "Video: No camera found.");
        let p = CaptureProblem::new(
            "Video",
            &CaptureError::Writer("disk full".into()).to_string(),
        );
        assert_eq!(
            capture_problem(&p),
            "Video: Couldn’t write the video: disk full"
        );
        let p = CaptureProblem::new("Screen & Camera", "camera busy");
        assert_eq!(capture_problem(&p), "Screen & Camera: camera busy");
    }

    #[test]
    fn errors_read_as_the_swift_app_wrote_them() {
        assert_eq!(
            mic(&MicError::Rejected { code: 0x0e }),
            "Receiver rejected the command (0x0e)"
        );
        assert_eq!(
            capture(&CaptureError::NothingRecorded("camera".into())),
            "No camera frames arrived, so nothing was saved."
        );
        assert_eq!(
            plugin_run(&PluginRunError::Exited {
                status: 2,
                log_tail: vec!["boom".into()]
            }),
            "The plugin exited with status 2.\nboom"
        );
        assert_eq!(
            preview_hint("Double-click to shrink"),
            "Double-click to shrink"
        );
    }
}
