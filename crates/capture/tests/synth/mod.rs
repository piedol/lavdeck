//! Synthetic takes for the composer: a screen-like and a camera-like clip written with AVAssetWriter from
//! generated frames, a stereo WAV, and a pixel reader for the finished video. No camera, screen or
//! permission involved.

#![allow(dead_code)]

use std::path::Path;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetImageGenerator, AVAssetTrack, AVAssetWriter, AVAssetWriterInput,
    AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus, AVAsynchronousKeyValueLoading,
    AVFileTypeQuickTimeMovie, AVMediaType, AVMediaTypeAudio, AVMediaTypeVideo, AVURLAsset,
    AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoCodecTypeHEVC, AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextGetData, CGColorSpace, CGContext, CGImage,
    CGImageAlphaInfo,
};
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString, NSURL};

pub fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn any<T: objc2::Message>(o: &T) -> &AnyObject {
    unsafe { &*(o as *const T).cast::<AnyObject>() }
}

/// Writes `frames` BGRA frames of `width`x`height` at 30 fps to a .mov; `paint(frame, x, y)` gives each
/// pixel's (b, g, r).
pub fn write_clip(
    path: &Path,
    width: usize,
    height: usize,
    frames: usize,
    hevc: bool,
    paint: impl Fn(usize, usize, usize) -> (u8, u8, u8),
) {
    let _ = std::fs::remove_file(path);
    unsafe {
        let writer = AVAssetWriter::assetWriterWithURL_fileType_error(
            &url(path),
            AVFileTypeQuickTimeMovie.unwrap(),
        )
        .unwrap();
        let codec = if hevc {
            AVVideoCodecTypeHEVC
        } else {
            AVVideoCodecTypeH264
        }
        .unwrap();
        let (w, h) = (NSNumber::new_usize(width), NSNumber::new_usize(height));
        let keys = [
            AVVideoCodecKey.unwrap(),
            AVVideoWidthKey.unwrap(),
            AVVideoHeightKey.unwrap(),
        ];
        let values: [&AnyObject; 3] = [any(codec), any(&*w), any(&*h)];
        let settings = NSDictionary::from_slices(&keys, &values);
        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            AVMediaTypeVideo.unwrap(),
            Some(&settings),
        );
        input.setExpectsMediaDataInRealTime(false);
        let adaptor =
            AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                &input, None,
            );
        writer.addInput(&input);
        assert!(writer.startWriting(), "{:?}", writer.error());
        writer.startSessionAtSourceTime(CMTime::new(0, 30));
        for frame in 0..frames {
            while !input.isReadyForMoreMediaData() {
                std::thread::sleep(Duration::from_millis(2));
            }
            let buffer = pixel_buffer(width, height, |x, y| paint(frame, x, y));
            assert!(
                adaptor
                    .appendPixelBuffer_withPresentationTime(&buffer, CMTime::new(frame as i64, 30))
            );
        }
        input.markAsFinished();
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        writer.finishWritingWithCompletionHandler(&done);
        rx.recv().unwrap();
        assert_eq!(
            writer.status(),
            AVAssetWriterStatus::Completed,
            "{:?}",
            writer.error()
        );
    }
}

fn pixel_buffer(
    width: usize,
    height: usize,
    paint: impl Fn(usize, usize) -> (u8, u8, u8),
) -> CFRetained<CVPixelBuffer> {
    let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
    let status = unsafe {
        CVPixelBufferCreate(
            None,
            width,
            height,
            kCVPixelFormatType_32BGRA,
            None,
            NonNull::from(&mut out),
        )
    };
    assert_eq!(status, 0);
    let buffer = unsafe { CFRetained::from_raw(NonNull::new(out).unwrap()) };
    unsafe {
        CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
        let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
        let stride = CVPixelBufferGetBytesPerRow(&buffer);
        for y in 0..height {
            let row = std::slice::from_raw_parts_mut(base.add(y * stride), width * 4);
            for x in 0..width {
                let (b, g, r) = paint(x, y);
                row[x * 4..x * 4 + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
    }
    buffer
}

/// A 16-bit stereo 48 kHz WAV of `seconds`: 440 Hz left, 660 Hz right.
pub fn write_wav(path: &Path, seconds: f64) {
    let rate = 48_000u32;
    let frames = (seconds * rate as f64) as u32;
    let mut data = Vec::with_capacity(frames as usize * 4);
    for i in 0..frames {
        let t = i as f64 / rate as f64;
        for f in [440.0, 660.0] {
            let v = ((t * f * std::f64::consts::TAU).sin() * 8000.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav).unwrap();
}

/// A movie's duration, video size and track counts (video, audio).
pub struct Info {
    pub duration: f64,
    pub size: (f64, f64),
    pub video_tracks: usize,
    pub audio_tracks: usize,
    pub frame_rate: f32,
}

fn tracks(asset: &AVURLAsset, kind: &AVMediaType) -> Vec<Retained<AVAssetTrack>> {
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(
        move |tracks: *mut NSArray<AVAssetTrack>, _e: *mut NSError| {
            let list = unsafe { tracks.as_ref() }
                .map(|t| t.to_vec())
                .unwrap_or_default();
            let _ = tx.send(Wrap(list));
        },
    );
    unsafe { asset.loadTracksWithMediaType_completionHandler(kind, &done) };
    rx.recv().unwrap().0
}

struct Wrap<T>(T);
unsafe impl<T> Send for Wrap<T> {}

fn load(object: &(impl AVAsynchronousKeyValueLoading + objc2::Message), keys: &[&str]) {
    let names: Vec<Retained<NSString>> = keys.iter().map(|k| NSString::from_str(k)).collect();
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(move || {
        let _ = tx.send(());
    });
    unsafe {
        object.loadValuesAsynchronouslyForKeys_completionHandler(
            &NSArray::from_retained_slice(&names),
            Some(&done),
        )
    };
    rx.recv().unwrap();
}

pub fn info(path: &Path) -> Info {
    let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url(path), None) };
    load(&*asset, &["duration"]);
    let video = tracks(&asset, unsafe { AVMediaTypeVideo }.unwrap());
    let audio = tracks(&asset, unsafe { AVMediaTypeAudio }.unwrap());
    let (size, frame_rate) = video
        .first()
        .map(|t| {
            load(&**t, &["naturalSize", "nominalFrameRate"]);
            let s = unsafe { t.naturalSize() };
            ((s.width, s.height), unsafe { t.nominalFrameRate() })
        })
        .unwrap_or(((0.0, 0.0), 0.0));
    Info {
        duration: unsafe { asset.duration().seconds() },
        size,
        video_tracks: video.len(),
        audio_tracks: audio.len(),
        frame_rate,
    }
}

/// The RGBA pixels of the frame at `seconds`, top row first.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// (r, g, b) at `x`, `y` from the top left.
    pub fn at(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let i = (y * self.width + x) * 4;
        (self.rgba[i], self.rgba[i + 1], self.rgba[i + 2])
    }
}

pub fn frame(path: &Path, seconds: f64) -> Frame {
    let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url(path), None) };
    let generator = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset) };
    unsafe {
        let zero = CMTime::new(0, 600);
        generator.setRequestedTimeToleranceBefore(zero);
        generator.setRequestedTimeToleranceAfter(zero);
    }
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(
        move |image: *mut CGImage, _t: CMTime, error: *mut NSError| {
            let image =
                unsafe { image.as_ref() }.map(|i| Wrap(unsafe { CFRetained::retain(i.into()) }));
            let error = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
            let _ = tx.send((image, error));
        },
    );
    unsafe {
        generator.generateCGImageAsynchronouslyForTime_completionHandler(
            CMTime::with_seconds(seconds, 600),
            &done,
        )
    };
    let (image, error) = rx.recv().unwrap();
    let image = image
        .unwrap_or_else(|| panic!("no frame at {seconds}: {error:?}"))
        .0;
    let (width, height) = (CGImage::width(Some(&image)), CGImage::height(Some(&image)));
    let space =
        CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB })).unwrap();
    let context: CFRetained<CGContext> = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            width,
            height,
            8,
            width * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }
    .unwrap();
    CGContext::draw_image(
        Some(&context),
        CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        ),
        Some(&image),
    );
    let data = CGBitmapContextGetData(Some(&context)).cast::<u8>();
    let rgba = unsafe { std::slice::from_raw_parts(data, width * height * 4) }.to_vec();
    Frame {
        width,
        height,
        rgba,
    }
}
