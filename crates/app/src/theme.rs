//! The Channel Strip palette and type (DESIGN.md). Neutrals resolve per appearance (light,
//! dark and both Increase Contrast variants) through dynamic `NSColor`s; the signal lamps are
//! fixed in every appearance.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAccessibilityHighContrastAqua,
    NSAppearanceNameAccessibilityHighContrastDarkAqua, NSAppearanceNameAqua,
    NSAppearanceNameDarkAqua, NSColor, NSColorSpace, NSFont, NSFontWeight, NSFontWeightBold,
    NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold,
};
use objc2_foundation::{NSArray, NSString};

pub fn hex(rgb: u32) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(rgb >> 16 & 0xff) / 255.0,
        f64::from(rgb >> 8 & 0xff) / 255.0,
        f64::from(rgb & 0xff) / 255.0,
        1.0,
    )
}

/// Which of the four appearances `appearance` is closest to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    Light,
    Dark,
    HighContrastLight,
    HighContrastDark,
}

pub fn look(appearance: &NSAppearance) -> Look {
    // SAFETY: AppKit's appearance name constants.
    let names: [&NSString; 4] = unsafe {
        [
            NSAppearanceNameAqua,
            NSAppearanceNameDarkAqua,
            NSAppearanceNameAccessibilityHighContrastAqua,
            NSAppearanceNameAccessibilityHighContrastDarkAqua,
        ]
    };
    let best = appearance.bestMatchFromAppearancesWithNames(&NSArray::from_slice(&names));
    match best {
        Some(n) if n.isEqualToString(names[1]) => Look::Dark,
        Some(n) if n.isEqualToString(names[2]) => Look::HighContrastLight,
        Some(n) if n.isEqualToString(names[3]) => Look::HighContrastDark,
        _ => Look::Light,
    }
}

/// The appearance AppKit is drawing in right now.
pub fn current_look() -> Look {
    look(&NSAppearance::currentDrawingAppearance())
}

fn dynamic(
    light: u32,
    dark: u32,
    hc_light: Option<u32>,
    hc_dark: Option<u32>,
) -> Retained<NSColor> {
    let colors = [
        hex(light),
        hex(dark),
        hex(hc_light.unwrap_or(light)),
        hex(hc_dark.unwrap_or(dark)),
    ];
    let provider = RcBlock::new(
        move |appearance: NonNull<NSAppearance>| -> NonNull<NSColor> {
            // SAFETY: AppKit passes a valid appearance.
            let i = match look(unsafe { appearance.as_ref() }) {
                Look::Light => 0,
                Look::Dark => 1,
                Look::HighContrastLight => 2,
                Look::HighContrastDark => 3,
            };
            // The block owns the colors for as long as AppKit keeps the dynamic color.
            NonNull::from(&*colors[i])
        },
    );
    // SAFETY: the provider always returns one of the colors it owns.
    unsafe { NSColor::colorWithName_dynamicProvider(None, &provider) }
}

macro_rules! tokens {
    ($($(#[$doc:meta])* $name:ident = $make:expr;)*) => {
        $(
            $(#[$doc])*
            pub fn $name() -> Retained<NSColor> {
                thread_local! {
                    static C: Retained<NSColor> = $make;
                }
                C.with(|c| c.clone())
            }
        )*
    };
}

tokens! {
    /// The anodized deck: the popover background.
    deck = dynamic(0xE3E5E8, 0x1C1E21, None, None);
    /// Strip faces, the master section and the absent panel.
    face = dynamic(0xF7F8F9, 0x26292D, None, None);
    /// Recessed surfaces: meter housing, arm button, switch track, hollow plate.
    well = dynamic(0xFFFFFF, 0x16181B, None, None);
    /// Every 0.5 pt module edge.
    hairline = dynamic(0xCDD2D7, 0x3A3F45, Some(0x6B737B), Some(0x9AA3AC));
    ink = dynamic(0x121416, 0xF1F3F5, None, None);
    /// Text on an `ink` fill.
    on_ink = dynamic(0xFFFFFF, 0x121416, None, None);
    mute = dynamic(0x5A636B, 0xA9B1B9, None, None);
    /// The scribble strip is black in every appearance.
    plate = hex(0x121416);
    plate_text = hex(0xFFFFFF);
    plate_sub = hex(0xC9D0D6);
    /// A recording plate's file line.
    plate_rec_sub = hex(0xFFE1DE);
    led_off = dynamic(0xD5D9DD, 0x34393F, None, None);
    green = hex(0x18A957);
    amber = hex(0xE4A100);
    /// The meter's clip zone; never the recording state.
    red = hex(0xD32F2F);
    /// The live take, and only the live take.
    rec = hex(0xC4261F);
    /// Record red for text: lifted in dark appearance to clear 4.5:1.
    rec_text = dynamic(0xC4261F, 0xFF6B61, None, None);
    warn_fill = dynamic(0xFBF0DC, 0x3A2E14, None, None);
    warn_text = dynamic(0x6B4300, 0xFFD27A, None, None);
}

/// `color` at `alpha` (dynamic colors stay dynamic).
pub fn alpha(color: &NSColor, alpha: f64) -> Retained<NSColor> {
    color.colorWithAlphaComponent(alpha)
}

/// `color` as a fixed sRGB value for the appearance being drawn (for APIs that don't resolve
/// dynamic colors themselves, such as symbol palettes).
pub fn resolved(color: &NSColor) -> Retained<NSColor> {
    color
        .colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())
        .unwrap_or_else(|| color.retain())
}

use objc2::Message;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl Weight {
    pub fn ns(self) -> NSFontWeight {
        // SAFETY: AppKit's font weight constants.
        unsafe {
            match self {
                Weight::Regular => NSFontWeightRegular,
                Weight::Medium => NSFontWeightMedium,
                Weight::Semibold => NSFontWeightSemibold,
                Weight::Bold => NSFontWeightBold,
            }
        }
    }
}

/// SF Pro at a size and weight.
pub fn font(size: f64, weight: Weight) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, weight.ns())
}

/// SF Pro with tabular figures (the elapsed counter, the fader scale).
pub fn digits(size: f64, weight: Weight) -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(size, weight.ns())
}

/// SF Mono: values the receiver or the filesystem produced.
pub fn mono(size: f64, weight: Weight) -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(size, weight.ns())
}

/// Body (12 pt).
pub fn label() -> Retained<NSFont> {
    font(12.0, Weight::Regular)
}

/// Body strong (12 pt semibold).
pub fn label_bold() -> Retained<NSFont> {
    font(12.0, Weight::Semibold)
}

/// Silkscreen caps (11 pt bold).
pub fn caps() -> Retained<NSFont> {
    font(11.0, Weight::Bold)
}

/// Machine data (11 pt SF Mono medium).
pub fn mono_data() -> Retained<NSFont> {
    mono(11.0, Weight::Medium)
}
