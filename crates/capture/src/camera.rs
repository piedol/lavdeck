//! Records the default camera to a .mov. The same capture session feeds the camera preview.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use dispatch2::{DispatchQueue, DispatchRetained};
use lavdeck_kit::HostClock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice,
    AVCaptureDeviceDiscoverySession, AVCaptureDeviceFormat, AVCaptureDeviceInput,
    AVCaptureDevicePosition, AVCaptureDeviceTypeBuiltInWideAngleCamera,
    AVCaptureDeviceTypeExternal, AVCaptureOutput, AVCaptureSession,
    AVCaptureSessionPreset1920x1080, AVCaptureSessionPresetHigh, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_media::{
    CMClock, CMSampleBuffer, CMSyncConvertTime, CMTime, CMTimeFlags,
    CMVideoFormatDescriptionGetDimensions,
};
use objc2_core_video::{
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation::{NSArray, NSNumber, NSObject, NSObjectProtocol};

use crate::error::CaptureError;
use crate::layout::{self, CameraFormat};
use crate::screen::{RecordedFile, add_input, encoder_input, finish_writing, new_writer};
use crate::util::{Shared, any, cf_ns, dictionary};

#[derive(Default)]
struct State {
    writer: Option<Shared<Retained<AVAssetWriter>>>,
    input: Option<Shared<Retained<AVAssetWriterInput>>>,
    start_pts: Option<CMTime>,
    start_host_time: Option<f64>,
    last_pts: Option<CMTime>,
    recording: bool,
    failure: Option<String>,
}

struct Inner {
    path: PathBuf,
    hevc: bool,
    frame_rate: f64,
    session: Shared<Retained<AVCaptureSession>>,
    state: Mutex<State>,
    /// Called on the camera queue with the host time of the first frame written.
    on_started: Box<dyn Fn(f64) + Send + Sync>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and CameraOutput has no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "LavdeckCameraOutput"]
    #[ivars = Arc<Inner>]
    struct CameraOutput;

    unsafe impl NSObjectProtocol for CameraOutput {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CameraOutput {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(&self, _output: &AVCaptureOutput, sample: &CMSampleBuffer, _connection: &AVCaptureConnection) {
            self.ivars().sample(sample);
        }
    }
);

impl CameraOutput {
    fn new(inner: Arc<Inner>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(inner);
        unsafe { msg_send![super(this), init] }
    }
}

impl Inner {
    /// The encoder is sized from the first frame the camera delivers, so the file is always full
    /// resolution.
    fn make_input(&self, width: i64, height: i64) -> Retained<AVAssetWriterInput> {
        let bit_rate = layout::camera_bit_rate(width, height, self.frame_rate, self.hevc);
        encoder_input(width, height, bit_rate, self.frame_rate as i64, self.hevc)
    }

    fn sample(&self, sample: &CMSampleBuffer) {
        let mut state = self.state();
        let Some(writer) = state.writer.as_ref().map(|w| w.0.clone()) else {
            return;
        };
        if !state.recording {
            return;
        }
        let pts = unsafe { sample.presentation_time_stamp() };
        if state.start_pts.is_none() {
            let Some(frame) = (unsafe { sample.image_buffer() }) else {
                return;
            };
            let input = self.make_input(
                CVPixelBufferGetWidth(&frame) as i64,
                CVPixelBufferGetHeight(&frame) as i64,
            );
            if add_input(&writer, &input).is_err() {
                state.failure = Some("camera encoder settings were refused".into());
                state.recording = false;
                return;
            }
            state.input = Some(Shared(input));
            if !unsafe { writer.startWriting() } {
                state.failure =
                    unsafe { writer.error() }.map(|e| e.localizedDescription().to_string());
                state.recording = false;
                return;
            }
            unsafe { writer.startSessionAtSourceTime(pts) };
            state.start_pts = Some(pts);
            // Camera frames are stamped on the session's clock; convert to the host clock to line up.
            let host = unsafe { CMClock::host_time_clock() };
            let clock = unsafe { self.session.synchronizationClock() };
            let from = clock.as_deref().unwrap_or(&host);
            let converted = unsafe { CMSyncConvertTime(pts, from, &host) };
            state.start_host_time = converted
                .flags
                .contains(CMTimeFlags::Valid)
                .then(|| unsafe { converted.seconds() });
            (self.on_started)(state.start_host_time.unwrap_or_else(HostClock::now));
        }
        if let Some(input) = &state.input
            && unsafe { input.isReadyForMoreMediaData() }
            && unsafe { input.appendSampleBuffer(sample) }
        {
            state.last_pts = Some(pts);
        }
    }
}

pub struct CameraRecorder {
    /// The capture session, for the camera preview.
    pub session: Retained<AVCaptureSession>,
    inner: Arc<Inner>,
    /// Delivers frames (the sample buffer delegate queue).
    queue: DispatchRetained<DispatchQueue>,
    /// Starts and stops the session, which blocks for a moment.
    session_queue: DispatchRetained<DispatchQueue>,
    _delegate: Shared<Retained<CameraOutput>>,
    _output: Shared<Retained<AVCaptureVideoDataOutput>>,
}

fn format_info(format: &AVCaptureDeviceFormat) -> CameraFormat {
    let d = unsafe { CMVideoFormatDescriptionGetDimensions(&format.formatDescription()) };
    let max_frame_rate = unsafe { format.videoSupportedFrameRateRanges() }
        .iter()
        .map(|r| unsafe { r.maxFrameRate() })
        .fold(None, |best: Option<f64>, r| {
            Some(best.map_or(r, |b| b.max(r)))
        });
    CameraFormat {
        width: d.width,
        height: d.height,
        max_frame_rate,
    }
}

impl CameraRecorder {
    /// Opens the default camera at its largest landscape format, ready to record to `path`.
    pub fn make(
        path: &Path,
        hevc: bool,
        on_started: impl Fn(f64) + Send + Sync + 'static,
    ) -> Result<CameraRecorder, CaptureError> {
        let video = unsafe { AVMediaTypeVideo }.expect("AVMediaTypeVideo");
        // Lavdeck asks for camera access; this only checks it.
        if unsafe { AVCaptureDevice::authorizationStatusForMediaType(video) }
            != AVAuthorizationStatus::Authorized
        {
            return Err(CaptureError::CameraPermission);
        }
        let device = unsafe { AVCaptureDevice::defaultDeviceWithMediaType(video) }.or_else(|| {
            let types = unsafe {
                NSArray::from_slice(&[
                    AVCaptureDeviceTypeBuiltInWideAngleCamera,
                    AVCaptureDeviceTypeExternal,
                ])
            };
            let discovery = unsafe {
                AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
                    &types,
                    Some(video),
                    AVCaptureDevicePosition::Unspecified,
                )
            };
            unsafe { discovery.devices() }.firstObject()
        });
        let device = device.ok_or(CaptureError::NoCamera)?;
        let session = unsafe { AVCaptureSession::new() };
        let frame_rate = configure_device(&session, &device)?;

        let inner = Arc::new(Inner {
            path: path.to_path_buf(),
            hevc,
            frame_rate,
            session: Shared(session.clone()),
            state: Mutex::new(State::default()),
            on_started: Box::new(on_started),
        });
        let queue = DispatchQueue::new("lavdeck.capture.camera", None);
        let delegate = CameraOutput::new(inner.clone());
        let output = unsafe { AVCaptureVideoDataOutput::new() };
        unsafe {
            let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
            let settings = dictionary(&[(cf_ns(kCVPixelBufferPixelFormatTypeKey), any(&*format))]);
            output.setVideoSettings(Some(&settings));
            output.setAlwaysDiscardsLateVideoFrames(false);
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&queue),
            );
            if !session.canAddOutput(&output) {
                session.commitConfiguration();
                return Err(CaptureError::NoCamera);
            }
            session.addOutput(&output);
            session.commitConfiguration();
        }
        inner.state().writer = Some(Shared(new_writer(path)?));
        Ok(CameraRecorder {
            session,
            inner,
            queue,
            session_queue: DispatchQueue::new("lavdeck.capture.camera.session", None),
            _delegate: Shared(delegate),
            _output: Shared(output),
        })
    }

    /// Starts the camera. Frames are recorded from the first one it delivers.
    pub fn start(&self) {
        let inner = self.inner.clone();
        self.session_queue.exec_async(move || {
            inner.state().recording = true;
            unsafe { inner.session.startRunning() };
        });
    }

    /// Stops the camera and finishes the file. `done` runs on a framework queue.
    pub fn stop(self, done: impl FnOnce(Result<RecordedFile, CaptureError>) + Send + 'static) {
        let CameraRecorder {
            inner,
            queue,
            session_queue,
            _delegate: delegate,
            _output: output,
            ..
        } = self;
        session_queue.exec_async(move || {
            unsafe { inner.session.stopRunning() };
            // Frames already queued for the delegate land before this.
            queue.exec_async(move || {
                let _alive = (delegate, output);
                finish(&inner, done);
            });
        });
    }
}

/// Adds the device to the session and picks its format. Returns the frame rate it runs at. Leaves the
/// session's configuration open for the output.
fn configure_device(
    session: &AVCaptureSession,
    device: &AVCaptureDevice,
) -> Result<f64, CaptureError> {
    unsafe { session.beginConfiguration() };
    let input = match unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(device) } {
        Ok(input) => input,
        Err(error) => {
            unsafe { session.commitConfiguration() };
            return Err(error.into());
        }
    };
    if !unsafe { session.canAddInput(&input) } {
        unsafe { session.commitConfiguration() };
        return Err(CaptureError::NoCamera);
    }
    unsafe { session.addInput(&input) };
    // Pick the camera's largest landscape format at its highest frame rate (the MacBook Pro camera tops out
    // at 1920x1080, 30 fps). A session preset alone only applies when the session starts, so reading the
    // format before then gets whatever mode the camera was last left in, often 720p.
    let formats = unsafe { device.formats() }.to_vec();
    let infos: Vec<CameraFormat> = formats.iter().map(|f| format_info(f)).collect();
    if let Some(best) = layout::best_camera_format(&infos)
        && unsafe { device.lockForConfiguration() }.is_ok()
    {
        let fps = layout::camera_frame_rate(&infos[best]);
        unsafe {
            device.setActiveFormat(&formats[best]);
            device.setActiveVideoMinFrameDuration(CMTime::new(1, fps as i32));
            device.setActiveVideoMaxFrameDuration(CMTime::new(1, fps as i32));
            device.unlockForConfiguration();
        }
        Ok(fps)
    } else {
        unsafe {
            if session.canSetSessionPreset(AVCaptureSessionPreset1920x1080) {
                session.setSessionPreset(AVCaptureSessionPreset1920x1080);
            } else {
                session.setSessionPreset(AVCaptureSessionPresetHigh);
            }
        }
        Ok(30.0)
    }
}

fn finish(
    inner: &Arc<Inner>,
    done: impl FnOnce(Result<RecordedFile, CaptureError>) + Send + 'static,
) {
    let mut state = inner.state();
    state.recording = false;
    let (Some(writer), Some(_)) = (state.writer.as_ref().map(|w| w.0.clone()), state.start_pts)
    else {
        if let Some(w) = &state.writer {
            unsafe { w.cancelWriting() };
        }
        let what = state.failure.clone().unwrap_or_else(|| "camera".into());
        drop(state);
        return done(Err(CaptureError::NothingRecorded(what)));
    };
    unsafe {
        if let Some(input) = &state.input {
            input.markAsFinished();
        }
        if let Some(last) = state.last_pts {
            writer.endSessionAtSourceTime(last);
        }
    }
    let start = state.start_host_time;
    drop(state);
    let path = inner.path.clone();
    finish_writing(writer, move |result| {
        done(result.map(|()| RecordedFile {
            path,
            source: "camera",
            start_host_time: start,
        }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::ClassType;

    #[test]
    fn host_clock_is_core_medias_host_time_clock() {
        // First frames are converted to CMClockGetHostTimeClock; the audio's start is HostClock.
        let before = HostClock::now();
        let cm = unsafe { CMClock::host_time_clock().time().seconds() };
        let after = HostClock::now();
        assert!(
            before <= cm + 1e-6 && cm <= after + 1e-6,
            "{before} {cm} {after}"
        );
    }

    #[test]
    fn output_class_registers_with_its_protocol() {
        let class = CameraOutput::class();
        assert_eq!(class.name().to_str().unwrap(), "LavdeckCameraOutput");
        // An empty session: no camera is opened.
        let session = unsafe { AVCaptureSession::new() };
        let inner = Arc::new(Inner {
            path: "/x".into(),
            hevc: true,
            frame_rate: 30.0,
            session: Shared(session),
            state: Mutex::default(),
            on_started: Box::new(|_| {}),
        });
        let output = CameraOutput::new(inner);
        let _: &ProtocolObject<dyn AVCaptureVideoDataOutputSampleBufferDelegate> =
            ProtocolObject::from_ref(&*output);
    }
}
