//! Drawing helpers for custom views, in flipped (top-down) coordinates: rounded shapes with
//! inside strokes (as SwiftUI's `strokeBorder` draws them), styled text and SF Symbols.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAttributedStringNSExtendedStringDrawing, NSAttributedStringNSStringDrawing,
    NSBackgroundColorAttributeName, NSBezierPath, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSGraphicsContext, NSImage, NSImageSymbolConfiguration,
    NSKernAttributeName, NSLineBreakMode, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    NSStrikethroughStyleAttributeName, NSStringDrawingOptions,
};
use objc2_core_graphics::CGContext;
use objc2_foundation::{
    NSAttributedString, NSDictionary, NSMutableAttributedString, NSNumber, NSPoint, NSRange,
    NSRect, NSSize, NSString,
};

use crate::theme::{self, Weight};
use crate::util::ns;

pub fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

pub fn inset(r: NSRect, dx: f64, dy: f64) -> NSRect {
    rect(
        r.origin.x + dx,
        r.origin.y + dy,
        r.size.width - 2.0 * dx,
        r.size.height - 2.0 * dy,
    )
}

pub fn fill_round(r: NSRect, radius: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, radius, radius).fill();
}

/// A border drawn inside `r`, like SwiftUI's `strokeBorder`.
pub fn stroke_round(r: NSRect, radius: f64, width: f64, color: &NSColor) {
    let r = inset(r, width / 2.0, width / 2.0);
    let radius = (radius - width / 2.0).max(0.0);
    color.setStroke();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, radius, radius);
    path.setLineWidth(width);
    path.stroke();
}

pub fn fill_oval(r: NSRect, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithOvalInRect(r).fill();
}

pub fn stroke_oval(r: NSRect, width: f64, color: &NSColor) {
    color.setStroke();
    let path = NSBezierPath::bezierPathWithOvalInRect(inset(r, width / 2.0, width / 2.0));
    path.setLineWidth(width);
    path.stroke();
}

pub fn fill_rect(r: NSRect, color: &NSColor) {
    color.setFill();
    NSBezierPath::fillRect(r);
}

fn cg() -> Option<Retained<CGContext>> {
    NSGraphicsContext::currentContext().map(|c| c.CGContext())
}

/// Draws `f` as one group at `alpha`, like SwiftUI's `.opacity` on a composite view.
pub fn with_alpha(alpha: f64, f: impl FnOnce()) {
    if alpha >= 1.0 {
        f();
        return;
    }
    let Some(ctx) = cg() else { return f() };
    CGContext::save_g_state(Some(&ctx));
    CGContext::set_alpha(Some(&ctx), alpha);
    // SAFETY: no auxiliary info.
    unsafe { CGContext::begin_transparency_layer(Some(&ctx), None) };
    f();
    CGContext::end_transparency_layer(Some(&ctx));
    CGContext::restore_g_state(Some(&ctx));
}

/// Draws `f` with a drop shadow (the fader cap's).
pub fn with_shadow(dy: f64, blur: f64, color: &NSColor, f: impl FnOnce()) {
    let Some(ctx) = cg() else { return f() };
    CGContext::save_g_state(Some(&ctx));
    let c = theme::resolved(color);
    let cg_color = c.CGColor();
    // Flipped view: positive y in the view is negative in the context's unflipped space.
    CGContext::set_shadow_with_color(
        Some(&ctx),
        objc2_core_foundation::CGSize::new(0.0, -dy),
        blur,
        Some(&cg_color),
    );
    f();
    CGContext::restore_g_state(Some(&ctx));
}

/// Styled text: font, color, tracking, truncation.
#[derive(Clone)]
pub struct Text {
    pub string: String,
    pub font: Retained<NSFont>,
    pub color: Retained<NSColor>,
    pub kern: f64,
    pub strike: bool,
    pub line_break: NSLineBreakMode,
    pub align: objc2_app_kit::NSTextAlignment,
}

impl Text {
    pub fn new(
        string: impl Into<String>,
        font: Retained<NSFont>,
        color: Retained<NSColor>,
    ) -> Text {
        Text {
            string: string.into(),
            font,
            color,
            kern: 0.0,
            strike: false,
            line_break: NSLineBreakMode::ByTruncatingTail,
            align: objc2_app_kit::NSTextAlignment::Natural,
        }
    }

    pub fn kern(mut self, kern: f64) -> Text {
        self.kern = kern;
        self
    }

    pub fn strike(mut self, on: bool) -> Text {
        self.strike = on;
        self
    }

    pub fn middle(mut self) -> Text {
        self.line_break = NSLineBreakMode::ByTruncatingMiddle;
        self
    }

    pub fn centered(mut self) -> Text {
        self.align = objc2_app_kit::NSTextAlignment::Center;
        self
    }

    pub fn wrapping(mut self) -> Text {
        self.line_break = NSLineBreakMode::ByWordWrapping;
        self
    }

    pub fn attributed(&self) -> Retained<NSAttributedString> {
        let style = NSMutableParagraphStyle::new();
        style.setLineBreakMode(self.line_break);
        style.setAlignment(self.align);
        let kern = NSNumber::new_f64(self.kern);
        let strike = NSNumber::new_isize(if self.strike { 1 } else { 0 });
        // SAFETY: AppKit's attribute keys, each with a value of the documented type.
        let mut keys: Vec<&NSString> = unsafe {
            vec![
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
                NSStrikethroughStyleAttributeName,
            ]
        };
        let mut values: Vec<&AnyObject> = vec![&self.font, &self.color, &style, &strike];
        // An explicit 0 would turn off the font's own kerning, which SwiftUI keeps.
        if self.kern != 0.0 {
            // SAFETY: AppKit's attribute key.
            keys.push(unsafe { NSKernAttributeName });
            values.push(&kern);
        }
        let attrs = NSDictionary::from_slices(&keys, &values);
        // SAFETY: the attributes are valid string attributes.
        unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &ns(&self.string),
                Some(&attrs),
            )
        }
    }

    /// One line's size.
    pub fn size(&self) -> NSSize {
        let s = self.attributed().size();
        NSSize::new(s.width.ceil(), s.height.ceil())
    }

    pub fn width(&self) -> f64 {
        self.size().width
    }

    pub fn height(&self) -> f64 {
        line_height(&self.font)
    }

    /// The height wrapped text needs at `width`.
    pub fn wrapped_height(&self, width: f64) -> f64 {
        let r = self.attributed().boundingRectWithSize_options_context(
            NSSize::new(width, 100_000.0),
            NSStringDrawingOptions::UsesLineFragmentOrigin,
            None,
        );
        r.size.height.ceil()
    }

    /// Draws one line with its top-left at `p`.
    pub fn draw_at(&self, x: f64, y: f64) {
        self.attributed().drawAtPoint(NSPoint::new(x, y));
    }

    /// Draws in `r`: one truncated line, or wrapped lines for [`Text::wrapping`].
    pub fn draw_in(&self, r: NSRect) {
        let mut options = NSStringDrawingOptions::UsesLineFragmentOrigin;
        if self.line_break != NSLineBreakMode::ByWordWrapping {
            options |= NSStringDrawingOptions::TruncatesLastVisibleLine;
        }
        let r = rect(
            r.origin.x,
            r.origin.y,
            r.size.width.max(1.0),
            r.size.height.max(self.height()),
        );
        self.attributed()
            .drawWithRect_options_context(r, options, None);
    }
}

/// The line height AppKit gives a font (what a one-line label is tall).
pub fn line_height(font: &NSFont) -> f64 {
    let layout = objc2_app_kit::NSLayoutManager::new();
    layout.defaultLineHeightForFont(font).ceil()
}

/// Text with some ranges highlighted (search hits in a transcript line).
pub fn highlighted(
    text: &Text,
    ranges: &[(usize, usize)],
    bold: &NSFont,
    background: &NSColor,
) -> Retained<NSAttributedString> {
    let base = text.attributed();
    let out = NSMutableAttributedString::initWithAttributedString(
        NSMutableAttributedString::alloc(),
        &base,
    );
    let utf16: Vec<usize> = {
        // Byte offsets to UTF-16 offsets.
        let mut map = vec![0; text.string.len() + 1];
        let mut u = 0;
        for (i, c) in text.string.char_indices() {
            map[i] = u;
            u += c.len_utf16();
        }
        map[text.string.len()] = u;
        map
    };
    for &(start, end) in ranges {
        let (a, b) = (utf16[start], utf16[end]);
        let range = NSRange::new(a, b - a);
        // SAFETY: valid attribute keys and values within the string's range.
        unsafe {
            out.addAttribute_value_range(NSFontAttributeName, bold, range);
            out.addAttribute_value_range(NSBackgroundColorAttributeName, background, range);
        }
    }
    Retained::into_super(out)
}

/// An SF Symbol at a point size and weight, in one colour (resolved for the current
/// appearance), drawn monochrome as SwiftUI does: cut-outs stay transparent.
pub fn symbol(name: &str, size: f64, weight: Weight, color: &NSColor) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), None)?;
    let sized = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, weight.ns());
    let image = image.imageWithSymbolConfiguration(&sized)?;
    let tint = theme::resolved(color);
    let src = image.clone();
    let handler = block2::RcBlock::new(move |r: NSRect| -> objc2::runtime::Bool {
        src.drawInRect(r);
        tint.set();
        objc2_app_kit::NSRectFillUsingOperation(
            r,
            objc2_app_kit::NSCompositingOperation::SourceAtop,
        );
        objc2::runtime::Bool::YES
    });
    let tinted = NSImage::imageWithSize_flipped_drawingHandler(image.size(), false, &handler);
    tinted.setAlignmentRect(image.alignmentRect());
    Some(tinted)
}

/// Draws a symbol with its alignment rectangle centered on (`cx`, `cy`), as SwiftUI centers
/// an `Image` in a stack. Returns its size.
pub fn draw_symbol(
    name: &str,
    size: f64,
    weight: Weight,
    color: &NSColor,
    cx: f64,
    cy: f64,
) -> NSSize {
    let Some(img) = symbol(name, size, weight, color) else {
        return NSSize::new(0.0, 0.0);
    };
    let s = img.size();
    let a = img.alignmentRect();
    // Alignment centre in the image, measured from its top-left.
    let (ax, ay) = (
        a.origin.x + a.size.width / 2.0,
        s.height - (a.origin.y + a.size.height / 2.0),
    );
    let (cx, cy) = (cx - ax + s.width / 2.0, cy - ay + s.height / 2.0);
    // SAFETY: no hints.
    unsafe {
        img.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            rect(
                ((cx - s.width / 2.0) * 2.0).round() / 2.0,
                ((cy - s.height / 2.0) * 2.0).round() / 2.0,
                s.width,
                s.height,
            ),
            NSRect::ZERO,
            objc2_app_kit::NSCompositingOperation::SourceOver,
            1.0,
            true,
            None,
        )
    };
    s
}

/// A symbol's size without drawing it.
pub fn symbol_size(name: &str, size: f64, weight: Weight) -> NSSize {
    symbol(name, size, weight, &NSColor::blackColor()).map_or(NSSize::new(0.0, 0.0), |i| i.size())
}
