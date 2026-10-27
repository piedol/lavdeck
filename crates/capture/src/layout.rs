//! The geometry behind the recordings and the finished video, kept apart from AppKit and Core Image so
//! it can be tested: encoder sizes and bit rates, the camera format choice, the composed frame (the screen
//! fitted, the camera bubble in its corner) and the preview window's frame. Core Image and AppKit both put
//! the origin at the bottom left.

use crate::settings::{BubbleShape, BubbleSize, Corner, PreviewSize, VideoSettings};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    pub fn max_x(&self) -> f64 {
        self.x + self.w
    }

    pub fn max_y(&self) -> f64 {
        self.y + self.h
    }

    pub fn mid_x(&self) -> f64 {
        self.x + self.w / 2.0
    }

    pub fn mid_y(&self) -> f64 {
        self.y + self.h / 2.0
    }

    /// `CGRect.insetBy(dx:dy:)`; negative values grow the rect.
    pub fn inset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(
            self.x + dx,
            self.y + dy,
            self.w - 2.0 * dx,
            self.h - 2.0 * dy,
        )
    }

    /// `CGRect.contains(_:)`: the minimum edges are inside, the maximum edges aren't.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.max_x() && y >= self.y && y < self.max_y()
    }
}

// MARK: Recording

/// Longest side the screen recording is encoded at.
pub const MAX_SCREEN_WIDTH: i64 = 3840;

/// The screen recording's pixel size: the display's points times its scale, at most 3840 wide, even.
pub fn screen_pixels(points_w: f64, points_h: f64, scale: f64) -> (i64, i64) {
    let mut width = (points_w * scale) as i64;
    let mut height = (points_h * scale) as i64;
    if width > MAX_SCREEN_WIDTH {
        height = (height as f64 * MAX_SCREEN_WIDTH as f64 / width as f64) as i64;
        width = MAX_SCREEN_WIDTH;
    }
    (width - width % 2, height - height % 2)
}

/// Average bit rate for the screen recording.
pub fn screen_bit_rate(width: i64, height: i64, fps: u32, hevc: bool) -> i64 {
    let bits_per_pixel = if hevc { 0.06 } else { 0.1 };
    ((width * height * fps as i64) as f64 * bits_per_pixel) as i64
}

/// Average bit rate for the camera. Webcam video is noisy; too few bits turn it into mush. About 7.5 Mbps
/// HEVC or 12 Mbps H.264 at 1080p30.
pub fn camera_bit_rate(width: i64, height: i64, frame_rate: f64, hevc: bool) -> i64 {
    let bits_per_pixel = if hevc { 0.12 } else { 0.2 };
    ((width * height) as f64 * frame_rate * bits_per_pixel) as i64
}

/// A camera format: its dimensions and its highest frame rate (none when it lists no frame rates).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraFormat {
    pub width: i32,
    pub height: i32,
    pub max_frame_rate: Option<f64>,
}

/// The camera's largest landscape format up to 3840 wide, then its highest frame rate. Like Swift's
/// `max(by:)`, the first of equal formats wins.
pub fn best_camera_format(formats: &[CameraFormat]) -> Option<usize> {
    let score = |f: &CameraFormat| {
        (
            f.width.wrapping_mul(f.height),
            f.max_frame_rate.unwrap_or(0.0),
        )
    };
    let mut best: Option<usize> = None;
    for (i, f) in formats.iter().enumerate() {
        if f.width < f.height || f.width > 3840 {
            continue;
        }
        match best {
            Some(b) if score(&formats[b]) >= score(f) => {}
            _ => best = Some(i),
        }
    }
    best
}

/// The frame rate the camera is locked to for a format: its highest, at most 60.
pub fn camera_frame_rate(format: &CameraFormat) -> f64 {
    format.max_frame_rate.unwrap_or(30.0).min(60.0)
}

// MARK: The finished video

/// Longest side of the finished video.
pub const MAX_OUTPUT_WIDTH: f64 = 1920.0;

/// The finished video's size: the screen's aspect (or the camera's), at most 1920 wide, even.
pub fn render_size(base_w: f64, base_h: f64) -> (f64, f64) {
    let width = MAX_OUTPUT_WIDTH.min(base_w);
    let (w, h) = (width, (base_h * width / base_w).round());
    (w - w % 2.0, h - h % 2.0)
}

/// Where the camera goes in the finished video.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PipLayout {
    pub corner: Corner,
    /// Bubble size as a fraction of the output height.
    pub fraction: f64,
    pub circle: bool,
}

impl PipLayout {
    pub fn new(settings: &VideoSettings) -> Self {
        PipLayout {
            corner: settings.bubble_corner,
            fraction: match settings.bubble_size {
                BubbleSize::Small => 0.2,
                BubbleSize::Medium => 0.28,
                BubbleSize::Large => 0.38,
            },
            circle: settings.bubble_shape == BubbleShape::Circle,
        }
    }
}

/// The scale that fits an image's extent in a frame.
pub fn fit_scale(extent: Rect, frame_w: f64, frame_h: f64) -> f64 {
    (frame_w / extent.w).min(frame_h / extent.h)
}

/// The translation that centres a scaled image in a frame. Takes the extent Core Image reports after
/// scaling, which it rounds out to whole pixels (640x480 scaled by 5/3 is 1067 wide, not 1066.67), so the
/// image lands where the Swift composer put it.
pub fn centre(scaled: Rect, frame_w: f64, frame_h: f64) -> (f64, f64) {
    (
        (frame_w - scaled.w) / 2.0 - scaled.x,
        (frame_h - scaled.h) / 2.0 - scaled.y,
    )
}

/// The camera bubble in the finished video.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bubble {
    /// The centred square cut from the camera frame.
    pub crop: Rect,
    /// Scale from the crop to the bubble.
    pub scale: f64,
    /// The bubble in the output frame.
    pub rect: Rect,
    pub radius: f64,
    /// The white ring drawn behind it, 3 px wider all round.
    pub ring: Rect,
    pub ring_radius: f64,
}

pub fn bubble(camera: Rect, layout: &PipLayout, out_w: f64, out_h: f64) -> Bubble {
    let diameter = (out_h * layout.fraction).round();
    let margin = (out_h * 0.035).round();
    // Centre-crop to a square, then scale to the bubble.
    let side = camera.w.min(camera.h);
    let crop = Rect::new(
        camera.mid_x() - side / 2.0,
        camera.mid_y() - side / 2.0,
        side,
        side,
    );
    let x = if layout.corner.is_left() {
        margin
    } else {
        out_w - diameter - margin
    };
    let y = if layout.corner.is_top() {
        out_h - diameter - margin
    } else {
        margin
    };
    let rect = Rect::new(x, y, diameter, diameter);
    let radius = if layout.circle {
        diameter / 2.0
    } else {
        diameter * 0.18
    };
    Bubble {
        crop,
        scale: diameter / side,
        rect,
        radius,
        ring: rect.inset(-3.0, -3.0),
        ring_radius: radius + 3.0,
    }
}

// MARK: The preview window

/// The round preview's diameter, in points.
pub const PREVIEW_DIAMETER: f64 = 180.0;

/// The preview window's frame at `size` on a screen's visible frame. The bubble returns to where it was
/// last left when that's still near the screen.
pub fn preview_frame(size: PreviewSize, screen: Rect, saved_origin: Option<(f64, f64)>) -> Rect {
    let d = PREVIEW_DIAMETER;
    match size {
        PreviewSize::Bubble => {
            let mut origin = (screen.max_x() - d - 32.0, screen.y + 32.0);
            if let Some(p) = saved_origin
                && screen.inset(-d, -d).contains(p.0, p.1)
            {
                origin = p;
            }
            Rect::new(origin.0, origin.1, d, d)
        }
        PreviewSize::Large => {
            let width = 720f64.min(screen.w * 0.45);
            let height = (width * 9.0 / 16.0).round();
            Rect::new(
                screen.max_x() - width - 32.0,
                screen.y + 32.0,
                width,
                height,
            )
        }
        PreviewSize::Full => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(corner: Corner, size: BubbleSize, shape: BubbleShape) -> PipLayout {
        PipLayout::new(&VideoSettings {
            bubble_corner: corner,
            bubble_size: size,
            bubble_shape: shape,
            ..VideoSettings::default()
        })
    }

    #[test]
    fn screen_size_is_even_and_capped() {
        // A 1512x982 point Retina display at 2x.
        assert_eq!(screen_pixels(1512.0, 982.0, 2.0), (3024, 1964));
        // A 5K display: 2560x1440 at 2x is 5120 wide, scaled to 3840.
        assert_eq!(screen_pixels(2560.0, 1440.0, 2.0), (3840, 2160));
        // Odd sizes lose a pixel.
        assert_eq!(screen_pixels(1001.0, 701.0, 1.0), (1000, 700));
        // 3440x1440 ultrawide at 2x: 6880 → 3840, 2880 * 3840 / 6880 = 1607.4 → 1607 → 1606.
        assert_eq!(screen_pixels(3440.0, 1440.0, 2.0), (3840, 1606));
    }

    #[test]
    fn bit_rates() {
        assert_eq!(screen_bit_rate(1920, 1080, 30, true), 3_732_480);
        assert_eq!(screen_bit_rate(1920, 1080, 60, false), 12_441_600);
        assert_eq!(camera_bit_rate(1920, 1080, 30.0, true), 7_464_960);
        assert_eq!(camera_bit_rate(1920, 1080, 30.0, false), 12_441_600);
    }

    #[test]
    fn picks_the_largest_landscape_camera_format() {
        let f = |width, height, rate| CameraFormat {
            width,
            height,
            max_frame_rate: Some(rate),
        };
        let formats = [
            f(1280, 720, 30.0),
            f(1920, 1080, 30.0),
            f(1080, 1920, 30.0),
            f(4096, 2160, 30.0),
            f(1920, 1080, 60.0),
        ];
        // Portrait and wider than 3840 are out; 1080p at 60 beats 1080p at 30.
        assert_eq!(best_camera_format(&formats), Some(4));
        // Equal formats: the first wins, as with Swift's max(by:).
        assert_eq!(
            best_camera_format(&[f(1920, 1080, 30.0), f(1920, 1080, 30.0)]),
            Some(0)
        );
        assert_eq!(best_camera_format(&[f(720, 1280, 30.0)]), None);
        assert_eq!(camera_frame_rate(&f(1280, 720, 120.0)), 60.0);
        assert_eq!(camera_frame_rate(&f(1280, 720, 29.97)), 29.97);
        assert_eq!(
            camera_frame_rate(&CameraFormat {
                width: 1280,
                height: 720,
                max_frame_rate: None
            }),
            30.0
        );
    }

    #[test]
    fn render_size_follows_the_base_aspect() {
        assert_eq!(render_size(3024.0, 1964.0), (1920.0, 1246.0));
        assert_eq!(render_size(1280.0, 720.0), (1280.0, 720.0));
        assert_eq!(render_size(1920.0, 1080.0), (1920.0, 1080.0));
        // 1001x501: 501 → odd, loses one; 1001 → 1000.
        assert_eq!(render_size(1001.0, 501.0), (1000.0, 500.0));
    }

    #[test]
    fn fit_centres_and_letterboxes() {
        // 4:3 camera in a 16:9 frame: full height, pillarboxed.
        let camera = Rect::new(0.0, 0.0, 640.0, 480.0);
        assert_eq!(fit_scale(camera, 1280.0, 720.0), 1.5);
        assert_eq!(
            centre(Rect::new(0.0, 0.0, 960.0, 720.0), 1280.0, 720.0),
            (160.0, 0.0)
        );
        // Scaled by 5/3, Core Image reports 1067 wide: the Swift composer centred that, at 106.5.
        assert_eq!(fit_scale(camera, 1280.0, 800.0), 800.0 / 480.0);
        assert_eq!(
            centre(Rect::new(0.0, 0.0, 1067.0, 800.0), 1280.0, 800.0),
            (106.5, 0.0)
        );
        // The screen in its own render size: rounding the height to even leaves it height-bound.
        assert_eq!(
            fit_scale(Rect::new(0.0, 0.0, 3024.0, 1964.0), 1920.0, 1246.0),
            1246.0 / 1964.0
        );
    }

    #[test]
    fn bubble_sits_in_its_corner() {
        let camera = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        // Medium (0.28) at 1280x720: diameter 202 (201.6), margin 25 (25.2).
        let b = bubble(
            camera,
            &layout(Corner::BottomRight, BubbleSize::Medium, BubbleShape::Circle),
            1280.0,
            720.0,
        );
        assert_eq!(b.rect, Rect::new(1280.0 - 202.0 - 25.0, 25.0, 202.0, 202.0));
        assert_eq!(b.crop, Rect::new(420.0, 0.0, 1080.0, 1080.0));
        assert_eq!(b.scale, 202.0 / 1080.0);
        assert_eq!((b.radius, b.ring_radius), (101.0, 104.0));
        assert_eq!(b.ring, Rect::new(1053.0 - 3.0, 22.0, 208.0, 208.0));

        let tl = bubble(
            camera,
            &layout(Corner::TopLeft, BubbleSize::Small, BubbleShape::Rounded),
            1280.0,
            720.0,
        );
        // Small (0.2): 144; rounded corners are 18% of it.
        assert_eq!(tl.rect, Rect::new(25.0, 720.0 - 144.0 - 25.0, 144.0, 144.0));
        assert!((tl.radius - 25.92).abs() < 1e-9);

        let tr = bubble(
            camera,
            &layout(Corner::TopRight, BubbleSize::Large, BubbleShape::Circle),
            1920.0,
            1246.0,
        );
        // Large (0.38) at 1246 high: 473 (473.48), margin 44 (43.61).
        assert_eq!(
            tr.rect,
            Rect::new(1920.0 - 473.0 - 44.0, 1246.0 - 473.0 - 44.0, 473.0, 473.0)
        );

        let bl = bubble(
            Rect::new(0.0, 0.0, 480.0, 640.0),
            &layout(Corner::BottomLeft, BubbleSize::Medium, BubbleShape::Circle),
            1280.0,
            720.0,
        );
        // A portrait camera crops a square from its middle.
        assert_eq!(bl.crop, Rect::new(0.0, 80.0, 480.0, 480.0));
        assert_eq!((bl.rect.x, bl.rect.y), (25.0, 25.0));
    }

    #[test]
    fn preview_window_frames() {
        let screen = Rect::new(0.0, 0.0, 1512.0, 945.0);
        assert_eq!(
            preview_frame(PreviewSize::Bubble, screen, None),
            Rect::new(1512.0 - 212.0, 32.0, 180.0, 180.0)
        );
        // A saved spot near the screen is kept; one far off it is not.
        assert_eq!(
            preview_frame(PreviewSize::Bubble, screen, Some((-100.0, 400.0))).x,
            -100.0
        );
        assert_eq!(
            preview_frame(PreviewSize::Bubble, screen, Some((-180.0, 400.0))).x,
            -180.0
        );
        assert_eq!(
            preview_frame(PreviewSize::Bubble, screen, Some((1692.0, 400.0))).x,
            1300.0
        );
        // Large: 45% of the width up to 720, 16:9.
        assert_eq!(
            preview_frame(PreviewSize::Large, screen, None),
            Rect::new(1512.0 - 680.4 - 32.0, 32.0, 680.4, 383.0)
        );
        let wide = Rect::new(0.0, 25.0, 2560.0, 1390.0);
        assert_eq!(
            preview_frame(PreviewSize::Large, wide, None),
            Rect::new(2560.0 - 752.0, 57.0, 720.0, 405.0)
        );
        assert_eq!(preview_frame(PreviewSize::Full, wide, None), wide);
    }
}
