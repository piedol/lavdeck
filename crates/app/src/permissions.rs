//! System permissions capture and plugins can need. Lavdeck asks for each one itself, once,
//! however many plugins declare it; plugin processes run under Lavdeck's grants. The demo
//! only reads the states and never prompts.

use std::cell::Cell;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_app_kit::NSWorkspace;
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use objc2_foundation::NSURL;

use crate::i18n::tr;
use crate::util::{self, ns};

pub const SCREEN: &str = "screen-recording";
pub const CAMERA: &str = "camera";
pub const MICROPHONE: &str = "microphone";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Granted,
    NotAsked,
    Denied,
}

thread_local! {
    /// macOS applies a new screen recording grant only after Lavdeck relaunches, so ask at
    /// most once per launch; after that, say what to do instead of prompting again.
    static ASKED_SCREEN: Cell<bool> = const { Cell::new(false) };
}

fn av_state(video: bool) -> State {
    // SAFETY: AVFoundation's media type constants.
    let ty = unsafe {
        if video {
            AVMediaTypeVideo
        } else {
            AVMediaTypeAudio
        }
    };
    let Some(ty) = ty else { return State::Denied };
    // SAFETY: a valid media type.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(ty) } {
        AVAuthorizationStatus::Authorized => State::Granted,
        AVAuthorizationStatus::NotDetermined => State::NotAsked,
        _ => State::Denied,
    }
}

pub fn state(permission: &str) -> State {
    match permission {
        CAMERA => av_state(true),
        MICROPHONE => av_state(false),
        SCREEN => {
            if CGPreflightScreenCaptureAccess() {
                State::Granted
            } else {
                State::Denied
            }
        }
        _ => State::Granted,
    }
}

pub fn granted(permissions: &[&str]) -> Vec<String> {
    permissions
        .iter()
        .filter(|p| state(p) == State::Granted)
        .map(|p| p.to_string())
        .collect()
}

/// Asks for any of `permissions` not yet granted: the system prompt for camera and microphone,
/// macOS's screen recording prompt at most once per launch.
pub fn request(permissions: &[&str]) {
    if !crate::store::backend().may_prompt() {
        return;
    }
    let mut seen = Vec::new();
    for p in permissions {
        if seen.contains(p) || state(p) == State::Granted {
            continue;
        }
        seen.push(*p);
        match *p {
            CAMERA | MICROPHONE => {
                // SAFETY: AVFoundation's media type constants.
                let ty = unsafe {
                    if *p == CAMERA {
                        AVMediaTypeVideo
                    } else {
                        AVMediaTypeAudio
                    }
                };
                let Some(ty) = ty else { continue };
                let done = RcBlock::new(|_granted: Bool| {
                    util::main_async(|| crate::store::update(|_| {}))
                });
                // SAFETY: a valid media type; the block may run on any thread and only queues work.
                unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(ty, &done) };
            }
            SCREEN => {
                if !ASKED_SCREEN.with(|a| a.replace(true)) {
                    let _ = CGRequestScreenCaptureAccess();
                }
            }
            _ => {}
        }
    }
    crate::store::update(|_| {});
}

/// Opens the permission's pane in System Settings.
pub fn open_settings(permission: &str) {
    let anchor = match permission {
        CAMERA => "Privacy_Camera",
        MICROPHONE => "Privacy_Microphone",
        _ => "Privacy_ScreenCapture",
    };
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    if let Some(u) = NSURL::URLWithString(&ns(&url)) {
        NSWorkspace::sharedWorkspace().openURL(&u);
    }
    crate::store::update(|_| {});
}

/// The permission a capture source needs: `screen` needs `screen-recording`.
pub fn for_source(source: &str) -> Option<&'static str> {
    match source {
        "screen" => Some(SCREEN),
        "camera" => Some(CAMERA),
        _ => None,
    }
}

pub fn name(permission: &str) -> String {
    match permission {
        SCREEN => tr("Screen recording"),
        CAMERA => tr("Camera"),
        MICROPHONE => tr("Microphone"),
        "network" => tr("Network"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_map_to_permissions() {
        assert_eq!(for_source("screen"), Some(SCREEN));
        assert_eq!(for_source("camera"), Some(CAMERA));
        assert_eq!(for_source("system-audio"), None);
    }
}
