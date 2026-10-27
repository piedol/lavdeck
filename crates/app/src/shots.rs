//! Drawing views and windows to PNG without screen recording permission: AppKit draws them
//! into a 2x sRGB bitmap (`cacheDisplayInRect:toBitmapImageRep:`), as Kipimo's shots do.

use std::path::Path;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSBitmapImageFileType, NSBitmapImageRep, NSColorSpace, NSDeviceRGBColorSpace,
    NSView, NSWindow,
};
use objc2_foundation::{NSDictionary, NSSize};

/// The appearance for a snapshot suffix: `light`, `dark`, `hc-light`, `hc-dark`.
pub fn appearance(suffix: &str) -> Option<Retained<NSAppearance>> {
    // SAFETY: AppKit's appearance name constants.
    let name = unsafe {
        match suffix {
            "dark" => objc2_app_kit::NSAppearanceNameDarkAqua,
            "hc-light" => objc2_app_kit::NSAppearanceNameAccessibilityHighContrastAqua,
            "hc-dark" => objc2_app_kit::NSAppearanceNameAccessibilityHighContrastDarkAqua,
            _ => objc2_app_kit::NSAppearanceNameAqua,
        }
    };
    NSAppearance::appearanceNamed(name)
}

/// Draws `view` at `scale` into PNG data.
pub fn png(view: &NSView, scale: f64) -> Option<Vec<u8>> {
    let b = view.bounds();
    let (w, h) = (
        (b.size.width * scale).round() as isize,
        (b.size.height * scale).round() as isize,
    );
    if w <= 0 || h <= 0 {
        return None;
    }
    // SAFETY: a standard 8-bit RGBA bitmap that allocates its own memory.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w,
            h,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    // Tagged sRGB, so the design's sRGB values land unchanged.
    let rep = rep.bitmapImageRepByRetaggingWithColorSpace(&NSColorSpace::sRGBColorSpace())?;
    rep.setSize(NSSize::new(b.size.width, b.size.height));
    view.cacheDisplayInRect_toBitmapImageRep(b, &rep);
    let props = NSDictionary::new();
    // SAFETY: PNG with no properties is a valid request.
    let data =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    Some(data.to_vec())
}

pub fn write(view: &NSView, path: &Path) {
    if let Some(data) = png(view, 2.0) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, data);
    }
}

/// Draws a window, title bar and toolbar included.
pub fn write_window(window: &NSWindow, path: &Path) {
    // SAFETY: the content view's superview is the window's frame view, owned by the window.
    let Some(frame_view) = window.contentView().and_then(|v| unsafe { v.superview() }) else {
        return;
    };
    frame_view.displayIfNeeded();
    write(&frame_view, path);
}
