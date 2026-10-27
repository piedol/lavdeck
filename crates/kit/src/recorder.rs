//! Records the DJI receiver's USB audio input to a file with AVCaptureSession and
//! AVCaptureAudioFileOutput: WAV (24-bit, 48 kHz, stereo) or M4A (AAC, 256 kbps).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureAudioFileOutput, AVCaptureConnection, AVCaptureDevice,
    AVCaptureDeviceDiscoverySession, AVCaptureDeviceInput, AVCaptureDevicePosition,
    AVCaptureDeviceTypeExternal, AVCaptureDeviceTypeMicrophone, AVCaptureFileOutput,
    AVCaptureFileOutputRecordingDelegate, AVCaptureSession, AVFileType, AVFileTypeAppleM4A,
    AVFileTypeWAVE, AVMediaTypeAudio,
};
use objc2_avf_audio::{
    AVEncoderBitRateKey, AVFormatIDKey, AVLinearPCMBitDepthKey, AVLinearPCMIsBigEndianKey,
    AVLinearPCMIsFloatKey, AVNumberOfChannelsKey, AVSampleRateKey,
};
use objc2_core_media::{CMClock, CMSyncConvertTime, CMTime, CMTimeFlags};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSMutableDictionary, NSNumber, NSObject, NSObjectProtocol,
    NSString, NSURL,
};

/// The receiver's name as a macOS audio input.
pub const RECEIVER_DEVICE_NAME: &str = "Wireless Mic Rx";

const AUDIO_FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const AUDIO_FORMAT_MPEG4_AAC: u32 = u32::from_be_bytes(*b"aac ");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecordingFormat {
    Wav,
    M4a,
}

impl RecordingFormat {
    pub const ALL: [RecordingFormat; 2] = [Self::Wav, Self::M4a];

    /// `wav` or `m4a`: the file extension and the stored setting.
    pub fn raw(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::M4a => "m4a",
        }
    }

    pub fn from_raw(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.raw() == raw)
    }

    fn file_type(self) -> Option<&'static AVFileType> {
        // SAFETY: framework constants.
        unsafe {
            match self {
                Self::Wav => AVFileTypeWAVE,
                Self::M4a => AVFileTypeAppleM4A,
            }
        }
    }

    fn settings(self) -> Retained<NSDictionary<NSString, AnyObject>> {
        let settings = NSMutableDictionary::<NSString, AnyObject>::new();
        let set = |key: Option<&'static NSString>, value: Retained<NSNumber>| {
            if let Some(key) = key {
                settings.insert(key, &value);
            }
        };
        // SAFETY: framework constants.
        unsafe {
            match self {
                Self::Wav => {
                    set(AVFormatIDKey, NSNumber::new_u32(AUDIO_FORMAT_LINEAR_PCM));
                    set(AVSampleRateKey, NSNumber::new_isize(48_000));
                    set(AVNumberOfChannelsKey, NSNumber::new_isize(2));
                    set(AVLinearPCMBitDepthKey, NSNumber::new_isize(24));
                    set(AVLinearPCMIsFloatKey, NSNumber::new_bool(false));
                    set(AVLinearPCMIsBigEndianKey, NSNumber::new_bool(false));
                }
                Self::M4a => {
                    set(AVFormatIDKey, NSNumber::new_u32(AUDIO_FORMAT_MPEG4_AAC));
                    set(AVSampleRateKey, NSNumber::new_isize(48_000));
                    set(AVNumberOfChannelsKey, NSNumber::new_isize(2));
                    set(AVEncoderBitRateKey, NSNumber::new_isize(256_000));
                }
            }
        }
        Retained::into_super(settings)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecorderError {
    DeviceNotFound,
    PermissionDenied,
    CannotAddInput(String),
    CannotAddOutput,
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceNotFound => {
                f.write_str("The DJI receiver is not available as an audio input")
            }
            Self::PermissionDenied => f.write_str("Microphone access was denied"),
            Self::CannotAddInput(reason) => write!(f, "Cannot use the receiver as input: {reason}"),
            Self::CannotAddOutput => f.write_str("Cannot create the audio file output"),
        }
    }
}

impl std::error::Error for RecorderError {}

/// Lavdeck's microphone permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicPermission {
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
}

/// The current microphone permission, without asking.
pub fn microphone_permission() -> MicPermission {
    // SAFETY: framework constant and class method.
    let status = unsafe {
        AVMediaTypeAudio.map(|audio| AVCaptureDevice::authorizationStatusForMediaType(audio))
    };
    match status {
        Some(AVAuthorizationStatus::Authorized) => MicPermission::Authorized,
        Some(AVAuthorizationStatus::NotDetermined) => MicPermission::NotDetermined,
        Some(AVAuthorizationStatus::Restricted) => MicPermission::Restricted,
        _ => MicPermission::Denied,
    }
}

/// True when Lavdeck may record. Asks the user if they haven't decided yet, blocking until they
/// answer, so don't call it on the main thread while that would stall the UI.
pub fn request_microphone_permission() -> bool {
    match microphone_permission() {
        MicPermission::Authorized => true,
        MicPermission::NotDetermined => {
            let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
                return false;
            };
            let (sender, receiver) = mpsc::sync_channel(1);
            let handler = RcBlock::new(move |granted: Bool| {
                let _ = sender.try_send(granted.as_bool());
            });
            // SAFETY: the block is copied by AVFoundation and called once.
            unsafe {
                AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler)
            };
            receiver.recv().unwrap_or(false)
        }
        _ => false,
    }
}

/// Finds an audio input by its name, e.g. [`RECEIVER_DEVICE_NAME`].
pub fn find_device(name: &str) -> Option<Retained<AVCaptureDevice>> {
    // SAFETY: framework constants and a discovery query.
    unsafe {
        let types =
            NSArray::from_slice(&[AVCaptureDeviceTypeMicrophone, AVCaptureDeviceTypeExternal]);
        let session =
            AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
                &types,
                AVMediaTypeAudio,
                AVCaptureDevicePosition::Unspecified,
            );
        session
            .devices()
            .iter()
            .find(|d| d.localizedName().to_string() == name)
    }
}

type FinishCallback = Arc<dyn Fn(PathBuf, Option<String>) + Send + Sync>;

#[derive(Default)]
struct State {
    is_recording: bool,
    current: Option<PathBuf>,
    start_host_time: Option<f64>,
    on_finish: Option<FinishCallback>,
}

/// AVFoundation objects; AVCaptureSession and its outputs may be driven from any one thread at a
/// time, which `Shared::control` ensures.
struct Capture {
    session: Retained<AVCaptureSession>,
    output: Retained<AVCaptureAudioFileOutput>,
    delegate: Retained<Delegate>,
}

// SAFETY: see `Capture`; every use goes through `Shared::control` or is a thread-safe read.
unsafe impl Send for Capture {}
unsafe impl Sync for Capture {}

struct Shared {
    device_name: String,
    capture: Capture,
    control: Mutex<()>,
    state: Mutex<State>,
}

/// Records one file at a time. Clones share the recorder.
///
/// AVCaptureAudioFileOutput finishes a file on the main thread: `stop` waits for it there, so the
/// main thread's run loop must be running (as it is in the app), and the finish callback follows
/// from an AVFoundation thread once the file is written.
#[derive(Clone)]
pub struct MicRecorder {
    shared: Arc<Shared>,
}

impl Default for MicRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl MicRecorder {
    /// A recorder for the DJI receiver's input.
    pub fn new() -> Self {
        Self::with_device_name(RECEIVER_DEVICE_NAME)
    }

    /// A recorder for another input by name (e.g. a built-in microphone, for testing).
    pub fn with_device_name(name: &str) -> Self {
        let shared = Arc::new_cyclic(|weak: &Weak<Shared>| Shared {
            device_name: name.into(),
            capture: Capture {
                // SAFETY: plain initialisers.
                session: unsafe { AVCaptureSession::new() },
                output: unsafe { AVCaptureAudioFileOutput::new() },
                delegate: Delegate::new(weak.clone()),
            },
            control: Mutex::new(()),
            state: Mutex::new(State::default()),
        });
        Self { shared }
    }

    /// Called on an AVFoundation thread when a file is finished: its path, and the error if
    /// recording stopped because of one.
    pub fn set_on_finish(
        &self,
        callback: impl Fn(PathBuf, Option<String>) + Send + Sync + 'static,
    ) {
        self.shared.state.lock().unwrap().on_finish = Some(Arc::new(callback));
    }

    pub fn is_recording(&self) -> bool {
        self.shared.state.lock().unwrap().is_recording
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.shared.state.lock().unwrap().current.clone()
    }

    /// Host-clock seconds of the current file's first audio sample, once recording has started.
    /// Capture plugins' files are lined up against this.
    pub fn start_host_time(&self) -> Option<f64> {
        self.shared.state.lock().unwrap().start_host_time
    }

    pub fn start(&self, path: &Path, format: RecordingFormat) -> Result<(), RecorderError> {
        let device = find_device(&self.shared.device_name).ok_or(RecorderError::DeviceNotFound)?;
        let _control = self.shared.control.lock().unwrap();
        let Capture {
            session,
            output,
            delegate,
        } = &self.shared.capture;
        // SAFETY: AVCaptureSession configuration on the one thread holding `control`.
        unsafe {
            session.beginConfiguration();
            for input in session.inputs().iter() {
                session.removeInput(&input);
            }
            let input = match AVCaptureDeviceInput::deviceInputWithDevice_error(&device) {
                Ok(input) => input,
                Err(error) => {
                    session.commitConfiguration();
                    return Err(RecorderError::CannotAddInput(
                        error.localizedDescription().to_string(),
                    ));
                }
            };
            if !session.canAddInput(&input) {
                session.commitConfiguration();
                return Err(RecorderError::CannotAddInput("rejected by session".into()));
            }
            session.addInput(&input);
            let output_base: &objc2_av_foundation::AVCaptureOutput = output;
            if !session
                .outputs()
                .iter()
                .any(|o| std::ptr::eq(&*o, output_base))
            {
                if !session.canAddOutput(output) {
                    session.commitConfiguration();
                    return Err(RecorderError::CannotAddOutput);
                }
                session.addOutput(output);
            }
            output.setAudioSettings(Some(&format.settings()));
            session.commitConfiguration();
            session.startRunning();
            self.shared.state.lock().unwrap().start_host_time = None;
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            let Some(file_type) = format.file_type() else {
                return Err(RecorderError::CannotAddOutput);
            };
            output.startRecordingToOutputFileURL_outputFileType_recordingDelegate(
                &url,
                file_type,
                ProtocolObject::from_ref(&**delegate),
            );
        }
        let mut state = self.shared.state.lock().unwrap();
        state.current = Some(path.to_path_buf());
        state.is_recording = true;
        Ok(())
    }

    /// Finishes the file; the finish callback follows once it is written.
    pub fn stop(&self) {
        if !self.is_recording() {
            return;
        }
        let _control = self.shared.control.lock().unwrap();
        // SAFETY: stopping the output on the thread holding `control`.
        unsafe { self.shared.capture.output.stopRecording() };
    }

    /// Current average power per channel in dBFS (left, right), while recording.
    pub fn channel_levels(&self) -> Vec<f32> {
        // SAFETY: reading the output's connection meters.
        unsafe {
            let connections = self.shared.capture.output.connections();
            let Some(connection): Option<Retained<AVCaptureConnection>> = connections.firstObject()
            else {
                return Vec::new();
            };
            connection
                .audioChannels()
                .iter()
                .map(|c| c.averagePowerLevel())
                .collect()
        }
    }
}

impl Shared {
    fn did_start(&self, start_pts: CMTime) {
        // SAFETY: CoreMedia clock conversion between the session's clock and the host clock.
        let seconds = unsafe {
            let host = CMClock::host_time_clock();
            let clock = self.capture.session.synchronizationClock();
            let from: &CMClock = clock.as_deref().unwrap_or(&host);
            let converted = CMSyncConvertTime(start_pts, from, &host);
            converted
                .flags
                .contains(CMTimeFlags::Valid)
                .then(|| converted.seconds())
        };
        self.state.lock().unwrap().start_host_time = seconds;
    }

    fn did_finish(&self, url: &NSURL, error: Option<&NSError>) {
        let callback = {
            let mut state = self.state.lock().unwrap();
            state.is_recording = false;
            state.on_finish.clone()
        };
        // SAFETY: stopping the session once the file is done.
        unsafe { self.capture.session.stopRunning() };
        let path = url
            .path()
            .map(|p| PathBuf::from(p.to_string()))
            .unwrap_or_default();
        if let Some(callback) = callback {
            callback(path, error.map(|e| e.localizedDescription().to_string()));
        }
    }
}

struct DelegateIvars {
    shared: Weak<Shared>,
}

define_class!(
    // SAFETY: NSObject subclass; the callbacks only touch `Shared`, which is thread-safe.
    #[unsafe(super(NSObject))]
    #[name = "LavdeckKitRecordingDelegate"]
    #[ivars = DelegateIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl AVCaptureFileOutputRecordingDelegate for Delegate {
        #[unsafe(method(captureOutput:didStartRecordingToOutputFileAtURL:startPTS:fromConnections:))]
        fn did_start(
            &self,
            _output: &AVCaptureFileOutput,
            _url: &NSURL,
            start_pts: CMTime,
            _connections: &NSArray<AVCaptureConnection>,
        ) {
            if let Some(shared) = self.ivars().shared.upgrade() {
                shared.did_start(start_pts);
            }
        }

        #[unsafe(method(captureOutput:didFinishRecordingToOutputFileAtURL:fromConnections:error:))]
        fn did_finish(
            &self,
            _output: &AVCaptureFileOutput,
            url: &NSURL,
            _connections: &NSArray<AVCaptureConnection>,
            error: Option<&NSError>,
        ) {
            if let Some(shared) = self.ivars().shared.upgrade() {
                shared.did_finish(url, error);
            }
        }
    }
);

impl Delegate {
    fn new(shared: Weak<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { shared });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_round_trip_and_carry_the_swift_settings() {
        assert_eq!(RecordingFormat::from_raw("m4a"), Some(RecordingFormat::M4a));
        assert_eq!(RecordingFormat::from_raw("caf"), None);
        let wav = RecordingFormat::Wav.settings();
        let get = |key: Option<&NSString>| -> Option<i64> {
            wav.objectForKey(key?)
                .and_then(|v| v.downcast::<NSNumber>().ok())
                .map(|n| n.as_i64())
        };
        unsafe {
            assert_eq!(get(AVFormatIDKey), Some(AUDIO_FORMAT_LINEAR_PCM as i64));
            assert_eq!(get(AVSampleRateKey), Some(48_000));
            assert_eq!(get(AVLinearPCMBitDepthKey), Some(24));
            assert_eq!(get(AVNumberOfChannelsKey), Some(2));
        }
        let m4a = RecordingFormat::M4a.settings();
        let bitrate = unsafe { m4a.objectForKey(AVEncoderBitRateKey.unwrap()) }
            .and_then(|v| v.downcast::<NSNumber>().ok());
        assert_eq!(bitrate.map(|n| n.as_i64()), Some(256_000));
    }

    #[test]
    fn errors_read_like_swift() {
        assert_eq!(
            RecorderError::DeviceNotFound.to_string(),
            "The DJI receiver is not available as an audio input"
        );
        assert_eq!(
            RecorderError::CannotAddInput("busy".into()).to_string(),
            "Cannot use the receiver as input: busy"
        );
    }

    #[test]
    fn a_missing_input_is_reported_before_touching_the_session() {
        let recorder = MicRecorder::with_device_name("No Such Microphone 1234");
        let path = std::env::temp_dir().join("lavdeck-never-written.wav");
        assert_eq!(
            recorder.start(&path, RecordingFormat::Wav),
            Err(RecorderError::DeviceNotFound)
        );
        assert!(!recorder.is_recording() && !path.exists());
    }
}
