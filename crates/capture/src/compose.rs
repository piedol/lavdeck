//! Makes a take's finished video, `<stem> PiP.mp4`: the screen full frame with the camera in a corner (or
//! the camera alone), and the mic audio, lined up by the offsets Lavdeck recorded. System audio recorded
//! with the screen is kept. A custom AVFoundation compositor draws each frame with Core Image.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, ClassType, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAssetExportPresetHEVCHighestQuality, AVAssetExportPresetHighestQuality, AVAssetExportSession,
    AVAssetExportSessionStatus, AVAssetTrack, AVAsynchronousKeyValueLoading,
    AVAsynchronousVideoCompositionRequest, AVFileTypeMPEG4, AVKeyValueStatus, AVMediaType,
    AVMediaTypeAudio, AVMediaTypeVideo, AVMutableComposition, AVMutableVideoComposition,
    AVURLAsset, AVVideoCompositing, AVVideoCompositionInstructionProtocol,
    AVVideoCompositionRenderContext,
};
use objc2_core_foundation::{CFRetained, CGAffineTransform, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
use objc2_core_image::{
    CIColor, CIContext, CIFilter, CIImage, CIVector, kCIContextCacheIntermediates,
};
use objc2_core_image::{kCIInputBackgroundImageKey, kCIInputMaskImageKey};
use objc2_core_media::{
    CMPersistentTrackID, CMTime, CMTimeRange, kCMPersistentTrackID_Invalid, kCMTimeZero,
};
use objc2_core_video::{kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString, NSValue,
};

use crate::error::CaptureError;
use crate::layout::{self, PipLayout, Rect};
use crate::settings::VideoSettings;
use crate::take::{TakeDescriptor, TakeFile, TakeRole};
use crate::util::{Shared, any, cf_ns, dictionary, file_url, ns, number, wait};

/// Makes and remakes the finished video.
pub struct VideoComposer;

impl VideoComposer {
    /// The file the finished video is written to.
    pub fn output_path(take: &TakeDescriptor) -> PathBuf {
        take.folder.join(format!("{} PiP.mp4", take.stem))
    }

    /// Writes the finished video for `take` (replacing an earlier one, for Remake Video) and returns its path;
    /// fails when the take has no screen or camera recording. Blocks until the export finishes, calling
    /// `progress` with 0…1 on this thread about twice a second.
    pub fn make(
        take: &TakeDescriptor,
        settings: &VideoSettings,
        progress: impl FnMut(f64),
    ) -> Result<PathBuf, CaptureError> {
        let screen = take.capture("screen");
        let camera = take.capture("camera");
        if screen.is_none() && camera.is_none() {
            return Err(CaptureError::NothingRecorded("screen or camera".into()));
        }
        let tracks: Vec<&TakeFile> = take
            .files
            .iter()
            .filter(|f| f.role == TakeRole::Track)
            .collect();
        let audio = if tracks.len() >= 2 {
            tracks
        } else {
            take.main_audio().into_iter().collect()
        };
        let out = Self::output_path(take);
        export(screen, camera, &audio, &out, settings, progress)?;
        Ok(out)
    }
}

fn media(kind: Option<&'static AVMediaType>) -> &'static AVMediaType {
    kind.expect("AVFoundation media type constant")
}

fn zero() -> CMTime {
    unsafe { kCMTimeZero }
}

fn seconds(s: f64) -> CMTime {
    unsafe { CMTime::with_seconds(s, 600) }
}

fn greater(a: CMTime, b: CMTime) -> bool {
    unsafe { a.compare(b) > 0 }
}

/// Where a captured file sits on the take's timeline, which starts at the first audio sample: a file that
/// started early is trimmed at its head, one that started late is placed later, and none runs past the
/// audio.
#[derive(Clone, Copy, Debug)]
struct Placement {
    source_start: CMTime,
    at: CMTime,
    length: CMTime,
}

fn place(offset: f64, duration: CMTime, total: CMTime) -> Option<Placement> {
    let source_start = seconds((-offset).max(0.0));
    let at = seconds(offset.max(0.0));
    let mut length = unsafe { duration.subtract(source_start) };
    if greater(total, zero()) {
        length = unsafe { length.minimum(total.subtract(at)) };
    }
    greater(length, zero()).then_some(Placement {
        source_start,
        at,
        length,
    })
}

fn asset(path: &Path) -> Retained<AVURLAsset> {
    unsafe { AVURLAsset::URLAssetWithURL_options(&file_url(path), None) }
}

/// Loads `keys` and fails with the first one that didn't load.
fn load<T>(object: &T, keys: &[&str]) -> Result<(), CaptureError>
where
    T: AVAsynchronousKeyValueLoading + objc2::Message,
{
    let names: Vec<Retained<NSString>> = keys.iter().map(|k| ns(k)).collect();
    let array = NSArray::from_retained_slice(&names);
    wait(|tx: mpsc::Sender<()>| {
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        unsafe { object.loadValuesAsynchronouslyForKeys_completionHandler(&array, Some(&done)) };
    });
    for key in &names {
        let mut error: *mut NSError = std::ptr::null_mut();
        let status: AVKeyValueStatus =
            unsafe { msg_send![any(object), statusOfValueForKey: &**key, error: &mut error] };
        if status != AVKeyValueStatus::Loaded {
            let message = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
            return Err(CaptureError::System(
                message.unwrap_or_else(|| format!("couldn’t load {key}")),
            ));
        }
    }
    Ok(())
}

fn load_tracks(
    asset: &AVURLAsset,
    kind: &AVMediaType,
) -> Result<Vec<Retained<AVAssetTrack>>, CaptureError> {
    let result = wait(|tx| {
        let done = RcBlock::new(
            move |tracks: *mut NSArray<AVAssetTrack>, error: *mut NSError| {
                let result = match unsafe { tracks.as_ref() } {
                    Some(tracks) => Ok(Shared(tracks.to_vec())),
                    None => {
                        Err(unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string()))
                    }
                };
                let _ = tx.send(result);
            },
        );
        unsafe { asset.loadTracksWithMediaType_completionHandler(kind, &done) };
    });
    match result {
        Some(Ok(tracks)) => Ok(tracks.0),
        Some(Err(message)) => Err(CaptureError::System(
            message.unwrap_or_else(|| "couldn’t load tracks".into()),
        )),
        None => Err(CaptureError::System("couldn’t load tracks".into())),
    }
}

fn export(
    screen: Option<&TakeFile>,
    camera: Option<&TakeFile>,
    audio: &[&TakeFile],
    out: &Path,
    settings: &VideoSettings,
    mut progress: impl FnMut(f64),
) -> Result<(), CaptureError> {
    let composition = unsafe { AVMutableComposition::composition() };
    let mut total = zero();

    // The take's timeline starts at the first audio sample.
    for file in audio {
        let asset = asset(&file.path);
        let Some(track) = load_tracks(&asset, media(unsafe { AVMediaTypeAudio }))?
            .into_iter()
            .next()
        else {
            continue;
        };
        load(&*asset, &["duration"])?;
        let duration = unsafe { asset.duration() };
        let target = unsafe {
            composition.addMutableTrackWithMediaType_preferredTrackID(
                media(AVMediaTypeAudio),
                kCMPersistentTrackID_Invalid,
            )
        };
        if let Some(target) = target {
            unsafe {
                target.insertTimeRange_ofTrack_atTime_error(
                    CMTimeRange {
                        start: zero(),
                        duration,
                    },
                    &track,
                    zero(),
                )
            }?;
        }
        total = unsafe { total.maximum(duration) };
    }

    let mut add_video =
        |file: Option<&TakeFile>| -> Result<Option<(CMPersistentTrackID, CGSize)>, CaptureError> {
            let Some(file) = file else { return Ok(None) };
            let asset = asset(&file.path);
            let Some(track) = load_tracks(&asset, media(unsafe { AVMediaTypeVideo }))?
                .into_iter()
                .next()
            else {
                return Ok(None);
            };
            load(&*asset, &["duration"])?;
            load(&*track, &["naturalSize"])?;
            let duration = unsafe { asset.duration() };
            let size = unsafe { track.naturalSize() };
            let Some(p) = place(file.offset.unwrap_or(0.0), duration, total) else {
                return Ok(None);
            };
            let range = CMTimeRange {
                start: p.source_start,
                duration: p.length,
            };
            let target = unsafe {
                composition.addMutableTrackWithMediaType_preferredTrackID(
                    media(AVMediaTypeVideo),
                    kCMPersistentTrackID_Invalid,
                )
            };
            if let Some(target) = &target {
                unsafe { target.insertTimeRange_ofTrack_atTime_error(range, &track, p.at) }?;
            }
            // System audio, when it was recorded, rides in the screen file: keep it, lined up the same way.
            for sound in load_tracks(&asset, media(unsafe { AVMediaTypeAudio }))? {
                let lane = unsafe {
                    composition.addMutableTrackWithMediaType_preferredTrackID(
                        media(AVMediaTypeAudio),
                        kCMPersistentTrackID_Invalid,
                    )
                };
                if let Some(lane) = lane {
                    unsafe { lane.insertTimeRange_ofTrack_atTime_error(range, &sound, p.at) }?;
                }
            }
            if !greater(total, zero()) {
                total = unsafe { p.at.add(p.length) };
            }
            Ok(target.map(|t| (unsafe { t.trackID() }, size)))
        };
    let screen_track = add_video(screen)?;
    let camera_track = add_video(camera)?;
    let Some((_, base)) = screen_track.or(camera_track) else {
        return Err(CaptureError::NothingRecorded("video".into()));
    };

    // Output size: the screen's aspect (or the camera's), at most 1920 wide.
    let (width, height) = layout::render_size(base.width, base.height);
    let instruction = PipInstruction::new(InstructionIvars {
        time_range: CMTimeRange {
            start: zero(),
            duration: total,
        },
        screen: screen_track.map(|t| t.0),
        camera: camera_track.map(|t| t.0),
        layout: PipLayout::new(settings),
    });
    let video = unsafe { AVMutableVideoComposition::videoComposition() };
    unsafe {
        video.setCustomVideoCompositorClass(Some(PipCompositor::class()));
        video.setRenderSize(CGSize::new(width, height));
        video.setFrameDuration(CMTime::new(1, 30));
        video.setInstructions(&NSArray::from_retained_slice(&[
            ProtocolObject::from_retained(instruction),
        ]));
    }

    let _ = std::fs::remove_file(out);
    let preset = unsafe {
        if settings.hevc() {
            AVAssetExportPresetHEVCHighestQuality
        } else {
            AVAssetExportPresetHighestQuality
        }
    };
    let session = unsafe {
        AVAssetExportSession::initWithAsset_presetName(
            AVAssetExportSession::alloc(),
            &composition,
            preset,
        )
    }
    .ok_or_else(|| CaptureError::Writer(format!("no export session for {preset}")))?;
    unsafe {
        session.setVideoComposition(Some(&video));
        session.setOutputURL(Some(&file_url(out)));
        session.setOutputFileType(AVFileTypeMPEG4);
    }
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(move || {
        let _ = tx.send(());
    });
    unsafe { session.exportAsynchronouslyWithCompletionHandler(&done) };
    while let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(Duration::from_millis(500)) {
        if unsafe { session.status() } == AVAssetExportSessionStatus::Exporting {
            progress(unsafe { session.progress() } as f64);
        }
    }
    if unsafe { session.status() } == AVAssetExportSessionStatus::Completed {
        Ok(())
    } else {
        let message = unsafe { session.error() }.map(|e| e.localizedDescription().to_string());
        Err(CaptureError::System(
            message.unwrap_or_else(|| "The export failed.".into()),
        ))
    }
}

// MARK: The instruction and the compositor

struct InstructionIvars {
    time_range: CMTimeRange,
    screen: Option<CMPersistentTrackID>,
    camera: Option<CMPersistentTrackID>,
    layout: PipLayout,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and PipInstruction has no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "LavdeckPiPInstruction"]
    #[ivars = InstructionIvars]
    struct PipInstruction;

    unsafe impl NSObjectProtocol for PipInstruction {}

    unsafe impl AVVideoCompositionInstructionProtocol for PipInstruction {
        #[unsafe(method(timeRange))]
        fn time_range(&self) -> CMTimeRange {
            self.ivars().time_range
        }

        #[unsafe(method(enablePostProcessing))]
        fn enable_post_processing(&self) -> bool {
            false
        }

        #[unsafe(method(containsTweening))]
        fn contains_tweening(&self) -> bool {
            true
        }

        #[unsafe(method_id(requiredSourceTrackIDs))]
        fn required_source_track_ids(&self) -> Option<Retained<NSArray<NSValue>>> {
            let ivars = self.ivars();
            let ids: Vec<Retained<NSValue>> = [ivars.screen, ivars.camera]
                .into_iter()
                .flatten()
                .map(|id| NSNumber::new_i32(id).into_super())
                .collect();
            Some(NSArray::from_retained_slice(&ids))
        }

        #[unsafe(method(passthroughTrackID))]
        fn passthrough_track_id(&self) -> CMPersistentTrackID {
            kCMPersistentTrackID_Invalid
        }
    }
);

// SAFETY: the ivars are plain data, set once before AVFoundation sees the instruction.
unsafe impl Send for PipInstruction {}
// SAFETY: as above.
unsafe impl Sync for PipInstruction {}

impl PipInstruction {
    fn new(ivars: InstructionIvars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }
}

struct CompositorIvars {
    context: Retained<CIContext>,
    color_space: Shared<CFRetained<CGColorSpace>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and PipCompositor has no Drop impl. AVFoundation
    // creates it with -init, which sets the ivars.
    #[unsafe(super(NSObject))]
    #[name = "LavdeckPiPCompositor"]
    #[ivars = CompositorIvars]
    struct PipCompositor;

    impl PipCompositor {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let options = dictionary(&[(unsafe { kCIContextCacheIntermediates }, any(&*NSNumber::new_bool(false)))]);
            let context = unsafe { CIContext::contextWithOptions(Some(&options)) };
            let color_space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).expect("sRGB color space");
            let this = this.set_ivars(CompositorIvars { context, color_space: Shared(color_space) });
            unsafe { msg_send![super(this), init] }
        }
    }

    unsafe impl NSObjectProtocol for PipCompositor {}

    unsafe impl AVVideoCompositing for PipCompositor {
        #[unsafe(method_id(sourcePixelBufferAttributes))]
        fn source_pixel_buffer_attributes(&self) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
            Some(bgra())
        }

        #[unsafe(method_id(requiredPixelBufferAttributesForRenderContext))]
        fn required_pixel_buffer_attributes(&self) -> Retained<NSDictionary<NSString, AnyObject>> {
            bgra()
        }

        #[unsafe(method(renderContextChanged:))]
        fn render_context_changed(&self, _context: &AVVideoCompositionRenderContext) {}

        #[unsafe(method(startVideoCompositionRequest:))]
        fn start_video_composition_request(&self, request: &AVAsynchronousVideoCompositionRequest) {
            self.render(request);
        }
    }
);

// SAFETY: CIContext is thread-safe, and the color space is immutable.
unsafe impl Send for PipCompositor {}
// SAFETY: as above.
unsafe impl Sync for PipCompositor {}

fn bgra() -> Retained<NSDictionary<NSString, AnyObject>> {
    let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    dictionary(&[(
        cf_ns(unsafe { kCVPixelBufferPixelFormatTypeKey }),
        any(&*format),
    )])
}

fn cg_rect(r: Rect) -> CGRect {
    CGRect::new(CGPoint::new(r.x, r.y), CGSize::new(r.w, r.h))
}

fn scale(image: &CIImage, s: f64) -> Retained<CIImage> {
    let t = CGAffineTransform {
        a: s,
        b: 0.0,
        c: 0.0,
        d: s,
        tx: 0.0,
        ty: 0.0,
    };
    unsafe { image.imageByApplyingTransform(t) }
}

fn translate(image: &CIImage, x: f64, y: f64) -> Retained<CIImage> {
    let t = CGAffineTransform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: x,
        ty: y,
    };
    unsafe { image.imageByApplyingTransform(t) }
}

fn extent(image: &CIImage) -> Rect {
    let e = unsafe { image.extent() };
    Rect::new(e.origin.x, e.origin.y, e.size.width, e.size.height)
}

fn rounded_rect(rect: Rect, radius: f64, color: &CIColor) -> Option<Retained<CIImage>> {
    let extent = unsafe { CIVector::vectorWithCGRect(cg_rect(rect)) };
    let radius = number(radius);
    let params = dictionary(&[
        (&ns("inputExtent"), any(&*extent)),
        (&ns("inputRadius"), any(&*radius)),
        (&ns("inputColor"), any(color)),
    ]);
    let filter = unsafe {
        CIFilter::filterWithName_withInputParameters(
            &ns("CIRoundedRectangleGenerator"),
            Some(&params),
        )
    }?;
    unsafe { filter.outputImage() }
}

impl PipCompositor {
    /// Draws the screen full-frame with the camera in a bubble (or the camera full-frame when there is no
    /// screen recording).
    fn render(&self, request: &AVAsynchronousVideoCompositionRequest) {
        let instruction = unsafe { request.videoCompositionInstruction() };
        let instruction =
            AsRef::<AnyObject>::as_ref(&*instruction).downcast_ref::<PipInstruction>();
        let context = unsafe { request.renderContext() };
        let (Some(instruction), Some(output)) = (instruction, unsafe { context.newPixelBuffer() })
        else {
            let error = unsafe { NSError::errorWithDomain_code_userInfo(&ns("PiP"), 1, None) };
            unsafe { request.finishWithError(&error) };
            return;
        };
        let size = unsafe { context.size() };
        let frame = Rect::new(0.0, 0.0, size.width, size.height);
        let mut image = unsafe {
            CIImage::imageWithColor(&CIColor::blackColor()).imageByCroppingToRect(cg_rect(frame))
        };

        let ivars = instruction.ivars();
        let source = |id: Option<CMPersistentTrackID>| {
            id.and_then(|id| unsafe { request.sourceFrameByTrackID(id) })
                .map(|buffer| unsafe { CIImage::imageWithCVPixelBuffer(&buffer) })
        };
        let screen = source(ivars.screen);
        let camera = source(ivars.camera);

        if let Some(screen) = screen {
            image = unsafe { fit(&screen, frame).imageByCompositingOverImage(&image) };
            if let Some(camera) = camera
                && let Some(bubble) = bubble(&camera, &ivars.layout, size.width, size.height)
            {
                image = unsafe { bubble.imageByCompositingOverImage(&image) };
            }
        } else if let Some(camera) = camera {
            image = unsafe { fit(&camera, frame).imageByCompositingOverImage(&image) };
        }
        let ivars = self.ivars();
        unsafe {
            ivars.context.render_toCVPixelBuffer_bounds_colorSpace(
                &image,
                &output,
                cg_rect(frame),
                Some(&ivars.color_space),
            );
            request.finishWithComposedVideoFrame(&output);
        }
    }
}

/// Scales an image to fit the frame, centred.
fn fit(image: &CIImage, frame: Rect) -> Retained<CIImage> {
    let scaled = scale(image, layout::fit_scale(extent(image), frame.w, frame.h));
    let (dx, dy) = layout::centre(extent(&scaled), frame.w, frame.h);
    translate(&scaled, dx, dy)
}

fn bubble(
    camera: &CIImage,
    layout: &PipLayout,
    width: f64,
    height: f64,
) -> Option<Retained<CIImage>> {
    let b = layout::bubble(extent(camera), layout, width, height);
    let face = unsafe { camera.imageByCroppingToRect(cg_rect(b.crop)) };
    let face = translate(
        &scale(&translate(&face, -b.crop.x, -b.crop.y), b.scale),
        b.rect.x,
        b.rect.y,
    );
    let mask = rounded_rect(b.rect, b.radius, &*unsafe { CIColor::whiteColor() })?;
    let ring = rounded_rect(b.ring, b.ring_radius, &*unsafe {
        CIColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, 0.9)
    })?;
    let empty = unsafe { CIImage::emptyImage() };
    let params = dictionary(&[
        (unsafe { kCIInputBackgroundImageKey }, any(&*empty)),
        (unsafe { kCIInputMaskImageKey }, any(&*mask)),
    ]);
    let masked =
        unsafe { face.imageByApplyingFilter_withInputParameters(&ns("CIBlendWithMask"), &params) };
    Some(unsafe { masked.imageByCompositingOverImage(&ring) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(t: CMTime) -> f64 {
        unsafe { t.seconds() }
    }

    #[test]
    fn video_that_started_early_is_trimmed_at_its_head() {
        // The camera started 0.8 s before the audio and ran 10 s; the audio is 9 s long.
        let p = place(-0.8, seconds(10.0), seconds(9.0)).unwrap();
        assert!((secs(p.source_start) - 0.8).abs() < 1e-3);
        assert_eq!(secs(p.at), 0.0);
        assert!((secs(p.length) - 9.0).abs() < 1e-3);
    }

    #[test]
    fn video_that_started_late_is_placed_later_and_cut_at_the_audio_end() {
        let p = place(0.5, seconds(10.0), seconds(9.0)).unwrap();
        assert_eq!(secs(p.source_start), 0.0);
        assert!((secs(p.at) - 0.5).abs() < 1e-3);
        assert!((secs(p.length) - 8.5).abs() < 1e-3);
        // Shorter than the audio: keeps its own length.
        let p = place(0.5, seconds(3.0), seconds(9.0)).unwrap();
        assert!((secs(p.length) - 3.0).abs() < 1e-3);
    }

    #[test]
    fn without_audio_the_video_sets_the_length() {
        let p = place(-0.25, seconds(4.0), zero()).unwrap();
        assert!((secs(p.length) - 3.75).abs() < 1e-3);
        // No offset recorded (an older take): lined up at the start.
        let p = place(0.0, seconds(4.0), seconds(5.0)).unwrap();
        assert_eq!(
            (secs(p.source_start), secs(p.at), secs(p.length)),
            (0.0, 0.0, 4.0)
        );
    }

    #[test]
    fn video_entirely_outside_the_audio_is_dropped() {
        assert!(place(-5.0, seconds(4.0), seconds(9.0)).is_none());
        assert!(place(9.5, seconds(4.0), seconds(9.0)).is_none());
    }
}
