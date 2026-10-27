//! Records the main display (and optionally system audio) to a .mov with ScreenCaptureKit and an
//! AVAssetWriter. Sample buffers are stamped on the host clock, so the first frame's PTS is its host time.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, Message, define_class, msg_send};
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus, AVFileTypeQuickTimeMovie,
    AVMediaTypeAudio, AVMediaTypeVideo, AVVideoAverageBitRateKey, AVVideoCodecKey,
    AVVideoCodecTypeH264, AVVideoCodecTypeHEVC, AVVideoCompressionPropertiesKey,
    AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_avf_audio::{AVEncoderBitRateKey, AVFormatIDKey, AVNumberOfChannelsKey, AVSampleRateKey};
use objc2_core_foundation::CFArray;
use objc2_core_graphics::{CGMainDisplayID, kCGColorSpaceSRGB};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use objc2_screen_capture_kit::{
    SCContentFilter, SCFrameStatus, SCRunningApplication, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamDelegate, SCStreamErrorCode, SCStreamFrameInfoStatus,
    SCStreamOutput, SCStreamOutputType, SCWindow,
};

use crate::error::CaptureError;
use crate::layout;
use crate::util::{Shared, any, dictionary, file_url, integer, wait};

/// The bundle identifier of the Lavdeck app, whose windows never appear in the recording.
const LAVDECK_BUNDLE_ID: &str = "io.github.piedol.lavdeck";

/// A finished recording: the file and the host-clock time of its first frame.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedFile {
    pub path: PathBuf,
    pub source: &'static str,
    pub start_host_time: Option<f64>,
}

#[derive(Default)]
struct State {
    writer: Option<Shared<Retained<AVAssetWriter>>>,
    video: Option<Shared<Retained<AVAssetWriterInput>>>,
    audio: Option<Shared<Retained<AVAssetWriterInput>>>,
    start_pts: Option<CMTime>,
    last_pts: Option<CMTime>,
    failure: Option<String>,
}

struct Inner {
    path: PathBuf,
    state: Mutex<State>,
    /// Called on the capture queue with the host time of the first frame written.
    on_started: Box<dyn Fn(f64) + Send + Sync>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and ScreenOutput has no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "LavdeckScreenOutput"]
    #[ivars = Arc<Inner>]
    struct ScreenOutput;

    unsafe impl NSObjectProtocol for ScreenOutput {}

    unsafe impl SCStreamOutput for ScreenOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            self.ivars().sample(sample, kind);
        }
    }

    unsafe impl SCStreamDelegate for ScreenOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            self.ivars().state().failure = Some(error.localizedDescription().to_string());
        }
    }
);

impl ScreenOutput {
    fn new(inner: Arc<Inner>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(inner);
        unsafe { msg_send![super(this), init] }
    }
}

/// `SCFrameStatus` of a screen sample, from its `SCStreamFrameInfo` attachments.
fn frame_status(sample: &CMSampleBuffer) -> Option<SCFrameStatus> {
    let attachments = unsafe { sample.sample_attachments_array(false) }?;
    // SAFETY: CFArray of CFDictionary is toll-free bridged to NSArray of NSDictionary.
    let attachments: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*(&*attachments as *const CFArray).cast() };
    let first = attachments.firstObject()?;
    let raw = first.objectForKey(unsafe { SCStreamFrameInfoStatus })?;
    Some(SCFrameStatus(raw.downcast_ref::<NSNumber>()?.as_isize()))
}

impl Inner {
    fn sample(&self, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
        if !unsafe { sample.is_valid() } {
            return;
        }
        let mut state = self.state();
        let Some(writer) = state.writer.as_ref().map(|w| w.0.clone()) else {
            return;
        };
        if kind == SCStreamOutputType::Screen {
            if frame_status(sample) != Some(SCFrameStatus::Complete) {
                return;
            }
            let pts = unsafe { sample.presentation_time_stamp() };
            if state.start_pts.is_none() {
                if !unsafe { writer.startWriting() } {
                    state.failure =
                        unsafe { writer.error() }.map(|e| e.localizedDescription().to_string());
                    return;
                }
                unsafe { writer.startSessionAtSourceTime(pts) };
                state.start_pts = Some(pts);
                (self.on_started)(unsafe { pts.seconds() });
            }
            if let Some(video) = &state.video
                && unsafe { video.isReadyForMoreMediaData() }
                && unsafe { video.appendSampleBuffer(sample) }
            {
                state.last_pts = Some(pts);
            }
        } else if kind == SCStreamOutputType::Audio {
            if state.start_pts.is_none() {
                return;
            }
            if let Some(audio) = &state.audio
                && unsafe { audio.isReadyForMoreMediaData() }
            {
                unsafe { audio.appendSampleBuffer(sample) };
            }
        }
    }
}

pub struct ScreenRecorder {
    inner: Arc<Inner>,
    stream: Shared<Retained<SCStream>>,
    queue: DispatchRetained<DispatchQueue>,
    // Keeps the output and delegate alive as long as the stream.
    _output: Shared<Retained<ScreenOutput>>,
}

fn shareable_content() -> Result<Shared<Retained<SCShareableContent>>, CaptureError> {
    let result = wait(|tx| {
        let done = RcBlock::new(
            move |content: *mut SCShareableContent, _error: *mut NSError| {
                let _ = tx.send(unsafe { content.as_ref() }.map(|c| Shared(c.retain())));
            },
        );
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                false, true, &done,
            )
        };
    });
    // Without screen recording permission there is no content to share.
    result.flatten().ok_or(CaptureError::ScreenPermission)
}

fn is_lavdeck(app: Option<&SCRunningApplication>) -> bool {
    app.is_some_and(|a| unsafe { a.bundleIdentifier() }.to_string() == LAVDECK_BUNDLE_ID)
}

impl ScreenRecorder {
    /// Starts recording the main display to `path`, leaving Lavdeck's windows (and the camera preview,
    /// `excluding` window numbers) out. Blocks until the stream has started, so call it off the main
    /// thread; `on_started` runs on the capture queue with the first frame's host time.
    pub fn start(
        path: &Path,
        fps: u32,
        hevc: bool,
        system_audio: bool,
        excluding: &[isize],
        on_started: impl Fn(f64) + Send + Sync + 'static,
    ) -> Result<ScreenRecorder, CaptureError> {
        let listed = |content: &SCShareableContent| {
            unsafe { content.windows() }
                .iter()
                .any(|w| excluding.contains(&(unsafe { w.windowID() } as isize)))
        };
        let mut content = shareable_content()?;
        // The camera bubble was just shown; wait until the window server lists it, or it can't be left out.
        let mut tries = 0;
        while !excluding.is_empty() && tries < 15 && !listed(&content) {
            std::thread::sleep(Duration::from_millis(100));
            content = shareable_content()?;
            tries += 1;
        }
        let displays = unsafe { content.displays() };
        let main = CGMainDisplayID();
        let display = displays
            .iter()
            .find(|d| unsafe { d.displayID() } == main)
            .or_else(|| displays.firstObject())
            .ok_or(CaptureError::NoDisplay)?;

        // Leave Lavdeck's own windows (the popover, Recordings, the camera preview) out of the recording.
        let me = std::process::id() as libc::pid_t;
        let apps = unsafe { content.applications() };
        let filter = if apps.iter().any(|a| unsafe { a.processID() } == me) {
            let ours: Vec<Retained<SCRunningApplication>> = apps
                .iter()
                .filter(|a| is_lavdeck(Some(a)) || unsafe { a.processID() } == me)
                .collect();
            unsafe {
                SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&ours),
                    &NSArray::new(),
                )
            }
        } else {
            // Not listed as an app (e.g. a development build): leave out the preview and Lavdeck's windows
            // by window.
            let windows: Vec<Retained<SCWindow>> = unsafe { content.windows() }
                .iter()
                .filter(|w| {
                    excluding.contains(&(unsafe { w.windowID() } as isize))
                        || is_lavdeck(unsafe { w.owningApplication() }.as_deref())
                })
                .collect();
            unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&windows),
                )
            }
        };

        let scale = unsafe { filter.pointPixelScale() } as f64;
        let rect = unsafe { filter.contentRect() };
        let (width, height) = layout::screen_pixels(rect.size.width, rect.size.height, scale);

        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setWidth(width as usize);
            config.setHeight(height as usize);
            config.setMinimumFrameInterval(CMTime::new(1, fps as i32));
            config.setPixelFormat(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
            config.setColorSpaceName(kCGColorSpaceSRGB);
            config.setShowsCursor(true);
            config.setQueueDepth(8);
            if system_audio {
                config.setCapturesAudio(true);
                config.setSampleRate(48_000);
                config.setChannelCount(2);
                config.setExcludesCurrentProcessAudio(true);
            }
        }

        let writer = new_writer(path)?;
        let video = video_input(width, height, fps, hevc);
        add_input(&writer, &video)?;
        let audio = if system_audio {
            let audio = audio_input();
            add_input(&writer, &audio)?;
            Some(Shared(audio))
        } else {
            None
        };
        let inner = Arc::new(Inner {
            path: path.to_path_buf(),
            state: Mutex::new(State {
                writer: Some(Shared(writer)),
                video: Some(Shared(video)),
                audio,
                ..State::default()
            }),
            on_started: Box::new(on_started),
        });

        let output = ScreenOutput::new(inner.clone());
        let queue = DispatchQueue::new("lavdeck.capture.screen", None);
        let delegate = ProtocolObject::from_ref(&*output);
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(delegate),
            )
        };
        let sink = ProtocolObject::from_ref(&*output);
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                sink,
                SCStreamOutputType::Screen,
                Some(&queue),
            )
        }?;
        if system_audio {
            unsafe {
                stream.addStreamOutput_type_sampleHandlerQueue_error(
                    sink,
                    SCStreamOutputType::Audio,
                    Some(&queue),
                )
            }?;
        }
        let started = wait(|tx| {
            let done = RcBlock::new(move |error: *mut NSError| {
                let code = unsafe { error.as_ref() }
                    .map(|e| (e.code(), e.localizedDescription().to_string()));
                let _ = tx.send(code);
            });
            unsafe { stream.startCaptureWithCompletionHandler(Some(&done)) };
        })
        .flatten();
        if let Some((code, message)) = started {
            return Err(if code == SCStreamErrorCode::UserDeclined.0 {
                CaptureError::ScreenPermission
            } else {
                CaptureError::System(message)
            });
        }
        Ok(ScreenRecorder {
            inner,
            stream: Shared(stream),
            queue,
            _output: Shared(output),
        })
    }

    /// Stops the stream and finishes the file. `done` runs on a framework queue.
    pub fn stop(self, done: impl FnOnce(Result<RecordedFile, CaptureError>) + Send + 'static) {
        let ScreenRecorder {
            inner,
            stream,
            queue,
            _output: output,
        } = self;
        let alive = Shared((stream.0.clone(), output.0));
        let pending = Mutex::new(Some(move || {
            queue.exec_async(move || {
                // Keep the stream and its output until it has stopped and the last frame is in.
                let _alive = alive;
                finish(&inner, done);
            });
        }));
        let stopped = RcBlock::new(move |_error: *mut NSError| {
            if let Some(f) = pending.lock().ok().and_then(|mut f| f.take()) {
                f();
            }
        });
        unsafe { stream.stopCaptureWithCompletionHandler(Some(&stopped)) };
    }
}

fn finish(
    inner: &Arc<Inner>,
    done: impl FnOnce(Result<RecordedFile, CaptureError>) + Send + 'static,
) {
    let state = inner.state();
    let (Some(writer), Some(start)) = (state.writer.as_ref().map(|w| w.0.clone()), state.start_pts)
    else {
        if let Some(w) = &state.writer {
            unsafe { w.cancelWriting() };
        }
        let what = state.failure.clone().unwrap_or_else(|| "screen".into());
        drop(state);
        return done(Err(CaptureError::NothingRecorded(what)));
    };
    unsafe {
        if let Some(v) = &state.video {
            v.markAsFinished();
        }
        if let Some(a) = &state.audio {
            a.markAsFinished();
        }
        if let Some(last) = state.last_pts {
            writer.endSessionAtSourceTime(last);
        }
    }
    drop(state);
    let path = inner.path.clone();
    let start = unsafe { start.seconds() };
    finish_writing(writer, move |result| {
        done(result.map(|()| RecordedFile {
            path,
            source: "screen",
            start_host_time: Some(start),
        }))
    });
}

/// `finishWriting`, then whether the file is complete.
pub fn finish_writing(
    writer: Retained<AVAssetWriter>,
    done: impl FnOnce(Result<(), CaptureError>) + Send + 'static,
) {
    let done = Mutex::new(Some(done));
    let w = Shared(writer.clone());
    let block = RcBlock::new(move || {
        let Some(done) = done.lock().ok().and_then(|mut d| d.take()) else {
            return;
        };
        if unsafe { w.status() } == AVAssetWriterStatus::Completed {
            done(Ok(()));
        } else {
            let message = unsafe { w.error() }.map(|e| e.localizedDescription().to_string());
            done(Err(CaptureError::Writer(
                message.unwrap_or_else(|| "unknown".into()),
            )));
        }
    });
    unsafe { writer.finishWritingWithCompletionHandler(&block) };
}

pub fn new_writer(path: &Path) -> Result<Retained<AVAssetWriter>, CaptureError> {
    let kind = unsafe { AVFileTypeQuickTimeMovie }.expect("AVFileTypeQuickTimeMovie");
    Ok(unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&file_url(path), kind) }?)
}

pub fn add_input(writer: &AVAssetWriter, input: &AVAssetWriterInput) -> Result<(), CaptureError> {
    if !unsafe { writer.canAddInput(input) } {
        return Err(CaptureError::Writer("encoder settings were refused".into()));
    }
    unsafe { writer.addInput(input) };
    Ok(())
}

/// An H.264 or HEVC input with explicit settings: the "recommended" ones can come back without a codec
/// (uncompressed 4:2:0).
pub fn encoder_input(
    width: i64,
    height: i64,
    bit_rate: i64,
    frame_rate: i64,
    hevc: bool,
) -> Retained<AVAssetWriterInput> {
    let key = |k: Option<&'static NSString>| k.expect("AVVideoSettings key");
    let codec = unsafe {
        if hevc {
            AVVideoCodecTypeHEVC
        } else {
            AVVideoCodecTypeH264
        }
    }
    .expect("codec type");
    let (bit_rate, frame_rate, width, height) = (
        integer(bit_rate),
        integer(frame_rate),
        integer(width),
        integer(height),
    );
    unsafe {
        let compression = dictionary(&[
            (key(AVVideoAverageBitRateKey), any(&*bit_rate)),
            (key(AVVideoExpectedSourceFrameRateKey), any(&*frame_rate)),
        ]);
        let settings = dictionary(&[
            (key(AVVideoCodecKey), any(codec)),
            (key(AVVideoWidthKey), any(&*width)),
            (key(AVVideoHeightKey), any(&*height)),
            (key(AVVideoCompressionPropertiesKey), any(&*compression)),
        ]);
        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            AVMediaTypeVideo.expect("AVMediaTypeVideo"),
            Some(&settings),
        );
        input.setExpectsMediaDataInRealTime(true);
        input
    }
}

fn video_input(width: i64, height: i64, fps: u32, hevc: bool) -> Retained<AVAssetWriterInput> {
    encoder_input(
        width,
        height,
        layout::screen_bit_rate(width, height, fps, hevc),
        fps as i64,
        hevc,
    )
}

fn audio_input() -> Retained<AVAssetWriterInput> {
    let key = |k: Option<&'static NSString>| k.expect("AVAudioSettings key");
    // kAudioFormatMPEG4AAC ('aac ').
    let (format, rate, channels, bit_rate) = (
        integer(0x6161_6320),
        integer(48_000),
        integer(2),
        integer(192_000),
    );
    unsafe {
        let settings = dictionary(&[
            (key(AVFormatIDKey), any(&*format)),
            (key(AVSampleRateKey), any(&*rate)),
            (key(AVNumberOfChannelsKey), any(&*channels)),
            (key(AVEncoderBitRateKey), any(&*bit_rate)),
        ]);
        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            AVMediaTypeAudio.expect("AVMediaTypeAudio"),
            Some(&settings),
        );
        input.setExpectsMediaDataInRealTime(true);
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::ClassType;
    use objc2::runtime::ProtocolObject;

    #[test]
    fn output_class_registers_with_its_protocols() {
        // Registering checks each method's signature against the protocol (in debug builds).
        let class = ScreenOutput::class();
        assert_eq!(class.name().to_str().unwrap(), "LavdeckScreenOutput");
        let inner = Arc::new(Inner {
            path: "/x".into(),
            state: Mutex::default(),
            on_started: Box::new(|_| {}),
        });
        let output = ScreenOutput::new(inner);
        let _: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);
        let _: &ProtocolObject<dyn SCStreamDelegate> = ProtocolObject::from_ref(&*output);
    }
}
