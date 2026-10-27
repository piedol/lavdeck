//! Lavdeck's own screen and camera recording, the floating camera preview, and each take's finished
//! picture-in-picture video. See `docs/architecture.md`.
//!
//! Recording runs on the main thread with callbacks ([`VideoCapture`]); the composer
//! ([`VideoComposer::make`]) blocks its calling thread, so run it off the main thread. Takes, capture
//! outputs and problems, and the host clock are the kit's types.

mod bubble;
mod camera;
mod capture;
mod compose;
mod error;
mod layout;
mod screen;
mod settings;
mod util;

pub use bubble::CameraBubble;
pub use capture::{
    FIRST_FRAME_TIMEOUT, PERMISSION_CAMERA, PERMISSION_SCREEN_RECORDING, Source, VideoCapture,
    wait_until,
};
pub use compose::VideoComposer;
pub use error::CaptureError;
pub use settings::{BubbleShape, BubbleSize, Codec, Corner, PreviewSize, VideoSettings};
