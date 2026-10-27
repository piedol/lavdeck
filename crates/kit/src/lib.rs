//! Lavdeck's engine, shared by the menu bar app and the `lavdeck` command-line tool.
//!
//! - The receiver: the USB link on interface 6 ([`MicController`]), DUML framing ([`duml`]), the
//!   status it pushes ([`status`]) and the closed command allowlist ([`commands`]).
//! - Its buttons ([`ButtonMonitor`]) and who is talking ([`TalkingDetector`]).
//! - Recording ([`MicRecorder`]), splitting stereo into per-mic tracks ([`audio`]), and takes on
//!   disk ([`takes`], [`transcript`]).
//! - Plugins: the protocol ([`protocol`], [`capture_protocol`]), installed plugins
//!   ([`PluginStore`]), post jobs ([`runner`]) and capture processes ([`CaptureHost`]).
//!
//! Nothing here touches AppKit. Long calls block and callbacks arrive on background threads; the
//! app hops to its main thread itself.

pub mod audio;
pub mod buttons;
pub mod capture_host;
pub mod capture_protocol;
pub mod commands;
pub mod controller;
pub mod defaults;
pub mod duml;
pub mod json;
pub mod plugins;
pub mod protocol;
pub mod recorder;
pub mod runner;
mod spawn;
pub mod status;
pub mod takes;
pub mod talking;
pub mod transcript;
mod usb;

pub use buttons::{ButtonMonitor, MicButton, MicButtonEvent};
pub use capture_host::{
    CaptureHost, CaptureOutput, CaptureProblem, CaptureProblemKind, CaptureResult,
};
pub use capture_protocol::{
    CaptureCommand, CaptureEvent, CaptureEventKind, CaptureSpec, CaptureTake, HostClock,
};
pub use commands::{MicCapabilities, MicCapability, MicCommand};
pub use controller::{Link, LinkError, LinkEvents, MicController, MicError};
pub use plugins::{InstallError, InstalledPlugin, PluginProblem, PluginStore};
pub use protocol::{
    AppSpec, JsonValue, PluginAction, PluginManifest, PluginMessage, PluginMessageKind,
    PluginRequest, PluginSetting, PluginSettingKind, TakeDescriptor, TakeFile, TakeRole,
    TakeSpeaker,
};
pub use recorder::{MicPermission, MicRecorder, RecorderError, RecordingFormat};
pub use runner::{CancelToken, PluginJob, PluginRunError, PluginRunResult};
pub use status::{ChannelMode, DeviceIdentity, MicStatus, ReceiverStatus, TransmitterStatus};
pub use takes::{Take, TakeMedia, TakeMetadata, TakeMic};
pub use talking::TalkingDetector;
pub use transcript::TranscriptDocument;

/// USB IDs of the DJI Mic Series Mobile RX.
pub const RECEIVER_VENDOR_ID: i32 = usb::VENDOR_ID;
pub const RECEIVER_PRODUCT_ID: i32 = usb::PRODUCT_ID;

/// Runs the current thread's run loop for `duration`, so a command-line tool receives HID events
/// and lets AVFoundation finish files on the main thread.
pub fn run_current_run_loop(duration: std::time::Duration) {
    // SAFETY: reading a framework constant.
    if let Some(mode) = unsafe { objc2_core_foundation::kCFRunLoopDefaultMode } {
        objc2_core_foundation::CFRunLoop::run_in_mode(Some(mode), duration.as_secs_f64(), false);
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh folder under the temporary directory.
    pub fn scratch_folder() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let folder =
            std::env::temp_dir().join(format!("lavdeck-kit-tests-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    pub fn make_executable(path: &Path, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
