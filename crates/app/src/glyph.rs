//! The menu bar glyph: two channel bars and a master stub, 18 × 14 pt, drawn in code. Healthy
//! and absent are template images (the menu bar tints them); the coloured states draw their
//! neutral parts in black or white for the bar's appearance. States change shape as well as
//! colour: recording swaps the stub for a dot, a dropped mic hollows its bar, low battery is a
//! battery, an error a triangle, absent is all hollow.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSBezierPath, NSColor, NSImage,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize};

use crate::draw::rect;
use crate::i18n::tr;
use crate::model::{Attention, Glyph};
use crate::theme::hex;

/// Whether the menu bar (the current drawing appearance) is dark.
fn on_dark() -> bool {
    // SAFETY: AppKit's appearance name constants.
    let names = unsafe { [NSAppearanceNameAqua, NSAppearanceNameDarkAqua] };
    NSAppearance::currentDrawingAppearance()
        .bestMatchFromAppearancesWithNames(&NSArray::from_slice(&names))
        .is_some_and(|n| n.isEqualToString(names[1]))
}

fn bar(r: NSRect, fill: bool, color: &NSColor) {
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, 1.0, 1.0);
    if fill {
        color.setFill();
        path.fill();
    } else {
        color.setStroke();
        path.setLineWidth(1.0);
        path.stroke();
    }
}

/// Draws the glyph for `state` in an 18 × 14 flipped space.
fn paint(state: Glyph) {
    let dark = on_dark();
    let neutral = if dark {
        NSColor::whiteColor()
    } else {
        NSColor::blackColor()
    };
    let amber = hex(0xE4A100);
    let ink = if state == Glyph::Recording {
        hex(0xC4261F)
    } else {
        neutral.clone()
    };
    match state {
        Glyph::Healthy => {
            bar(rect(1.0, 5.0, 3.0, 8.0), true, &ink);
            bar(rect(6.0, 2.0, 3.0, 11.0), true, &ink);
            bar(rect(12.0, 7.0, 5.0, 3.0), true, &ink);
        }
        Glyph::Recording => {
            bar(rect(1.0, 5.0, 3.0, 8.0), true, &ink);
            bar(rect(6.0, 2.0, 3.0, 11.0), true, &ink);
            ink.setFill();
            NSBezierPath::bezierPathWithOvalInRect(rect(11.5, 5.5, 6.0, 6.0)).fill();
        }
        Glyph::Attention(Attention::Dropped) => {
            bar(rect(1.0, 5.0, 3.0, 8.0), true, &neutral);
            bar(rect(6.5, 2.5, 2.0, 10.0), false, &amber);
            bar(rect(12.0, 7.0, 5.0, 3.0), true, &neutral);
        }
        Glyph::Attention(Attention::LowBattery) => {
            bar(rect(1.0, 5.0, 3.0, 8.0), true, &neutral);
            bar(rect(6.0, 2.0, 3.0, 11.0), true, &neutral);
            bar(rect(11.5, 5.5, 6.0, 5.0), false, &amber);
            bar(rect(12.5, 6.5, 1.5, 3.0), true, &amber);
        }
        Glyph::Attention(Attention::Error) => {
            // A warning triangle: a different shape from the recording dot, not just a colour.
            bar(rect(1.0, 5.0, 3.0, 8.0), true, &neutral);
            bar(rect(6.0, 2.0, 3.0, 11.0), true, &neutral);
            let tri = NSBezierPath::bezierPath();
            tri.moveToPoint(NSPoint::new(14.5, 3.5));
            tri.lineToPoint(NSPoint::new(18.0, 12.5));
            tri.lineToPoint(NSPoint::new(11.0, 12.5));
            tri.closePath();
            amber.setFill();
            tri.fill();
            (if dark {
                NSColor::blackColor()
            } else {
                NSColor::whiteColor()
            })
            .setFill();
            NSBezierPath::fillRect(rect(14.0, 6.5, 1.0, 3.0));
            NSBezierPath::fillRect(rect(14.0, 10.5, 1.0, 1.0));
        }
        Glyph::Absent => {
            bar(rect(1.5, 5.5, 2.0, 7.0), false, &neutral);
            bar(rect(6.5, 2.5, 2.0, 10.0), false, &neutral);
            bar(rect(12.5, 7.5, 4.0, 2.0), false, &neutral);
        }
    }
}

/// The status item image for `state`, drawn at display time so its neutral parts follow the
/// menu bar's own appearance.
pub fn image(state: Glyph) -> Retained<NSImage> {
    let handler = RcBlock::new(move |_r: NSRect| -> Bool {
        paint(state);
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(18.0, 14.0), true, &handler);
    // Pure-ink glyphs are templates; coloured ones pick black or white themselves.
    image.setTemplate(matches!(state, Glyph::Healthy | Glyph::Absent));
    image
}

pub fn accessibility(state: Glyph) -> String {
    match state {
        Glyph::Healthy => tr("Lavdeck, all mics linked"),
        Glyph::Recording => tr("Lavdeck, recording"),
        Glyph::Attention(Attention::Dropped) => tr("Lavdeck, a mic disconnected"),
        Glyph::Attention(Attention::LowBattery) => tr("Lavdeck, a mic battery is low"),
        Glyph::Attention(Attention::Error) => tr("Lavdeck, something needs attention"),
        Glyph::Absent => tr("Lavdeck, receiver not connected"),
    }
}
