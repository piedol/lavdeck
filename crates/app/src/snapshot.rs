//! Development aid, as `Snapshot.swift`: `Lavdeck --demo --snapshot <dir>` draws the deck
//! (idle, recording and identify; light, dark and Increase Contrast light) and the six menu
//! bar glyphs (light and dark) to PNGs, then quits. It never takes focus or touches the
//! receiver. `--snapshot-windows <dir>` draws Recordings and each Settings tab the same way.

use std::path::{Path, PathBuf};

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSAppearanceCustomization, NSColor, NSCompositingOperation, NSImage, NSRectFillUsingOperation,
};
use objc2_foundation::{NSRect, NSSize};

use crate::canvas::{Canvas, Layer};
use crate::draw::rect;
use crate::model::Glyph;
use crate::theme::hex;
use crate::{deck, demo, glyph, panel, shots, store, util};

pub struct Args {
    pub demo: bool,
    pub snapshot: Option<PathBuf>,
    pub windows: Option<PathBuf>,
    pub force_low_battery: bool,
    pub drive: Option<PathBuf>,
}

impl Args {
    pub fn parse(args: &[String]) -> Args {
        let after = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .map(PathBuf::from)
        };
        Args {
            demo: args.iter().any(|a| a == "--demo"),
            snapshot: after("--snapshot"),
            windows: after("--snapshot-windows"),
            force_low_battery: args.iter().any(|a| a == "--force-low-battery"),
            drive: after("--drive"),
        }
    }

    /// A run that draws or drives and quits: no menu bar icon of its own needed.
    pub fn is_scripted(&self) -> bool {
        self.snapshot.is_some() || self.windows.is_some() || self.drive.is_some()
    }
}

/// Draws the deck for the current model in each appearance.
fn render(name: &str, dir: &Path) {
    for suffix in ["light", "dark", "hc-light"] {
        let mtm = util::mtm();
        let root = panel::deck_root(mtm);
        root.setAppearance(shots::appearance(suffix).as_deref());
        let layer = Layer::new(root.clone());
        deck::reset_peaks();
        let h = {
            let s = store::with(|s| s.clone());
            deck::render(&layer, &s, &panel::LINKS)
        };
        root.setFrame(rect(0.0, 0.0, deck::W, h));
        shots::write(&root, &dir.join(format!("{name}-{suffix}.png")));
    }
}

/// Draws a glyph the way the menu bar does: template images take the bar's label colour.
fn glyph_png(state: Glyph, suffix: &str, path: &Path) {
    let mtm = util::mtm();
    let dark = suffix == "dark";
    let view = Canvas::new(mtm, rect(0.0, 0.0, 54.0, 42.0));
    view.setAppearance(shots::appearance(suffix).as_deref());
    view.on_draw(move |r| {
        let bg = if dark { hex(0x2B2D31) } else { hex(0xE8E9EB) };
        crate::draw::fill_rect(r, &bg);
        let source = glyph::image(state);
        let target = rect(9.0, 7.0, 36.0, 28.0);
        let image = if source.isTemplate() {
            let src = source.clone();
            let handler = RcBlock::new(move |r: NSRect| -> Bool {
                src.drawInRect(r);
                (if dark {
                    NSColor::whiteColor()
                } else {
                    NSColor::blackColor()
                })
                .set();
                NSRectFillUsingOperation(r, NSCompositingOperation::SourceAtop);
                Bool::YES
            });
            NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(18.0, 14.0), false, &handler)
        } else {
            source
        };
        // SAFETY: no hints.
        unsafe {
            image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                target,
                NSRect::ZERO,
                NSCompositingOperation::SourceOver,
                1.0,
                true,
                None,
            )
        };
    });
    shots::write(&view, path);
}

/// The Swift app's snapshot set, then quit.
pub fn run(dir: PathBuf, demo_mode: bool) {
    let _ = std::fs::create_dir_all(&dir);
    util::later(1.0, move || {
        if demo_mode {
            store::update(|s| demo::load(s, false));
            render("idle", &dir);
            store::update(|s| demo::load(s, true));
            render("recording", &dir);
            store::update(demo::load_identify);
        } else {
            render("idle", &dir);
        }
        if demo_mode {
            render("identify", &dir);
        }
        for state in Glyph::ALL {
            for suffix in ["light", "dark"] {
                glyph_png(
                    state,
                    suffix,
                    &dir.join(format!("glyph-{}-{suffix}.png", state.file_name())),
                );
            }
        }
        std::process::exit(0);
    });
}

/// Recordings (light and dark, one take and several selected) and each Settings tab.
pub fn run_windows(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    util::later(0.5, move || {
        store::update(|s| demo::load(s, false));
        steps(dir, 0);
    });
}

fn steps(dir: PathBuf, i: usize) {
    let shots: &[(&str, &str, fn())] = &[
        ("recordings", "light", || {
            crate::recordings::show_for_shots(false, None)
        }),
        ("recordings", "dark", || {
            crate::recordings::show_for_shots(true, None)
        }),
        ("recordings-selection", "dark", || {
            crate::recordings::show_for_shots(true, Some(4))
        }),
        ("recordings-selection", "light", || {
            crate::recordings::show_for_shots(false, Some(4))
        }),
        ("settings-general", "light", || {
            crate::settings_window::show_for_shots(false, 0)
        }),
        ("settings-video", "light", || {
            crate::settings_window::show_for_shots(false, 1)
        }),
        ("settings-plugins", "light", || {
            crate::settings_window::show_for_shots(false, 2)
        }),
        ("settings-general", "dark", || {
            crate::settings_window::show_for_shots(true, 0)
        }),
        ("settings-plugins", "dark", || {
            crate::settings_window::show_for_shots(true, 2)
        }),
    ];
    let Some((name, suffix, show)) = shots.get(i) else {
        std::process::exit(0);
    };
    show();
    let path = dir.join(format!("{name}-{suffix}.png"));
    util::later(1.2, move || {
        if name.starts_with("recordings") {
            crate::recordings::draw_for_shots(&path);
        } else {
            crate::settings_window::draw_for_shots(&path);
        }
        steps(dir, i + 1);
    });
}
