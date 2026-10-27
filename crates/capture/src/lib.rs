//! Lavdeck's own screen and camera recording, the floating camera preview, and each take's finished
//! picture-in-picture video. Ported from `Sources/LavdeckCapture`; see `docs/rust-port.md`.
//!
//! Recording runs on the main thread with callbacks ([`VideoCapture`]); the composer
//! ([`VideoComposer::make`]) blocks its calling thread, so run it off the main thread.

mod bubble;
mod camera;
mod capture;
mod compose;
mod error;
pub mod host_clock;
mod layout;
mod screen;
mod settings;
mod take;
mod util;

pub use bubble::CameraBubble;
pub use capture::{
    FIRST_FRAME_TIMEOUT, PERMISSION_CAMERA, PERMISSION_SCREEN_RECORDING, Source, StopResult,
    VideoCapture, wait_until,
};
pub use compose::VideoComposer;
pub use error::CaptureError;
pub use settings::{BubbleShape, BubbleSize, Codec, Corner, PreviewSize, VideoSettings};
pub use take::{CaptureOutput, TakeDescriptor, TakeFile, TakeRole};
