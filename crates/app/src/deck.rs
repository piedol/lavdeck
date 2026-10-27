//! The deck: two channel strips and a master section on an anodized deck (DESIGN.md's
//! Channel Strip). [`render`] lays it out from the view model into a [`Layer`], at the fixed
//! 372 pt width, and returns its height. Each part is a [`Canvas`] drawn in code; the rename
//! field and spinners are native controls.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{AnyThread, ClassType, DefinedClass};
use objc2_app_kit::{
    NSColor, NSControlSize, NSFocusRingType, NSHapticFeedbackManager, NSHapticFeedbackPattern,
    NSHapticFeedbackPerformanceTime, NSHapticFeedbackPerformer, NSProgressIndicator,
    NSProgressIndicatorStyle, NSTextField,
};
use objc2_foundation::NSObjectProtocol;
use objc2_foundation::NSRect;
use objc2_quartz_core::CAMediaTiming;

use crate::canvas::{Build, DragPhase, FieldDelegate, Layer};
use crate::draw::{self, Text, rect};
use crate::format;
use crate::i18n::{tr, trf, trn};
use crate::model::plugins::{CaptureToggle, Job, JobState};
use crate::model::settings::RecordingFormat;
use crate::model::status::ChannelMode;
use crate::model::{AppState, Link};
use crate::store;
use crate::theme::{self, Weight};
use crate::util::{self, ns};

/// The deck's fixed width.
pub const W: f64 = 372.0;
const PAD: f64 = 10.0;
const STRIP_W: f64 = 100.0;
const GUTTER: f64 = 6.0;
const GAIN_STEPS: [i32; 5] = [12, 6, 0, -6, -12];

/// Rounds to the 2x pixel grid, as SwiftUI places views.
trait Px {
    fn px(self) -> f64;
}

impl Px for f64 {
    fn px(self) -> f64 {
        (self * 2.0).round() / 2.0
    }
}

/// One line of text at a size, as tall as SwiftUI lays it out.
pub fn th(size: f64) -> f64 {
    match size as i64 {
        ..=10 => 13.0,
        11 => 14.0,
        12 => 15.0,
        13 => 16.0,
        _ => (size * 1.2).ceil(),
    }
}

/// Interface state that isn't the model's: the rename field, meter peaks, a fader drag.
struct Ui {
    editing: Cell<Option<u8>>,
    /// Peak level and when it was set, per strip.
    peaks: RefCell<[(f64, f64); 3]>,
    /// The meter fill last drawn, per strip (for its 80 ms ease).
    lit: Cell<[i64; 3]>,
    drag_index: Cell<Option<usize>>,
    was_recording: Cell<bool>,
}

thread_local! {
    static UI: Ui = Ui {
        editing: Cell::new(None),
        peaks: RefCell::new([(0.0, 0.0); 3]),
        lit: Cell::new([-1; 3]),
        drag_index: Cell::new(None),
        was_recording: Cell::new(false),
    };
}

/// Where the deck's buttons lead, outside the deck.
pub struct Links {
    pub recordings: fn(),
    pub settings: fn(),
}

/// Lays the deck out for `s` and returns its height.
pub fn render(layer: &Layer, s: &AppState, links: &Links) -> f64 {
    let mut b = layer.build();
    let recording_changed = UI.with(|u| u.was_recording.replace(s.is_recording) != s.is_recording);
    if recording_changed && !util::reduce_motion() {
        crossfade(layer, 0.16);
    }
    if s.is_recording || !s.is_connected {
        UI.with(|u| u.editing.set(None));
    }
    let bg = b.canvas("bg", rect(0.0, 0.0, W, 0.0));
    let mut y = PAD;
    y = header(&mut b, s, y);
    if s.identifying {
        y += PAD;
        y = identify_banner(&mut b, s, y);
    }
    y += PAD;
    if s.is_connected {
        let top = y;
        let h1 = strip(&mut b, s, 1, PAD, top);
        let h2 = strip(&mut b, s, 2, PAD + STRIP_W + GUTTER, top);
        let hm = master(&mut b, s, PAD + 2.0 * (STRIP_W + GUTTER), top);
        y = top + h1.max(h2).max(hm) + PAD;
        y = footer(&mut b, s, y, links);
        y = notes(&mut b, s, y, links);
    } else {
        y = absent(&mut b, y, links);
    }
    let height = y + PAD;
    bg.setFrame(rect(0.0, 0.0, W, height));
    bg.on_draw(|r| draw::fill_rect(r, &theme::deck()));
    b.finish();
    height
}

/// Cross-fades the whole deck into its next state (the recording state change).
fn crossfade(layer: &Layer, secs: f64) {
    let Some(l) = layer.root.layer() else { return };
    let t = objc2_quartz_core::CATransition::new();
    t.setDuration(secs);
    // SAFETY: Core Animation's constants.
    unsafe {
        t.setType(objc2_quartz_core::kCATransitionFade);
        t.setTimingFunction(Some(
            &objc2_quartz_core::CAMediaTimingFunction::functionWithName(
                objc2_quartz_core::kCAMediaTimingFunctionEaseInEaseOut,
            ),
        ));
    }
    l.addAnimation_forKey(&t, Some(&ns("lavdeck.crossfade")));
}

// MARK: Header

fn header(b: &mut Build, s: &AppState, y: f64) -> f64 {
    let x = PAD + 4.0;
    let w = W - 2.0 * x;
    let title = Text::new(
        tr("Wireless Mic Rx"),
        theme::font(13.0, Weight::Semibold),
        theme::ink(),
    );
    let title_asc = title.font.ascender();
    if s.is_recording {
        // REC, the running time, the capture sources and, with no mixer to stop from, a Stop.
        let mut parts: Vec<(Text, f64)> = Vec::new();
        let rec = Text::new(tr("REC"), theme::caps(), theme::rec_text());
        parts.push((rec, 0.0));
        if let Some(started) = s.recording_started {
            let elapsed = (format::now() - started).max(0.0).floor();
            parts.push((
                Text::new(
                    format::length(elapsed),
                    theme::mono_data(),
                    theme::rec_text(),
                ),
                0.0,
            ));
        }
        for source in &s.capture_sources {
            let t = trf("· {source}", &[("source", &source_title(source))]);
            parts.push((Text::new(t, theme::label(), theme::mute()), 0.0));
        }
        let widths: f64 =
            parts.iter().map(|(t, _)| t.width()).sum::<f64>() + 8.0 + 5.0 * parts.len() as f64;
        let stop_w = if s.is_connected {
            0.0
        } else {
            stop_capsule_width() + 5.0
        };
        let total = widths + stop_w;
        let h = th(13.0);
        let baseline = y + title_asc;
        let c = b.canvas("header", rect(x, y, w, h));
        let parts2 = parts.clone();
        c.on_draw(move |r| {
            title.draw_at(0.0, 0.0);
            let mut px = r.size.width - total;
            draw::fill_oval(
                rect(px, (title_asc - 4.0 - 3.5).px(), 8.0, 8.0),
                &theme::rec(),
            );
            px += 8.0 + 5.0;
            for (t, _) in &parts2 {
                t.draw_at(px, title_asc - t.font.ascender());
                px += t.width() + 5.0;
            }
        });
        if !s.is_connected {
            let cw = stop_capsule_width();
            let sc = b.canvas("header-stop", rect(x + w - cw, baseline - 15.0, cw, 21.0));
            sc.on_draw(|r| {
                let cap = rect(0.0, 3.0, r.size.width, 21.0 - 6.0 + 0.0);
                draw::fill_round(cap, cap.size.height / 2.0, &theme::rec());
                draw::draw_symbol(
                    "stop.fill",
                    9.0,
                    Weight::Bold,
                    &theme::on_ink(),
                    8.0 + 4.0,
                    cap.origin.y + cap.size.height / 2.0,
                );
                Text::new(tr("Stop"), theme::label_bold(), theme::on_ink())
                    .draw_at(8.0 + 9.0 + 4.0, 3.0);
            })
            .on_click(store::stop_recording)
            .tip(&tr("Stop recording (⌥⌘R)"));
        }
        return y + h;
    }
    if s.is_starting {
        let on: Vec<String> = s
            .starting_sources()
            .iter()
            .map(|src| source_title(src).to_lowercase())
            .collect();
        let label = if on.is_empty() {
            tr("Starting…")
        } else {
            trf("Starting {sources}…", &[("sources", &join_and(&on))])
        };
        let t = Text::new(label, theme::label(), theme::mute());
        let tw = t.width();
        let h = th(13.0);
        let c = b.canvas("header", rect(x, y, w, h));
        c.on_draw(move |r| {
            title.draw_at(0.0, 0.0);
            t.draw_at(r.size.width - tw, title_asc - t.font.ascender());
        });
        let spin = b.native(
            "header-spinner",
            rect(x + w - tw - 6.0 - 12.0, y + 2.0, 12.0, 12.0),
            || {
                let p = NSProgressIndicator::new(util::mtm());
                p.setStyle(NSProgressIndicatorStyle::Spinning);
                p.setControlSize(NSControlSize::Mini);
                p.setIndeterminate(true);
                // SAFETY: plain call.
                unsafe { p.startAnimation(None) };
                p
            },
        );
        let _ = spin;
        return y + h;
    }
    if s.is_connected && !s.identifying {
        // The capsule sets the row's height; the title sits on its text baseline.
        let label = Text::new(tr("Which mic is which?"), theme::label_bold(), theme::ink());
        let sym = draw::symbol_size("waveform", 11.0, Weight::Semibold);
        let cw = 8.0 + sym.width + 4.0 + label.width() + 8.0;
        let ch = 21.0;
        let cy = y;
        let text_top = 3.0;
        // The title sits on the capsule's text baseline.
        let title_top = y + 1.5;
        let tc = b.canvas("header", rect(x, title_top, w - cw - 8.0, th(13.0)));
        tc.on_draw(move |_| title.draw_at(0.0, 0.0));
        let firmware = s
            .status
            .receiver
            .identity
            .firmware
            .clone()
            .unwrap_or_else(|| tr("unknown"));
        let c = b.canvas("identify-button", rect(x + w - cw, cy, cw, ch));
        c.on_draw(move |r| {
            draw::fill_round(r, r.size.height / 2.0, &theme::face());
            draw::stroke_round(r, r.size.height / 2.0, 0.5, &theme::hairline());
            draw::draw_symbol(
                "waveform",
                11.0,
                Weight::Semibold,
                &theme::ink(),
                8.0 + sym.width / 2.0,
                text_top + 7.5,
            );
            label.draw_at(8.0 + sym.width + 4.0, text_top);
        })
        .on_click(store::start_identify)
        .tip(&trf(
            "Talk into a mic to see which strip it is. Receiver: Mobile RX, firmware {firmware}",
            &[("firmware", &firmware)],
        ))
        .a11y(&tr("Which mic is which?"), None);
        return y + 21.0;
    }
    let c = b.canvas("header", rect(x, y, w, th(13.0)));
    c.on_draw(move |_| title.draw_at(0.0, 0.0));
    y + th(13.0)
}

fn stop_capsule_width() -> f64 {
    8.0 + 9.0 + 4.0 + Text::new(tr("Stop"), theme::label_bold(), theme::ink()).width() + 8.0
}

/// "camera and screen", "camera, screen and system audio".
fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => trf(
            "{list} and {last}",
            &[("list", &rest.join(", ")), ("last", last)],
        ),
    }
}

/// A capture source's name: "Screen", "Camera", "System audio", or a plugin's own.
pub fn source_title(source: &str) -> String {
    match source {
        "screen" => tr("Screen"),
        "camera" => tr("Camera"),
        "system-audio" => tr("System audio"),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

fn identify_banner(b: &mut Build, s: &AppState, y: f64) -> f64 {
    let x = PAD;
    let w = W - 2.0 * PAD;
    let title = match s.talking_mic {
        Some(i) => trf(
            "That’s {name} (TX{index}).",
            &[("name", &s.display_name(i)), ("index", &i.to_string())],
        ),
        None => tr("Talk into one mic. Its strip lights up."),
    };
    let title = Text::new(title, theme::label_bold(), theme::ink());
    let detail = Text::new(
        tr("Click a strip’s name to label it. Stereo is on while this is open; your setting returns when you’re done."),
        theme::label(),
        theme::mute(),
    )
    .wrapping();
    let done = Text::new(tr("Done"), theme::label_bold(), theme::on_ink());
    let done_w = 10.0 + done.width() + 10.0;
    let done_h = th(12.0) + 8.0;
    let sym = draw::symbol_size("waveform", 13.0, Weight::Semibold);
    let text_x = 10.0 + sym.width + 9.0;
    // The HStack's 9 pt spacing falls on both sides of its spacer.
    let text_w = w - 10.0 - text_x - 9.0 - 9.0 - done_w;
    let detail_h = detail.wrapped_height(text_w);
    let stack_h = th(12.0) + 3.0 + detail_h;
    let h = 10.0 + stack_h.max(done_h) + 10.0;
    let c = b.canvas("identify", rect(x, y, w, h));
    c.on_draw(move |r| {
        draw::fill_round(r, 9.0, &theme::face());
        draw::stroke_round(r, 9.0, 1.0, &theme::alpha(&theme::green(), 0.6));
        draw::draw_symbol(
            "waveform",
            13.0,
            Weight::Semibold,
            &theme::green(),
            10.0 + sym.width / 2.0,
            10.0 + 8.0,
        );
        title.draw_in(rect(text_x, 10.0, text_w, th(12.0)));
        detail.draw_in(rect(text_x, 10.0 + th(12.0) + 3.0, text_w, detail_h));
    });
    let d = b.canvas(
        "identify-done",
        rect(x + w - 10.0 - done_w, y + 10.0, done_w, done_h),
    );
    d.on_draw(move |r| {
        draw::fill_round(r, r.size.height / 2.0, &theme::ink());
        done.draw_at(10.0, 4.0);
    })
    .on_click(store::stop_identify)
    .a11y(&tr("Done"), None);
    y + h
}

// MARK: Channel strip

fn strip(b: &mut Build, s: &AppState, index: u8, x: f64, y: f64) -> f64 {
    let key = |part: &str| format!("tx{index}-{part}");
    let tx = s.transmitter(index);
    let linked = s.is_linked(index);
    let outdated = s.outdated_firmware().into_iter().find(|o| o.index == index);
    let armed = s.is_armed(index);
    let name = s.display_name(index);
    let slot = format!("TX{index}");
    let named = name != slot;
    let talking = s.identifying && s.talking_mic == Some(index);
    let ix = x + 8.0;
    let iw = STRIP_W - 16.0;
    let mut cy = y + 8.0;

    // Name plate (or the rename field).
    let editing = UI.with(|u| u.editing.get()) == Some(index);
    let plate_h;
    if s.is_recording {
        let dest = s.destination(index);
        let plate = match &dest {
            Some(file) => {
                let side = if index == 1 { "L" } else { "R" };
                let title = if s.recording_mode == Some(ChannelMode::Stereo) {
                    trf("{name} · {side}", &[("name", &name), ("side", side)])
                } else {
                    name.clone()
                };
                Plate {
                    title,
                    subtitle: Some(trf("→ {file}", &[("file", file)])),
                    tag: None,
                    armed: true,
                    dimmed: !linked,
                    hollow: false,
                }
            }
            None => Plate {
                title: name.clone(),
                subtitle: None,
                tag: named.then(|| slot.clone()),
                armed: false,
                dimmed: false,
                hollow: true,
            },
        };
        plate_h = plate.height();
        b.canvas(&key("plate"), rect(ix, cy, iw, plate_h))
            .on_draw(move |r| plate.draw(r));
    } else if editing {
        plate_h = 27.0;
        let field = rename_field(
            b,
            &key("field"),
            index,
            &name,
            named,
            rect(ix, cy, iw, plate_h),
        );
        let _ = field;
    } else {
        plate_h = 27.0;
        let plate = Plate {
            title: name.clone(),
            subtitle: None,
            tag: named.then(|| slot.clone()),
            armed: false,
            dimmed: !linked,
            hollow: false,
        };
        let has_id = tx.and_then(|t| t.identity.bluetooth_id.as_ref()).is_some();
        let c = b.canvas(&key("plate"), rect(ix, cy, iw, plate_h));
        c.on_draw(move |r| plate.draw(r));
        if has_id {
            c.on_click(move || begin_editing(index)).tip(&tr(
                "Click to name this mic (the name follows this physical mic)",
            ));
        }
        c.a11y(
            &trf(
                "{name}, transmitter {index}",
                &[("name", &name), ("index", &index.to_string())],
            ),
            None,
        );
    }
    cy += plate_h + 8.0;

    // Arm button.
    let live = s.is_recording;
    let arm = b.canvas(&key("arm"), rect(ix, cy, iw, 22.0));
    arm.on_draw(move |r| draw_arm(r, armed, live));
    if !live {
        arm.on_click(move || store::toggle_arm(index));
    }
    arm.tip(&if live {
        tr("Stop recording to change which mics record")
    } else if armed {
        tr("This mic will be recorded. Click to leave it out of Lavdeck’s recordings.")
    } else {
        tr("Left out of Lavdeck’s recordings. The receiver still sends this mic to other apps; click to include it.")
    })
    .a11y(&tr("Record this mic"), Some(&if armed { tr("on") } else { tr("off") }));
    cy += 22.0 + 8.0;

    // Link LED.
    let link = s.link(index);
    b.canvas(
        &key("led"),
        rect(x + (STRIP_W - 14.0) / 2.0, cy, 14.0, 14.0),
    )
    .on_draw(move |r| draw_led(r, link))
    .a11y(
        &match link {
            Link::Linked => tr("Linked"),
            Link::Dropped => tr("Disconnected"),
            Link::Off => tr("Not linked"),
        },
        None,
    );
    cy += 14.0 + 8.0;

    // LED meter, with "mono mix" under it when both meters show the mix.
    let inactive = !armed && !s.identifying;
    let level = s.meter(index);
    let mix = armed && linked && s.meters_are_mix() && !s.identifying;
    let (lit, peak) = meter_state(index, level, inactive);
    let changed = UI.with(|u| {
        let mut all = u.lit.get();
        let was = all[index as usize];
        all[index as usize] = lit;
        u.lit.set(all);
        was != lit && was >= 0
    });
    let m = b.canvas(
        &key("meter"),
        rect(x + (STRIP_W - 34.0) / 2.0, cy, 34.0, 114.0),
    );
    m.on_draw(move |r| draw_meter(r, lit, peak, inactive));
    if changed && !util::reduce_motion() {
        m.setWantsLayer(true);
        if let Some(l) = m.layer() {
            let t = objc2_quartz_core::CATransition::new();
            t.setDuration(0.08);
            // SAFETY: Core Animation's constants.
            unsafe {
                t.setType(objc2_quartz_core::kCATransitionFade);
                t.setTimingFunction(Some(
                    &objc2_quartz_core::CAMediaTimingFunction::functionWithName(
                        objc2_quartz_core::kCAMediaTimingFunctionEaseOut,
                    ),
                ));
            }
            l.addAnimation_forKey(&t, Some(&ns("lavdeck.meter")));
        }
    }
    m.tip(&if !armed {
        tr("Not armed: this mic is left out of recordings")
    } else if s.meters_are_mix() {
        tr("Mono mixes both mics, so this shows the mix. Switch to stereo for each mic’s own level.")
    } else {
        tr("This mic’s level, reported by the receiver")
    })
    .a11y(
        &tr("Level"),
        Some(&if inactive {
            tr("not armed")
        } else {
            trf("{percent} percent", &[("percent", &((level * 100.0) as i64).to_string())])
        }),
    );
    cy += 114.0;
    if mix {
        let t = Text::new(
            tr("mono mix"),
            theme::font(10.0, Weight::Semibold),
            theme::mute(),
        );
        let tw = t.width() + 8.0;
        b.canvas(
            &key("mix"),
            rect(x + (STRIP_W - tw) / 2.0, cy - 3.0, tw, 12.0),
        )
        .on_draw(move |r| {
            draw::fill_rect(r, &theme::face());
            t.draw_at(4.0, 0.0);
        });
        cy += 10.0;
    }
    cy += 8.0;

    // Charging state and battery code.
    let charging = if linked {
        tx.and_then(|t| t.is_charging)
    } else {
        None
    };
    let code = if linked {
        tx.and_then(|t| t.battery_level)
    } else {
        None
    };
    let cs = b.canvas(&key("battery"), rect(ix - 8.0, cy, iw + 16.0, 12.0));
    cs.on_draw(move |r| draw_charge(r, charging, code));
    cs.tip(&if code.is_none_or(|c| c == 1) {
        tr("Battery as the receiver reports it: a coarse level, not a percentage.")
    } else {
        trf(
            "The receiver reports battery code {code}, which isn’t mapped to a level yet. Changes are logged to ~/Library/Logs/Lavdeck/battery.log.",
            &[("code", &code.unwrap_or(0).to_string())],
        )
    })
    .a11y(
        &tr("Battery"),
        Some(&if charging == Some(true) { tr("charging") } else { battery_label(code).unwrap_or_else(|| tr("unknown")) }),
    );
    cy += 12.0 + 8.0;

    // Readout: dBFS while recording, else firmware, else link state.
    let (text, chip, tip) = if s.is_recording && !armed {
        (tr("not rec"), false, String::new())
    } else if let Some(db) = s.dbfs(index) {
        (
            if db <= -90 {
                tr("silent")
            } else {
                trf("{db} dBFS", &[("db", &db.to_string())])
            },
            false,
            String::new(),
        )
    } else if let Some(fw) = tx.and_then(|t| t.identity.firmware.clone()) {
        (
            format::short_firmware(&fw),
            outdated.is_some(),
            trf("Firmware {firmware}", &[("firmware", &fw)]),
        )
    } else {
        (
            if linked {
                "…".to_string()
            } else {
                tr("not linked")
            },
            false,
            String::new(),
        )
    };
    let t = Text::new(
        text,
        theme::mono_data(),
        if chip {
            theme::warn_text()
        } else {
            theme::mute()
        },
    );
    let rw = t.width() + if chip { 10.0 } else { 0.0 };
    let rh = th(11.0) + if chip { 4.0 } else { 0.0 };
    let rc = b.canvas(
        &key("readout"),
        rect(
            x + (STRIP_W - rw) / 2.0,
            cy - if chip { 2.0 } else { 0.0 },
            rw,
            rh,
        ),
    );
    rc.on_draw(move |r| {
        if chip {
            draw::fill_round(r, 4.0, &theme::warn_fill());
            t.draw_at(5.0, 2.0);
        } else {
            t.draw_at(0.0, 0.0);
        }
    });
    rc.tip(&tip);
    cy += th(11.0) + 8.0;

    let h = cy - y;
    let face = b.canvas(&key("face"), rect(x, y, STRIP_W, h));
    face.on_draw(move |r| {
        if talking {
            // A green wash over the deck, in place of the face.
            draw::fill_round(r, 9.0, &theme::alpha(&theme::green(), 0.12));
            draw::stroke_round(r, 9.0, 2.0, &theme::green());
        } else {
            draw::fill_round(r, 9.0, &theme::face());
            draw::stroke_round(r, 9.0, 0.5, &theme::hairline());
        }
    });
    send_to_back(&face);
    h
}

/// Keeps a module's face under its parts when it's made after them.
fn send_to_back(view: &objc2_app_kit::NSView) {
    let Some(parent) = (unsafe { view.superview() }) else {
        return;
    };
    let subviews = parent.subviews();
    // Just above the deck background, which is the first subview.
    if subviews.len() > 1 && !subviews.objectAtIndex(1).isEqual(Some(view)) {
        let bg = subviews.objectAtIndex(0);
        view.removeFromSuperview();
        parent.addSubview_positioned_relativeTo(
            view,
            objc2_app_kit::NSWindowOrderingMode::Above,
            Some(&bg),
        );
    }
}

/// Lit segments and the peak segment for a strip, keeping a 1.2 s peak hold.
fn meter_state(index: u8, level: f64, inactive: bool) -> (i64, i64) {
    UI.with(|u| {
        let mut peaks = u.peaks.borrow_mut();
        let (peak, at) = &mut peaks[index as usize];
        let now = format::now();
        if inactive {
            *peak = 0.0;
        } else if level >= *peak || now - *at > 1.2 {
            *peak = level;
            *at = now;
        }
        if inactive {
            return (0, -1);
        }
        (
            (level * 12.0).floor() as i64,
            (*peak * 12.0).floor() as i64 - 1,
        )
    })
}

/// Forgets peaks (snapshots draw each state fresh, as SwiftUI would).
pub fn reset_peaks() {
    UI.with(|u| {
        *u.peaks.borrow_mut() = [(0.0, 0.0); 3];
        u.lit.set([-1; 3]);
    });
}

fn begin_editing(index: u8) {
    UI.with(|u| u.editing.set(Some(index)));
    store::update(|_| {});
}

fn end_editing(index: u8, commit: Option<String>) {
    if UI.with(|u| u.editing.get()) != Some(index) {
        return;
    }
    UI.with(|u| u.editing.set(None));
    if let Some(name) = commit {
        store::rename(index, &name);
    } else {
        store::update(|_| {});
    }
}

fn rename_field(
    b: &mut Build,
    key: &str,
    index: u8,
    name: &str,
    named: bool,
    frame: NSRect,
) -> Retained<NSTextField> {
    let bg = b.canvas(&format!("{key}-bg"), frame);
    bg.on_draw(|r| {
        draw::fill_round(r, 5.0, &theme::plate());
        draw::stroke_round(r, 5.0, 1.5, &theme::green());
    });
    let field_frame = rect(
        frame.origin.x + 7.0,
        frame.origin.y + 6.0,
        frame.size.width - 14.0,
        th(12.0),
    );
    let initial = if named {
        name.to_string()
    } else {
        String::new()
    };
    b.native(key, field_frame, || {
        let f = NSTextField::new(util::mtm());
        f.setBordered(false);
        f.setDrawsBackground(false);
        f.setFocusRingType(NSFocusRingType::None);
        f.setFont(Some(&theme::label_bold()));
        f.setTextColor(Some(&theme::plate_text()));
        f.setPlaceholderString(Some(&ns(&format!("TX{index}"))));
        f.setStringValue(&ns(&initial));
        let d = FieldDelegate::new();
        *d.ivars().ended.borrow_mut() = Some(Box::new(move |text| end_editing(index, Some(text))));
        *d.ivars().cancelled.borrow_mut() = Some(Box::new(move || end_editing(index, None)));
        d.attach(&f);
        f
    })
}

/// Focuses the rename field once it's in a window.
pub fn focus_editing(layer: &Layer) {
    let Some(index) = UI.with(|u| u.editing.get()) else {
        return;
    };
    let Some(view) = layer.get(&format!("tx{index}-field")) else {
        return;
    };
    if let Some(w) = view.window()
        && w.firstResponder()
            .is_none_or(|r| !r.isKindOfClass(objc2_app_kit::NSTextView::class()))
    {
        w.makeFirstResponder(Some(&view));
    }
}

/// Ends a rename in progress (Escape in the deck).
pub fn cancel_editing() -> bool {
    match UI.with(|u| u.editing.get()) {
        Some(i) => {
            end_editing(i, None);
            true
        }
        None => false,
    }
}

/// A scribble strip: the mic's name, its slot as a tag, and while recording its file.
#[derive(Clone)]
pub struct Plate {
    title: String,
    subtitle: Option<String>,
    tag: Option<String>,
    armed: bool,
    dimmed: bool,
    /// Left out of the take: hollow so it stays readable.
    hollow: bool,
}

impl Plate {
    /// A name plate as Recordings shows a take's speakers.
    pub fn speaker(name: &str, tag: Option<String>) -> Plate {
        Plate {
            title: name.to_string(),
            subtitle: None,
            tag,
            armed: false,
            dimmed: false,
            hollow: false,
        }
    }

    /// Its natural width.
    pub fn width(&self) -> f64 {
        let title = Text::new(self.title.clone(), theme::label_bold(), theme::plate_text())
            .kern(0.5)
            .width();
        let tag = self.tag.as_ref().map_or(0.0, |t| {
            4.0 + Text::new(
                t.clone(),
                theme::mono(10.0, Weight::Semibold),
                theme::plate_sub(),
            )
            .width()
        });
        14.0 + title + tag
    }

    pub fn height(&self) -> f64 {
        12.0 + th(12.0) + self.subtitle.as_ref().map_or(0.0, |_| 1.0 + th(11.0))
    }

    pub fn draw(&self, r: NSRect) {
        let (x0, y0) = (r.origin.x, r.origin.y);
        draw::with_alpha(if self.dimmed { 0.45 } else { 1.0 }, || {
            let fill = if self.hollow {
                theme::well()
            } else if self.armed {
                theme::rec()
            } else {
                theme::plate()
            };
            draw::fill_round(r, 5.0, &fill);
            draw::stroke_round(r, 5.0, 0.5, &theme::hairline());
            let title = Text::new(
                self.title.clone(),
                theme::label_bold(),
                if self.hollow {
                    theme::ink()
                } else {
                    theme::plate_text()
                },
            )
            .kern(0.5);
            let mut title_w = r.size.width - 14.0;
            if let Some(tag) = &self.tag {
                let t = Text::new(
                    tag.clone(),
                    theme::mono(10.0, Weight::Semibold),
                    if self.hollow {
                        theme::mute()
                    } else {
                        theme::plate_sub()
                    },
                );
                let tw = t.width();
                title_w -= tw + 4.0;
                t.draw_at(
                    x0 + r.size.width - 7.0 - tw,
                    y0 + 6.0 + title.font.ascender() - t.font.ascender(),
                );
            }
            title.draw_in(rect(x0 + 7.0, y0 + 6.0, title_w, th(12.0)));
            if let Some(sub) = &self.subtitle {
                let color = if self.hollow {
                    theme::mute()
                } else if self.armed {
                    theme::plate_rec_sub()
                } else {
                    theme::plate_sub()
                };
                Text::new(sub.clone(), theme::mono_data(), color)
                    .middle()
                    .draw_in(rect(
                        x0 + 7.0,
                        y0 + 6.0 + th(12.0) + 1.0,
                        r.size.width - 14.0,
                        th(11.0),
                    ));
            }
        });
    }
}

fn draw_arm(r: NSRect, armed: bool, live: bool) {
    let red = armed && live;
    draw::fill_round(r, 6.0, &theme::well());
    if red {
        draw::stroke_round(r, 6.0, 1.0, &theme::alpha(&theme::rec(), 0.6));
    } else {
        draw::stroke_round(r, 6.0, 0.5, &theme::hairline());
    }
    let label = if red {
        tr("REC")
    } else if armed {
        tr("ARMED")
    } else {
        tr("OFF")
    };
    let color = if red {
        theme::rec_text()
    } else if armed {
        theme::ink()
    } else {
        theme::mute()
    };
    let t = Text::new(label, theme::caps(), color).kern(0.6);
    let w = 8.0 + 5.0 + t.width();
    let x = ((r.size.width - w) / 2.0).px();
    let dot = rect(x, 4.0 + 7.0 - 4.0, 8.0, 8.0);
    if red {
        draw::fill_oval(dot, &theme::rec());
    } else if armed {
        draw::fill_oval(dot, &theme::ink());
    } else {
        draw::stroke_oval(dot, 1.0, &theme::mute());
    }
    t.draw_at(x + 13.0, 4.0);
}

fn draw_led(r: NSRect, link: Link) {
    let c = (r.size.width / 2.0, r.size.height / 2.0);
    let disc = |d: f64| rect(c.0 - d / 2.0, c.1 - d / 2.0, d, d);
    match link {
        Link::Linked => {
            draw::fill_oval(disc(13.0), &theme::alpha(&theme::green(), 0.18));
            draw::fill_oval(disc(9.0), &theme::green());
        }
        Link::Dropped => draw::stroke_oval(disc(10.0), 2.0, &theme::amber()),
        Link::Off => draw::fill_oval(disc(9.0), &theme::led_off()),
    }
}

fn segment_color(i: i64) -> Retained<NSColor> {
    if i >= 10 {
        theme::red()
    } else if i >= 8 {
        theme::amber()
    } else {
        theme::green()
    }
}

fn draw_meter(r: NSRect, lit: i64, peak: i64, inactive: bool) {
    draw::with_alpha(if inactive { 0.5 } else { 1.0 }, || {
        draw::fill_round(r, 5.0, &theme::well());
        draw::stroke_round(r, 5.0, 0.5, &theme::hairline());
        for i in 0..12_i64 {
            let y = 4.0 + (11 - i) as f64 * 9.0;
            let on = i < lit || i == peak;
            let color = if on {
                segment_color(i)
            } else {
                theme::led_off()
            };
            draw::fill_round(rect(4.0, y, 26.0, 7.0), 1.5, &color);
        }
    });
}

/// The receiver's battery code as a word: code 1 is confirmed "High"; others aren't mapped yet.
fn battery_label(code: Option<i32>) -> Option<String> {
    match code {
        Some(1) => Some(tr("High")),
        Some(c) => Some(trf("Level {code}", &[("code", &c.to_string())])),
        None => None,
    }
}

fn draw_charge(r: NSRect, charging: Option<bool>, code: Option<i32>) {
    let font = theme::font(10.0, Weight::Semibold);
    let color = if charging == Some(true) {
        theme::green()
    } else {
        theme::mute()
    };
    let (symbol, size, weight, text) = if charging == Some(true) {
        (Some("bolt.fill"), 9.0, Weight::Bold, tr("Charging"))
    } else if let (Some(label), Some(_)) = (battery_label(code), charging) {
        (
            Some(if code == Some(1) {
                "battery.100percent"
            } else {
                "battery.50percent"
            }),
            10.0,
            Weight::Medium,
            label,
        )
    } else {
        (None, 0.0, Weight::Regular, "—".to_string())
    };
    let t = Text::new(text, font, color.clone());
    let sym = symbol
        .map(|s| draw::symbol_size(s, size, weight))
        .map_or(0.0, |s| s.width);
    let w = if symbol.is_some() { sym + 3.0 } else { 0.0 } + t.width();
    let x = ((r.size.width - w) / 2.0).px();
    if let Some(name) = symbol {
        draw::draw_symbol(name, size, weight, &color, x + sym / 2.0, 6.0);
    }
    t.draw_at(x + if symbol.is_some() { sym + 3.0 } else { 0.0 }, 0.0);
}

// MARK: Master section

fn master(b: &mut Build, s: &AppState, x: f64, y: f64) -> f64 {
    let w = W - PAD - x;
    let ix = x + 8.0;
    let iw = w - 16.0;
    let mut cy = y + 8.0;
    if let (true, Some(started)) = (s.is_recording, s.recording_started) {
        let elapsed = (format::now() - started).max(0.0).floor();
        let counter = Text::new(
            format::elapsed(elapsed),
            theme::digits(20.0, Weight::Semibold),
            theme::ink(),
        );
        let since = Text::new(
            trf("started {time}", &[("time", &format::time(started))]),
            theme::label(),
            theme::mute(),
        );
        let h = th(20.0) + 1.0 + th(12.0);
        b.canvas("master-clock", rect(ix, cy, iw, h))
            .on_draw(move |r| {
                counter.draw_at(((r.size.width - counter.width()) / 2.0).px(), 0.0);
                since.draw_at(((r.size.width - since.width()) / 2.0).px(), th(20.0) + 1.0);
            });
        cy += h + 10.0;
    } else {
        let gain = Text::new(tr("GAIN"), theme::caps(), theme::mute()).kern(0.9);
        b.canvas("master-gain-label", rect(ix, cy, iw, th(11.0)))
            .on_draw(move |_| gain.draw_at(0.0, 0.0));
        cy += th(11.0) + 10.0;
    }

    // Gain fader. The capture row takes its height from it, so the section lines up with the strips.
    let fader_h = if s.is_recording { 84.0 } else { 96.0 };
    fader(b, s, rect(ix, cy, iw, fader_h));
    cy += fader_h + 10.0;

    // MONO | STEREO.
    let mode = s.displayed_channel();
    let disabled = s.is_recording || s.pending_channel.is_some();
    let sw_h = th(11.0) + 12.0;
    let half = iw / 2.0;
    for (i, (label, target)) in [
        (tr("MONO"), ChannelMode::Mono),
        (tr("STEREO"), ChannelMode::Stereo),
    ]
    .into_iter()
    .enumerate()
    {
        let on = mode == Some(target);
        let c = b.canvas(
            &format!("mode-{i}"),
            rect(ix + half * i as f64, cy, half, sw_h),
        );
        let first = i == 0;
        c.on_draw(move |r| {
            draw::with_alpha(if disabled && !on { 0.45 } else { 1.0 }, || {
                // Each segment is half of one rounded switch.
                let whole = if first {
                    rect(0.0, 0.0, r.size.width * 2.0, r.size.height)
                } else {
                    rect(-r.size.width, 0.0, r.size.width * 2.0, r.size.height)
                };
                let fill = if on { theme::ink() } else { theme::well() };
                draw::fill_round(whole, 7.0, &fill);
                let t = Text::new(
                    label.clone(),
                    theme::caps(),
                    if on { theme::on_ink() } else { theme::mute() },
                )
                .kern(0.7);
                t.draw_at(((r.size.width - t.width()) / 2.0).px(), 6.0);
            });
        });
        if !disabled {
            c.on_click(move || store::set_channel(target));
        }
        c.tip(&if s.is_recording {
            tr("Stop recording to change the channel mode")
        } else {
            tr("Mono mixes both mics; stereo puts TX1 left and TX2 right")
        });
    }
    b.canvas("mode-border", rect(ix, cy, iw, sw_h))
        .on_draw(|r| draw::stroke_round(r, 7.0, 0.5, &theme::hairline()));
    cy += sw_h + 10.0;

    // Capture switches: Lavdeck's own screen and camera, then any capture plugin's sources.
    let toggles = s.plugins.capture_toggles();
    let mut items: Vec<CaptureItem> = vec![
        CaptureItem {
            id: "lavdeck/screen".into(),
            source: "screen".into(),
            on: s.settings.video.screen,
            live: s.capture_sources.iter().any(|c| c == "screen"),
            toggle: None,
        },
        CaptureItem {
            id: "lavdeck/camera".into(),
            source: "camera".into(),
            on: s.settings.video.camera,
            live: s.capture_sources.iter().any(|c| c == "camera"),
            toggle: None,
        },
    ];
    for t in toggles {
        items.push(CaptureItem {
            id: t.id(),
            source: t.source.clone(),
            on: s.plugins.is_on(&t),
            live: s.capture_sources.contains(&t.source),
            toggle: Some(t),
        });
    }
    let n = items.len() as f64;
    let cw = (iw - 6.0 * (n - 1.0)) / n;
    let ch = th(10.0) + 12.0;
    let locked = s.is_recording;
    let mut keys = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let key = format!("capture-{}", item.id);
        keys.push(key.clone());
        let c = b.canvas(&key, rect(ix + i as f64 * (cw + 6.0), cy, cw, ch));
        let title = source_title(&item.source);
        let it = item.clone();
        c.on_draw(move |r| draw_capture(r, &it, locked));
        if !locked {
            let it = item.clone();
            c.on_click(move || match &it.toggle {
                Some(t) => store::set_capture(t, !it.on),
                None => store::set_video_source(&it.source, !it.on),
            });
        }
        c.tip(&if locked {
            tr("Stop recording to change what’s captured")
        } else if item.on {
            trf(
                "{source} records with each take. Click to turn it off.",
                &[("source", &title)],
            )
        } else {
            trf(
                "Click to record the {source} with each take.",
                &[("source", &title.to_lowercase())],
            )
        })
        .a11y(
            &trf("Record {source}", &[("source", &title.to_lowercase())]),
            Some(&if item.on { tr("on") } else { tr("off") }),
        );
    }
    cy += ch + 10.0;

    // Record / Stop.
    let recording = s.is_recording;
    let disabled = !recording && s.settings.armed.is_empty();
    let rh = 8.0 + 31.0;
    let rb = b.canvas("record", rect(ix, cy, iw, rh));
    rb.on_draw(move |r| draw_record(r, recording, disabled));
    if !disabled {
        rb.on_click(store::toggle_recording);
    }
    rb.tip(&if disabled {
        tr("Arm at least one mic to record")
    } else {
        String::new()
    })
    .a11y(
        &if recording {
            tr("Stop recording")
        } else {
            tr("Start recording")
        },
        None,
    );
    cy += rh + 8.0;

    let h = cy - y;
    let face = b.canvas("master-face", rect(x, y, w, h));
    face.on_draw(|r| {
        draw::fill_round(r, 9.0, &theme::face());
        draw::stroke_round(r, 9.0, 0.5, &theme::hairline());
    });
    send_to_back(&face);
    h
}

fn fader(b: &mut Build, s: &AppState, frame: NSRect) {
    let value = s.displayed_gain();
    let pending = s.pending_gain.is_some();
    let n = GAIN_STEPS.len();
    let h = frame.size.height;
    let drag = UI.with(|u| u.drag_index.get());
    let index = drag
        .or_else(|| value.and_then(|v| GAIN_STEPS.iter().position(|s| *s == v)))
        .unwrap_or(n / 2);
    // Padded so the cap's shadow isn't clipped.
    const M: f64 = 8.0;
    let track = b.canvas(
        "fader",
        rect(
            frame.origin.x - M,
            frame.origin.y - M,
            34.0 + 2.0 * M,
            h + 2.0 * M,
        ),
    );
    track.on_draw(move |_| {
        draw::fill_round(rect(M + 14.0, M, 6.0, h), 3.0, &theme::led_off());
        let y = M + h * index as f64 / (n - 1) as f64;
        let cap = rect(M, y - 8.0, 34.0, 16.0);
        draw::with_alpha(if pending { 0.55 } else { 1.0 }, || {
            // The only shadow in the design: the one object that stands proud of the panel.
            draw::with_shadow(2.0, 4.0, &theme::alpha(&NSColor::blackColor(), 0.3), || {
                draw::fill_round(cap, 4.0, &theme::hex(0x121416));
            });
            let path = objc2_app_kit::NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                cap, 4.0, 4.0,
            );
            let gradient = objc2_app_kit::NSGradient::initWithStartingColor_endingColor(
                objc2_app_kit::NSGradient::alloc(),
                &theme::hex(0x2A2E32),
                &theme::hex(0x121416),
            );
            if let Some(g) = gradient {
                // Flipped view: 90° runs top to bottom.
                g.drawInBezierPath_angle(&path, 90.0);
            }
            draw::fill_round(
                rect(M + 5.0, y - 1.0, 24.0, 2.0),
                1.0,
                &NSColor::whiteColor(),
            );
        });
    });
    track
        .on_drag(move |phase, p| {
            let i = (((p.y - M) / h.max(1.0)) * (n - 1) as f64)
                .round()
                .clamp(0.0, (n - 1) as f64) as usize;
            match phase {
                DragPhase::Began | DragPhase::Moved => {
                    if UI.with(|u| u.drag_index.replace(Some(i))) != Some(i) {
                        let performer = NSHapticFeedbackManager::defaultPerformer();
                        performer.performFeedbackPattern_performanceTime(
                            NSHapticFeedbackPattern::Alignment,
                            NSHapticFeedbackPerformanceTime::Now,
                        );
                        store::update(|_| {});
                    }
                }
                DragPhase::Ended => {
                    if let Some(i) = UI.with(|u| u.drag_index.take()) {
                        store::set_gain(GAIN_STEPS[i]);
                    }
                    store::update(|_| {});
                }
            }
        })
        .a11y(
            &tr("Receiver gain"),
            Some(&value.map_or_else(
                || tr("unknown"),
                |v| trf("{db} decibels", &[("db", &v.to_string())]),
            )),
        );
    // The scale: each step is also a click target.
    let lh = th(12.0);
    for (i, step) in GAIN_STEPS.iter().copied().enumerate() {
        let selected = Some(step) == value;
        let t = Text::new(
            format::gain_step(step),
            theme::digits(
                12.0,
                if selected {
                    Weight::Semibold
                } else {
                    Weight::Regular
                },
            ),
            if selected {
                theme::ink()
            } else {
                theme::mute()
            },
        );
        let ty = frame.origin.y + (h - lh) * i as f64 / (n - 1) as f64;
        let c = b.canvas(
            &format!("fader-step-{i}"),
            rect(frame.origin.x + 34.0 + 18.0, ty, t.width() + 4.0, lh),
        );
        c.on_draw(move |_| t.draw_at(0.0, 0.0))
            .on_click(move || store::set_gain(step));
    }
}

#[derive(Clone)]
struct CaptureItem {
    id: String,
    source: String,
    on: bool,
    live: bool,
    toggle: Option<CaptureToggle>,
}

fn capture_symbol(source: &str, on: bool) -> &'static str {
    match source {
        "screen" => "display",
        "camera" if on => "video.fill",
        "camera" => "video.slash",
        "system-audio" if on => "speaker.wave.2.fill",
        "system-audio" => "speaker.slash",
        _ => "record.circle",
    }
}

fn draw_capture(r: NSRect, item: &CaptureItem, locked: bool) {
    let red = locked && item.live;
    let filled = item.on && !locked;
    let fg = if red {
        theme::rec_text()
    } else if filled {
        theme::on_ink()
    } else if item.on {
        theme::ink()
    } else {
        theme::mute()
    };
    draw::with_alpha(if locked && !item.on { 0.45 } else { 1.0 }, || {
        let fill = if filled { theme::ink() } else { theme::well() };
        draw::fill_round(r, 7.0, &fill);
        if red {
            draw::stroke_round(r, 7.0, 1.0, &theme::alpha(&theme::rec(), 0.6));
        } else {
            draw::stroke_round(r, 7.0, 0.5, &theme::hairline());
        }
        // The widest of icon and label, label alone, or icon alone that fits.
        let t = Text::new(
            source_title(&item.source).to_uppercase(),
            theme::font(10.0, Weight::Bold),
            fg.clone(),
        )
        .kern(0.3);
        let symbol = capture_symbol(&item.source, item.on);
        let small = draw::symbol_size(symbol, 9.0, Weight::Bold);
        let both = small.width + 3.0 + t.width();
        let mid = r.size.height / 2.0;
        if both <= r.size.width {
            let x = ((r.size.width - both) / 2.0).px();
            draw::draw_symbol(symbol, 9.0, Weight::Bold, &fg, x + small.width / 2.0, mid);
            t.draw_at(x + small.width + 3.0, 6.0);
        } else if t.width() <= r.size.width {
            t.draw_at(((r.size.width - t.width()) / 2.0).px(), 6.0);
        } else {
            draw::draw_symbol(symbol, 12.0, Weight::Semibold, &fg, r.size.width / 2.0, mid);
        }
    });
}

fn draw_record(r: NSRect, recording: bool, disabled: bool) {
    draw::fill_rect(rect(0.0, 0.0, r.size.width, 0.5), &theme::hairline());
    draw::with_alpha(if disabled { 0.45 } else { 1.0 }, || {
        let top = 8.0;
        let stack = 31.0;
        let disc_y = top + (stack - 26.0) / 2.0;
        if recording {
            draw::fill_round(rect(2.0, disc_y + 2.0, 22.0, 22.0), 6.0, &theme::ink());
        } else {
            let disc = rect(0.0, disc_y, 26.0, 26.0);
            draw::fill_oval(disc, &theme::rec());
            draw::stroke_oval(disc, 1.0, &theme::alpha(&NSColor::blackColor(), 0.15));
        }
        let title = Text::new(
            if recording { tr("Stop") } else { tr("Record") },
            theme::label_bold(),
            theme::ink(),
        );
        title.draw_at(35.0, top);
        Text::new("⌥⌘R", theme::label(), theme::mute()).draw_at(35.0, top + th(12.0) + 1.0);
    });
}

// MARK: Footer and notes

/// "High", "Low", "Off", "…" for the receiver's O / L / H noise-cancelling switch.
fn noise_cancel_label(s: &AppState) -> String {
    let rx = &s.status.receiver;
    match (rx.noise_cancelling, rx.noise_cancelling_level) {
        (None, _) => "…".into(),
        (Some(false), _) => tr("Off"),
        (Some(true), Some(0)) => tr("Low"),
        (Some(true), Some(1)) => tr("High"),
        (Some(true), Some(l)) => trf("On ({level})", &[("level", &l.to_string())]),
        (Some(true), None) => tr("On"),
    }
}

fn footer(b: &mut Build, s: &AppState, y: f64, links: &Links) -> f64 {
    let x = PAD + 4.0;
    let w = W - 2.0 * x;
    let h = th(12.0);
    let lock = draw::symbol_size("lock", 10.0, Weight::Semibold);
    let nc = Text::new(tr("Noise cancel"), theme::label(), theme::mute());
    let value = Text::new(noise_cancel_label(s), theme::label_bold(), theme::ink());
    let left_w = lock.width + 5.0 + nc.width() + 5.0 + value.width();
    b.canvas("footer-nc", rect(x, y, left_w, h))
        .on_draw(move |_| {
            draw::draw_symbol(
                "lock",
                10.0,
                Weight::Semibold,
                &theme::mute(),
                lock.width / 2.0,
                7.5,
            );
            nc.draw_at(lock.width + 5.0, 0.0);
            value.draw_at(lock.width + 5.0 + nc.width() + 5.0, 0.0);
        })
        .tip(&tr("Set with the O/L/H switch on the receiver"));
    let gear = draw::symbol_size("gearshape", 12.0, Weight::Regular);
    let gx = x + w - gear.width;
    b.canvas("footer-gear", rect(gx, y, gear.width, h))
        .on_draw(move |r| {
            draw::draw_symbol(
                "gearshape",
                12.0,
                Weight::Regular,
                &theme::mute(),
                r.size.width / 2.0,
                7.5,
            );
        })
        .on_click(links.settings)
        .tip(&tr("Settings"))
        .a11y(&tr("Settings"), None);
    let format = Text::new(
        if s.settings.format == RecordingFormat::Wav {
            tr("WAV 24-bit")
        } else {
            tr("M4A")
        },
        theme::label_bold(),
        theme::ink(),
    );
    let split = s
        .settings
        .split_tracks
        .then(|| Text::new(tr("· split tracks"), theme::label(), theme::mute()));
    let stack = draw::symbol_size("rectangle.stack", 11.0, Weight::Regular);
    let fw = format.width() + split.as_ref().map_or(0.0, |t| 4.0 + t.width()) + 4.0 + stack.width;
    b.canvas("footer-format", rect(gx - 8.0 - fw, y, fw, h))
        .on_draw(move |r| {
            format.draw_at(0.0, 0.0);
            let mut px = format.width() + 4.0;
            if let Some(t) = &split {
                t.draw_at(px, 0.0);
                px += t.width() + 4.0;
            }
            let _ = r;
            draw::draw_symbol(
                "rectangle.stack",
                11.0,
                Weight::Regular,
                &theme::mute(),
                px + stack.width / 2.0,
                7.5,
            );
        })
        .on_click(links.recordings)
        .tip(&tr("Open Recordings"))
        .a11y(&tr("Open Recordings"), None);
    y + h
}

/// A warning-parchment note below the footer.
struct Note {
    text: String,
    symbol: &'static str,
    action: Option<(String, Box<dyn Fn()>)>,
    dismiss: Option<Box<dyn Fn()>>,
}

fn notes(b: &mut Build, s: &AppState, mut y: f64, links: &Links) -> f64 {
    // Running plugin jobs, e.g. a transcript being made for the last take.
    for job in s.plugins.running_jobs() {
        y += PAD;
        y = job_note(b, job, s.plugins.queued_count(&job.plugin_id), y, links);
    }
    let mut list: Vec<Note> = Vec::new();
    // Only jobs that ran on their own after a take; a failure of one started in Recordings shows there.
    if let Some(failed) = s.plugins.automatic_failure() {
        let (id, take) = (failed.id, failed.take_id.clone());
        let open = links.recordings;
        list.push(Note {
            text: trf(
                "{plugin} couldn’t finish the last take: {reason}",
                &[
                    ("plugin", &failed.plugin_name),
                    ("reason", failed.failure().unwrap_or_default()),
                ],
            ),
            symbol: "exclamationmark.triangle.fill",
            action: Some((
                tr("Show"),
                Box::new(move || {
                    let take = take.clone();
                    store::update(|s| s.focus_take_id = Some(take));
                    open();
                }),
            )),
            dismiss: Some(Box::new(move || store::dismiss_job(id))),
        });
    }
    if let Some(error) = &s.last_error {
        list.push(Note {
            text: error.clone(),
            symbol: "exclamationmark.triangle.fill",
            action: None,
            dismiss: None,
        });
    }
    for i in s.dropped_transmitters() {
        list.push(Note {
            text: trf(
                "TX{index} disconnected from the receiver. It reconnects on its own when back in range or powered on.",
                &[("index", &i.to_string())],
            ),
            symbol: "antenna.radiowaves.left.and.right.slash",
            action: None,
            dismiss: None,
        });
    }
    for i in s.low_battery() {
        list.push(Note {
            text: trf(
                "TX{index} battery looks low (receiver estimate). Charge it soon.",
                &[("index", &i.to_string())],
            ),
            symbol: "battery.25percent",
            action: None,
            dismiss: None,
        });
    }
    if let Some(solo) = s.solo_mic()
        && !s.is_recording
        && s.channel_mode() != Some(ChannelMode::Stereo)
    {
        list.push(Note {
            text: trf(
                "Only TX{index} is armed. Recording will switch the receiver to stereo so that mic records alone.",
                &[("index", &solo.to_string())],
            ),
            symbol: "info.circle.fill",
            action: None,
            dismiss: None,
        });
    }
    for o in s.outdated_firmware() {
        list.push(Note {
            text: trf(
                "TX{index} firmware {firmware} is behind TX{newest_index} ({newest}). Update it in DJI Mimo.",
                &[
                    ("index", &o.index.to_string()),
                    ("firmware", &o.firmware),
                    ("newest_index", &o.newest_index.to_string()),
                    ("newest", &o.newest),
                ],
            ),
            symbol: "arrow.down.circle.fill",
            action: None,
            dismiss: None,
        });
    }
    for (i, note) in list.into_iter().enumerate() {
        y += PAD;
        y = note_view(b, &format!("note-{i}"), note, y);
    }
    y
}

fn note_view(b: &mut Build, key: &str, note: Note, y: f64) -> f64 {
    let x = PAD;
    let w = W - 2.0 * PAD;
    let sym = draw::symbol_size(note.symbol, 11.0, Weight::Regular);
    let action = note
        .action
        .as_ref()
        .map(|(t, _)| Text::new(t.clone(), theme::label_bold(), NSColor::linkColor()));
    let action_w = action.as_ref().map_or(0.0, |t| t.width() + 7.0);
    let x_w = if note.dismiss.is_some() {
        draw::symbol_size("xmark", 10.0, Weight::Bold).width + 7.0
    } else {
        0.0
    };
    let text_x = 9.0 + sym.width + 7.0;
    let text_w = w - text_x - 9.0 - action_w - x_w;
    let text = Text::new(note.text.clone(), theme::label(), theme::warn_text()).wrapping();
    let th_ = text.wrapped_height(text_w).max(th(12.0));
    let h = 7.0 + th_ + 7.0;
    let symbol = note.symbol;
    b.canvas(key, rect(x, y, w, h)).on_draw(move |r| {
        draw::fill_round(r, 7.0, &theme::warn_fill());
        draw::draw_symbol(
            symbol,
            11.0,
            Weight::Regular,
            &theme::warn_text(),
            9.0 + sym.width / 2.0,
            7.0 + 7.5,
        );
        text.draw_in(rect(text_x, 7.0, text_w, th_));
    });
    let mut right = x + w - 9.0;
    if let Some(dismiss) = note.dismiss {
        let xw = draw::symbol_size("xmark", 10.0, Weight::Bold).width;
        right -= xw;
        b.canvas(&format!("{key}-x"), rect(right, y + 7.0, xw, th(12.0)))
            .on_draw(move |r| {
                draw::draw_symbol(
                    "xmark",
                    10.0,
                    Weight::Bold,
                    &theme::warn_text(),
                    r.size.width / 2.0,
                    7.5,
                );
            })
            .on_click(dismiss)
            .tip(&tr("Dismiss"))
            .a11y(&tr("Dismiss"), None);
        right -= 7.0;
    }
    if let (Some((_, run)), Some(t)) = (note.action, action) {
        let tw = t.width();
        right -= tw;
        b.canvas(&format!("{key}-action"), rect(right, y + 7.0, tw, th(12.0)))
            .on_draw(move |_| t.draw_at(0.0, 0.0))
            .on_click(run);
    }
    y + h
}

fn job_note(b: &mut Build, job: &Job, queued: usize, y: f64, links: &Links) -> f64 {
    let key = format!("job-{}", job.id);
    let x = PAD;
    let w = W - 2.0 * PAD;
    let title = Text::new(job.title.clone(), theme::label_bold(), theme::ink());
    let base = job.message.clone().unwrap_or_else(|| tr("Working"));
    let message = if queued > 0 {
        format!(
            "{base} · {}",
            trn(queued as u64, "{n} more queued", "{n} more queued")
        )
    } else {
        base
    };
    let message = Text::new(message, theme::label(), theme::mute());
    let h = 8.0 + th(12.0) + 1.0 + th(12.0) + 8.0;
    let show = Text::new(tr("Show"), theme::label(), NSColor::linkColor());
    let show_w = show.width();
    let xw = draw::symbol_size("xmark.circle.fill", 12.0, Weight::Regular).width;
    let text_w = w - 8.0 - 16.0 - 8.0 - 8.0 - show_w - 8.0 - xw - 8.0;
    b.canvas(&key, rect(x, y, w, h)).on_draw(move |r| {
        draw::fill_round(r, 8.0, &theme::face());
        title.draw_in(rect(32.0, 8.0, text_w, th(12.0)));
        message.draw_in(rect(32.0, 8.0 + th(12.0) + 1.0, text_w, th(12.0)));
    });
    let fraction = job.fraction;
    let p = b.native(
        &format!("{key}-progress"),
        rect(x + 8.0, y + (h - 16.0) / 2.0, 16.0, 16.0),
        || {
            let p = NSProgressIndicator::new(util::mtm());
            p.setStyle(NSProgressIndicatorStyle::Spinning);
            p.setControlSize(NSControlSize::Small);
            p.setIndeterminate(false);
            p.setMaxValue(1.0);
            p
        },
    );
    p.setDoubleValue(fraction);
    let open = links.recordings;
    b.canvas(
        &format!("{key}-show"),
        rect(
            x + w - 8.0 - xw - 8.0 - show_w,
            y + (h - th(12.0)) / 2.0,
            show_w,
            th(12.0),
        ),
    )
    .on_draw(move |_| show.draw_at(0.0, 0.0))
    .on_click(open);
    let id = job.id;
    b.canvas(
        &format!("{key}-cancel"),
        rect(x + w - 8.0 - xw, y + (h - th(12.0)) / 2.0, xw, th(12.0)),
    )
    .on_draw(move |r| {
        draw::draw_symbol(
            "xmark.circle.fill",
            12.0,
            Weight::Regular,
            &theme::mute(),
            r.size.width / 2.0,
            7.5,
        );
    })
    .on_click(move || store::cancel_job(id))
    .tip(&tr("Cancel"));
    let _ = JobState::Running;
    y + h
}

fn absent(b: &mut Build, y: f64, links: &Links) -> f64 {
    let x = PAD;
    let w = W - 2.0 * PAD;
    let title = Text::new(
        tr("Receiver not connected"),
        theme::label_bold(),
        theme::ink(),
    );
    let detail = Text::new(
        tr("Plug the DJI Mic receiver into this Mac. A hub needs to carry USB data, not just power."),
        theme::label(),
        theme::mute(),
    )
    .wrapping();
    let dh = detail.wrapped_height(w - 24.0);
    let link = Text::new(tr("Open Recordings"), theme::label(), NSColor::linkColor());
    let lw = link.width();
    let h = 12.0 + th(12.0) + 6.0 + dh + 6.0 + 2.0 + th(12.0) + 12.0;
    b.canvas("absent", rect(x, y, w, h)).on_draw(move |r| {
        draw::fill_round(r, 9.0, &theme::face());
        title.draw_at(12.0, 12.0);
        detail.draw_in(rect(12.0, 12.0 + th(12.0) + 6.0, r.size.width - 24.0, dh));
    });
    b.canvas(
        "absent-open",
        rect(x + 12.0, y + h - 12.0 - th(12.0), lw, th(12.0)),
    )
    .on_draw(move |_| link.draw_at(0.0, 0.0))
    .on_click(links.recordings);
    y + h
}
