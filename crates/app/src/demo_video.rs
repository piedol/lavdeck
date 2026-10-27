//! The demo's stand-in screen and camera recordings: a 75-second 640 × 360 H.264 movie (a
//! slow colour wash with a moving bar, two frames a second), written once with AVAssetWriter
//! so the demo's video takes play in the real player.

use std::path::Path;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVFileTypeMPEG4,
    AVFileTypeQuickTimeMovie, AVMediaTypeVideo, AVVideoCodecKey, AVVideoCodecTypeH264,
    AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_foundation::CFRetained;
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

const W: usize = 640;
const H: usize = 360;
const FPS: i64 = 2;

/// Writes the movie to `path` (MPEG-4 for `.mp4`, else QuickTime). Blocks; run off the main
/// thread.
pub fn write(path: &Path, seconds: f64) -> Result<(), String> {
    let mp4 = path.extension().is_some_and(|e| e == "mp4");
    let url = crate::util::file_url(path);
    // SAFETY: AVFoundation writer calls with valid settings, on a single thread, in the
    // documented order (add input, start writing, start session, append, finish).
    unsafe {
        let ty = if mp4 {
            AVFileTypeMPEG4
        } else {
            AVFileTypeQuickTimeMovie
        }
        .ok_or("no file type")?;
        let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&url, ty)
            .map_err(|e| e.localizedDescription().to_string())?;
        let keys: [&NSString; 3] = [
            AVVideoCodecKey.ok_or("no codec key")?,
            AVVideoWidthKey.ok_or("no width key")?,
            AVVideoHeightKey.ok_or("no height key")?,
        ];
        let width = NSNumber::new_usize(W);
        let height = NSNumber::new_usize(H);
        let codec: &AnyObject = AVVideoCodecTypeH264.ok_or("no H.264")?;
        let values: [&AnyObject; 3] = [codec, &width, &height];
        let settings = NSDictionary::from_slices(&keys, &values);
        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            AVMediaTypeVideo.ok_or("no video type")?,
            Some(&settings),
        );
        input.setExpectsMediaDataInRealTime(false);
        let adaptor = AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(&input, None);
        if !writer.canAddInput(&input) {
            return Err("can't add the video input".into());
        }
        writer.addInput(&input);
        if !writer.startWriting() {
            return Err("can't start writing".into());
        }
        writer.startSessionAtSourceTime(CMTime::new(0, 1));
        let frames = (seconds * FPS as f64) as i64;
        for i in 0..frames {
            let mut waited = 0;
            while !input.isReadyForMoreMediaData() && waited < 2000 {
                std::thread::sleep(Duration::from_millis(2));
                waited += 1;
            }
            let buffer = frame(i as f64 / FPS as f64).ok_or("no pixel buffer")?;
            if !adaptor.appendPixelBuffer_withPresentationTime(&buffer, CMTime::new(i, FPS as i32))
            {
                return Err("couldn't append a frame".into());
            }
        }
        input.markAsFinished();
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        writer.finishWritingWithCompletionHandler(&done);
        rx.recv_timeout(Duration::from_secs(20))
            .map_err(|_| "the writer didn't finish".to_string())
    }
}

/// One frame at `t` seconds.
fn frame(t: f64) -> Option<CFRetained<CVPixelBuffer>> {
    let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
    // SAFETY: a new BGRA buffer; `out` receives a +1 reference we take over below.
    let status = unsafe {
        CVPixelBufferCreate(
            None,
            W,
            H,
            kCVPixelFormatType_32BGRA,
            None,
            NonNull::from(&mut out),
        )
    };
    let buffer = NonNull::new(out).filter(|_| status == 0)?;
    // SAFETY: CVPixelBufferCreate returned a +1 reference.
    let buffer = unsafe { CFRetained::from_raw(buffer) };
    // SAFETY: the buffer is locked while its memory is written, within its bytes-per-row.
    unsafe {
        CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
        let base = CVPixelBufferGetBaseAddress(&buffer) as *mut u8;
        let row = CVPixelBufferGetBytesPerRow(&buffer);
        let bar = ((t / 75.0) * W as f64) as usize;
        let hue = (t / 75.0 * 255.0) as u8;
        for y in 0..H {
            let line = std::slice::from_raw_parts_mut(base.add(y * row), W * 4);
            for x in 0..W {
                let px = &mut line[x * 4..x * 4 + 4];
                let shade = (40 + (y * 60 / H)) as u8;
                if x.abs_diff(bar) < 6 {
                    px.copy_from_slice(&[240, 240, 240, 255]);
                } else {
                    px.copy_from_slice(&[
                        shade.saturating_add(hue / 3),
                        shade,
                        shade.saturating_add(40 - hue / 8),
                        255,
                    ]);
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
    }
    Some(buffer)
}
