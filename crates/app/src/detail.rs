//! The right side of Recordings: one take (its header and buttons, the player, its files and
//! its transcript), several takes (what they add up to, and actions for all of them), or an
//! empty state. Laid out top-down in code like the deck, with native buttons and the AVKit
//! player view where the Swift app used stock controls.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSBezelStyle, NSButton, NSCellImagePosition, NSColor,
    NSControlSize, NSImage, NSMenu, NSMenuItem, NSPopUpButton, NSProgressIndicator,
    NSProgressIndicatorStyle, NSScrollView, NSSharingServicePicker, NSSlider, NSView,
};
use objc2_av_kit::{AVPlayerView, AVPlayerViewControlsStyle};
use objc2_foundation::{
    NSArray, NSNotification, NSNotificationCenter, NSPoint, NSRect, NSRectEdge,
};

use crate::canvas::{Build, Canvas, Layer, item, on_action};
use crate::deck::Plate;
use crate::draw::{self, Text, rect};
use crate::i18n::{tr, trf, trn};
use crate::model::plugins::{Job, JobState, VIDEO_JOB_ID};
use crate::model::recordings as list;
use crate::model::takes::{MediaKind, Role, Take, TakeFile, TakeMedia, TranscriptDocument};
use crate::recordings::{self, selected_takes};
use crate::theme::{self, Weight};
use crate::util::{self, ns};
use crate::{format, player, store};

const PAD: f64 = 16.0;

/// The pane's views: a fixed header, a rule, and a scrolling body.
pub struct Pane {
    pub root: Retained<NSView>,
    header: Layer,
    rule: Retained<Canvas>,
    scroll: Retained<NSScrollView>,
    body: Layer,
    overlay: Layer,
}

/// What the pane shows, to tell a structural change from a playhead tick.
#[derive(Default)]
struct Ui {
    /// The transcript scrolls with playback until the user scrolls it.
    following: Cell<bool>,
    /// The transcript line under the playhead.
    current: RefCell<Option<String>>,
    /// The take on screen, to reset following when it changes.
    take: RefCell<Option<String>>,
    /// Where each transcript line sits in the body, by segment id.
    lines: RefCell<Vec<(String, f64, f64)>>,
    /// The scrubber's value while it's being dragged.
    scrubbing: Cell<Option<f64>>,
    hover: RefCell<Option<String>>,
}

thread_local! {
    static UI: Ui = Ui { following: Cell::new(true), ..Default::default() };
}

impl Pane {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Pane {
        let root = Canvas::new(mtm, frame);
        root.on_resize(|| util::later(0.0, render));
        root.on_draw(|r| draw::fill_rect(r, &NSColor::windowBackgroundColor()));
        let header_view = Canvas::new(mtm, rect(0.0, 0.0, frame.size.width, 80.0));
        root.addSubview(&header_view);
        let rule = Canvas::new(mtm, rect(0.0, 80.0, frame.size.width, 1.0));
        rule.on_draw(|r| draw::fill_rect(r, &NSColor::separatorColor()));
        root.addSubview(&rule);
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(0.0, 81.0, frame.size.width, frame.size.height - 81.0),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        let body_view = Canvas::new(mtm, rect(0.0, 0.0, frame.size.width, 100.0));
        scroll.setDocumentView(Some(&body_view));
        root.addSubview(&scroll);
        let overlay_view = Canvas::new(mtm, rect(0.0, 0.0, 0.0, 0.0));
        root.addSubview(&overlay_view);
        // Scrolling the transcript by hand stops it following playback.
        let block = RcBlock::new(|_n: NonNull<NSNotification>| UI.with(|u| u.following.set(false)));
        // SAFETY: the observer lives as long as the app; the block only flips a flag.
        unsafe {
            let _ = NSNotificationCenter::defaultCenter()
                .addObserverForName_object_queue_usingBlock(
                    Some(objc2_app_kit::NSScrollViewWillStartLiveScrollNotification),
                    Some(&scroll),
                    None,
                    &block,
                );
        }
        Pane {
            root: Retained::into_super(root),
            header: Layer::new(Retained::into_super(header_view)),
            rule,
            scroll,
            body: Layer::new(Retained::into_super(body_view)),
            overlay: Layer::new(Retained::into_super(overlay_view)),
        }
    }
}

/// What the pane shows now.
enum Mode {
    One(Take),
    Several(Vec<Take>),
    Empty,
    Nothing,
}

/// Rebuilds the pane from the window's state, the store and the player.
pub fn render() {
    let Some((mode, transcript, search)) = recordings::with(|s| {
        let selected = selected_takes(s);
        let mode = match selected.len() {
            1 => Mode::One(selected[0].clone()),
            n if n > 1 => Mode::Several(selected),
            _ if s.loaded && s.takes.is_empty() => Mode::Empty,
            _ => Mode::Nothing,
        };
        let transcript = match &mode {
            Mode::One(t) => s.transcripts.get(&t.id).cloned(),
            _ => None,
        };
        (mode, transcript, s.search.clone())
    }) else {
        return;
    };
    let Some(root) = recordings::with(|s| s.detail.root.clone()) else {
        return;
    };
    let width = root.frame().size.width;
    let height = root.frame().size.height;
    // A different take starts following playback again and loads its best media.
    let take_id = match &mode {
        Mode::One(t) => Some(t.id.clone()),
        _ => None,
    };
    let changed = UI.with(|u| u.take.replace(take_id.clone()) != take_id);
    if changed {
        UI.with(|u| u.following.set(true));
        if let Mode::One(t) = &mode {
            load_default(t);
        }
    } else if let Mode::One(t) = &mode {
        // Keep a source the user picked unless a finished video has just appeared.
        let media = TakeMedia::all(t);
        let current = player::with(|p| p.media.clone());
        let best = media.first();
        let keep = current.as_ref().is_some_and(|c| media.contains(c));
        if best.is_some()
            && (!keep
                || (best.is_some_and(|b| b.kind == MediaKind::Video)
                    && current
                        .as_ref()
                        .is_some_and(|c| c.kind != MediaKind::Video && c.is_video())))
        {
            load_default(t);
        }
    }

    let Some((p, transcripts)) = recordings::with(|s| (s.detail.clone(), s.transcripts.clone()))
    else {
        return;
    };
    let mut hb = p.header.build();
    let mut bb = p.body.build();
    let mut ob = p.overlay.build();
    let header_h = match &mode {
        Mode::One(take) => take_header(&mut hb, take, transcript.as_ref(), width),
        Mode::Several(takes) => several_header(&mut hb, takes, width),
        _ => 0.0,
    };
    let body_h = match &mode {
        Mode::One(take) => take_body(&mut bb, take, transcript.as_ref(), &search, width),
        Mode::Several(takes) => several_body(&mut bb, takes, &transcripts, width),
        Mode::Empty => {
            unavailable(
                &mut ob,
                "waveform",
                &tr("No recordings yet"),
                Some(&tr(
                    "Press ⌥⌘R, or Record in the menu bar, with the receiver plugged in. Takes appear here as soon as they stop.",
                )),
                width,
                height,
            );
            0.0
        }
        Mode::Nothing => {
            unavailable(
                &mut ob,
                "waveform",
                &tr("Select a recording"),
                None,
                width,
                height,
            );
            0.0
        }
    };
    if let (Mode::One(take), true) = (&mode, transcript.is_some()) {
        follow_button(&mut ob, take, width, height);
    }
    hb.finish();
    bb.finish();
    ob.finish();
    let has_header = header_h > 0.0;
    p.header.root.setFrame(rect(0.0, 0.0, width, header_h));
    p.rule.setHidden(!has_header);
    p.rule.setFrame(rect(0.0, header_h, width, 1.0));
    let top = if has_header { header_h + 1.0 } else { 0.0 };
    p.scroll.setHidden(!has_header);
    p.scroll
        .setFrame(rect(0.0, top, width, (height - top).max(0.0)));
    let content = p.scroll.contentSize();
    p.body
        .root
        .setFrame(rect(0.0, 0.0, content.width, body_h.max(content.height)));
    p.overlay.root.setFrame(rect(0.0, 0.0, width, height));
    tick();
}

fn load_default(take: &Take) {
    match TakeMedia::all(take).first() {
        Some(best) => player::load(best, take, false),
        None => player::stop(),
    }
}

/// Updates what follows the playhead: the scrubber and the transcript line, and scrolls the
/// transcript along unless the user is reading elsewhere.
pub fn tick() {
    let time = player::current_time_if_started();
    let current = recordings::with(|s| {
        let take = selected_takes(s).into_iter().next()?;
        let doc = s.transcripts.get(&take.id)?;
        let t = time?;
        doc.segment_at(t).map(|seg| seg.id())
    })
    .flatten();
    let changed = UI.with(|u| u.current.replace(current.clone()) != current);
    let Some(p) = recordings::with(|s| s.detail.clone()) else {
        return;
    };
    if let Some(v) = p.body.get("transport") {
        v.setNeedsDisplay(true);
    }
    if let Some(slider) = p
        .body
        .get("transport-slider")
        .and_then(|v| v.downcast::<NSSlider>().ok())
        && UI.with(|u| u.scrubbing.get()).is_none()
    {
        let (t, d) = player::with(|p| (p.time, p.duration.max(0.1)));
        slider.setMaxValue(d);
        slider.setDoubleValue(t.min(d));
    }
    if changed {
        for (id, _, _) in UI.with(|u| u.lines.borrow().clone()) {
            if let Some(v) = p.body.get(&format!("line-{id}")) {
                v.setNeedsDisplay(true);
            }
        }
    }
    if changed && let Some(id) = current {
        let playing = player::with(|p| p.playing);
        if playing && UI.with(|u| u.following.get()) {
            scroll_to_line(&id);
        }
    }
    // The Follow Along button comes and goes with playback.
    let show = UI.with(|u| !u.following.get())
        && player::with(|p| p.playing)
        && UI.with(|u| u.current.borrow().is_some());
    if let Some(b) = p.overlay.get("follow") {
        b.setHidden(!show);
    }
}

fn scroll_to_line(id: &str) {
    let Some((_, y, h)) = UI.with(|u| u.lines.borrow().iter().find(|(l, _, _)| l == id).cloned())
    else {
        return;
    };
    let Some(p) = recordings::with(|s| s.detail.clone()) else {
        return;
    };
    {
        let clip = p.scroll.contentView();
        let visible = clip.bounds().size.height;
        let doc_h = p.body.root.frame().size.height;
        let target = (y + h / 2.0 - visible / 2.0).clamp(0.0, (doc_h - visible).max(0.0));
        if util::reduce_motion() {
            clip.scrollToPoint(NSPoint::new(0.0, target));
        } else {
            objc2_app_kit::NSAnimationContext::runAnimationGroup(&block2::RcBlock::new(
                move |ctx: NonNull<objc2_app_kit::NSAnimationContext>| {
                    // SAFETY: AppKit passes a valid context.
                    unsafe { ctx.as_ref() }.setDuration(0.2);
                    clip.animator().setBoundsOrigin(NSPoint::new(0.0, target));
                },
            ));
        }
        p.scroll.reflectScrolledClipView(&p.scroll.contentView());
    }
}

// MARK: Helpers

fn secondary() -> Retained<NSColor> {
    NSColor::secondaryLabelColor()
}

fn primary() -> Retained<NSColor> {
    NSColor::labelColor()
}

/// macOS text styles: title3 15, body 13, callout 12, caption 10.
fn title3() -> Retained<objc2_app_kit::NSFont> {
    theme::font(15.0, Weight::Semibold)
}

fn callout() -> Retained<objc2_app_kit::NSFont> {
    theme::font(12.0, Weight::Regular)
}

fn caption() -> Retained<objc2_app_kit::NSFont> {
    theme::font(10.0, Weight::Regular)
}

fn body_font() -> Retained<objc2_app_kit::NSFont> {
    theme::font(13.0, Weight::Regular)
}

fn caps_label(b: &mut Build, key: &str, text: &str, x: f64, y: f64) -> f64 {
    let t = Text::new(text.to_string(), theme::caps(), theme::mute()).kern(0.9);
    let h = t.height();
    b.canvas(key, rect(x, y, t.width() + 2.0, h))
        .on_draw(move |_| t.draw_at(0.0, 0.0));
    h
}

/// A stock push button with an optional SF Symbol, sized to fit.
fn button(
    b: &mut Build,
    key: &str,
    title: &str,
    symbol: Option<&str>,
    small: bool,
    x: f64,
    y: f64,
    f: impl Fn() + 'static,
) -> Retained<NSButton> {
    let mtm = util::mtm();
    let btn = b.native(key, rect(x, y, 10.0, 10.0), || {
        let b = NSButton::new(mtm);
        b.setBezelStyle(NSBezelStyle::Push);
        b
    });
    btn.setTitle(&ns(title));
    if let Some(name) = symbol {
        btn.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), None).as_deref(),
        );
        btn.setImagePosition(NSCellImagePosition::ImageLeading);
    }
    btn.setControlSize(if small {
        NSControlSize::Small
    } else {
        NSControlSize::Regular
    });
    btn.sizeToFit();
    let size = btn.frame().size;
    btn.setFrame(rect(x, y, size.width, size.height));
    on_action(&btn, f);
    btn
}

/// Width a button will take, for laying out a row before placing it.
fn button_width(title: &str, symbol: bool, small: bool) -> f64 {
    let mtm = util::mtm();
    let b = NSButton::new(mtm);
    b.setBezelStyle(NSBezelStyle::Push);
    b.setTitle(&ns(title));
    if symbol {
        b.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns("play.fill"), None)
                .as_deref(),
        );
        b.setImagePosition(NSCellImagePosition::ImageLeading);
    }
    b.setControlSize(if small {
        NSControlSize::Small
    } else {
        NSControlSize::Regular
    });
    b.sizeToFit();
    b.frame().size.width
}

/// ContentUnavailableView: a large symbol, a title and a line of explanation, centered.
fn unavailable(
    b: &mut Build,
    symbol: &str,
    title: &str,
    description: Option<&str>,
    width: f64,
    height: f64,
) {
    let title_t = Text::new(
        title.to_string(),
        theme::font(17.0, Weight::Bold),
        primary(),
    );
    let desc = description.map(|d| Text::new(d.to_string(), body_font(), secondary()).wrapping());
    let dw = (width - 80.0).clamp(100.0, 360.0);
    let dh = desc.as_ref().map_or(0.0, |d| d.wrapped_height(dw));
    let total = 44.0 + 12.0 + title_t.height() + if desc.is_some() { 6.0 + dh } else { 0.0 };
    let top = ((height - total) / 2.0).max(0.0);
    let symbol = symbol.to_string();
    b.canvas("unavailable", rect(0.0, top, width, total))
        .on_draw(move |r| {
            draw::draw_symbol(
                &symbol,
                36.0,
                Weight::Regular,
                &secondary(),
                r.size.width / 2.0,
                22.0,
            );
            title_t.draw_at(((r.size.width - title_t.width()) / 2.0).round(), 56.0);
            if let Some(d) = &desc {
                let d = d.clone();
                let style = objc2_app_kit::NSMutableParagraphStyle::new();
                style.setAlignment(objc2_app_kit::NSTextAlignment::Center);
                let _ = style;
                d.draw_in(rect(
                    ((r.size.width - dw) / 2.0).round(),
                    56.0 + title_t.height() + 6.0,
                    dw,
                    dh,
                ));
            }
        });
}

// MARK: One take

fn mode_text(take: &Take) -> String {
    if take.is_two_speaker() {
        tr("Two speakers")
    } else if take.files.iter().any(|f| f.role == Role::Stereo) {
        tr("Stereo")
    } else if take.files.iter().any(|f| f.role == Role::Mix) {
        tr("Mono")
    } else {
        tr("Single mic")
    }
}

/// Date and actions side by side when they fit; the actions drop below in a narrow pane.
fn take_header(
    b: &mut Build,
    take: &Take,
    transcript: Option<&TranscriptDocument>,
    width: f64,
) -> f64 {
    let date = Text::new(
        take.started
            .map(format::date_text)
            .unwrap_or_else(|| take.id.clone()),
        title3(),
        primary(),
    );
    let summary = Text::new(
        trf(
            "{length} · {mode}",
            &[
                ("length", &format::length(take.duration.unwrap_or(0.0))),
                ("mode", &mode_text(take)),
            ],
        ),
        theme::digits(12.0, Weight::Regular),
        secondary(),
    );
    let plates: Vec<Plate> = take
        .speakers
        .iter()
        .map(|sp| {
            Plate::speaker(
                &sp.name,
                (sp.name != format!("TX{}", sp.mic)).then(|| format!("TX{}", sp.mic)),
            )
        })
        .collect();
    let plates_w: f64 = plates.iter().map(|p| p.width() + 6.0).sum();
    let line2_h = if plates.is_empty() {
        summary.height()
    } else {
        27.0
    };
    let info_w = date.width().max(summary.width() + plates_w);
    let info_h = date.height() + 4.0 + line2_h;

    // Buttons: Play, Share, app plugins, More.
    let playing = player::with(|p| p.playing);
    let video_job = store::with(|m| m.plugins.job(&take.id, Some(VIDEO_JOB_ID)).cloned());
    let share = take.share_path();
    let apps: Vec<(String, String)> = store::with(|m| {
        m.plugins
            .app_plugins()
            .iter()
            .map(|p| {
                let title = p
                    .manifest
                    .app
                    .as_ref()
                    .and_then(|a| a.action_title.clone())
                    .unwrap_or_else(|| trf("Open in {plugin}", &[("plugin", &p.manifest.name)]));
                (p.id().to_string(), title)
            })
            .collect()
    });
    let play_title = if playing { tr("Pause") } else { tr("Play") };
    let mut widths = vec![button_width(&play_title, true, false)];
    if share.is_some() {
        widths.push(button_width(&tr("Share"), true, false));
    }
    for (_, title) in &apps {
        widths.push(button_width(title, true, false));
    }
    widths.push(button_width(&tr("More"), true, false) + 18.0);
    let buttons_w: f64 = widths.iter().sum::<f64>() + 8.0 * (widths.len() as f64 - 1.0);
    let side = info_w + 16.0 + buttons_w <= width - 2.0 * PAD;
    let (bx, by) = if side {
        (width - PAD - buttons_w, PAD)
    } else {
        (PAD, PAD + info_h + 10.0)
    };

    let summary_w = summary.width();
    b.canvas("take-info", rect(PAD, PAD, info_w + 2.0, info_h))
        .on_draw(move |_| {
            date.draw_at(0.0, 0.0);
            let y2 = date.height() + 4.0;
            summary.draw_at(0.0, y2 + ((line2_h - summary.height()) / 2.0).round());
            let mut x = summary_w + 6.0;
            for p in &plates {
                let w = p.width();
                p.draw(rect(x, y2, w, 27.0));
                x += w + 6.0;
            }
        });

    let mut x = bx;
    let t = take.clone();
    let play = button(
        b,
        "play",
        &play_title,
        Some(if playing { "pause.fill" } else { "play.fill" }),
        false,
        x,
        by,
        move || {
            let loaded = player::with(|p| p.media.is_some());
            match TakeMedia::all(&t).first() {
                Some(best) if !loaded => player::load(best, &t, true),
                _ => player::toggle(),
            }
        },
    );
    play.setToolTip(Some(&ns(&tr("Play or pause (Space)"))));
    let button_h = play.frame().size.height;
    x += play.frame().size.width + 8.0;
    if let Some(path) = share.clone() {
        let video = path.extension().is_some_and(|e| e == "mp4");
        let anchor: RefCell<Option<Retained<NSButton>>> = RefCell::new(None);
        let anchor = std::rc::Rc::new(anchor);
        let a2 = anchor.clone();
        let sb = button(
            b,
            "share",
            &tr("Share"),
            Some("square.and.arrow.up"),
            false,
            x,
            by,
            move || {
                let url = util::file_url(&path);
                let items = NSArray::from_retained_slice(&[Retained::into_super(
                    Retained::into_super(url),
                )]);
                // SAFETY: file URLs are valid sharing items.
                let picker = unsafe {
                    NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items)
                };
                if let Some(view) = a2.borrow().as_ref() {
                    picker.showRelativeToRect_ofView_preferredEdge(
                        view.bounds(),
                        view,
                        NSRectEdge::MinY,
                    );
                }
            },
        );
        *anchor.borrow_mut() = Some(sb.clone());
        sb.setEnabled(video_job.is_none());
        sb.setToolTip(Some(&ns(&if video_job.is_some() {
            tr("Wait for the video to finish")
        } else if video {
            tr("Share the video")
        } else {
            tr("Share the audio")
        })));
        x += sb.frame().size.width + 8.0;
    }
    for (id, title) in apps {
        let t = take.clone();
        let ab = button(
            b,
            &format!("app-{id}"),
            &title,
            Some("film.stack"),
            false,
            x,
            by,
            move || store::open_in_app(&t, &id),
        );
        x += ab.frame().size.width + 8.0;
    }
    let more = b.native("more", rect(x, by, 90.0, button_h), || {
        NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(util::mtm()),
            rect(0.0, 0.0, 90.0, 24.0),
            true,
        )
    });
    more.setMenu(Some(&more_menu(take, transcript)));
    more.sizeToFit();
    let mf = more.frame();
    more.setFrame(rect(x, by, mf.size.width, button_h));
    if side {
        (PAD + info_h.max(button_h) + PAD).max(0.0)
    } else {
        by + button_h + PAD
    }
}

fn more_menu(take: &Take, transcript: Option<&TranscriptDocument>) -> Retained<NSMenu> {
    let mtm = util::mtm();
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let head = item(&tr("More"), || {});
    head.setImage(
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns("ellipsis.circle"), None)
            .as_deref(),
    );
    menu.addItem(&head);
    let paths = take.all_paths();
    menu.addItem(&item(&tr("Show in Finder"), move || util::reveal(&paths)));
    if let Some(doc) = transcript {
        let text = list::transcript_text(doc);
        menu.addItem(&item(&tr("Copy Transcript"), move || {
            util::copy_text(&text)
        }));
    }
    let actions: Vec<(String, String, String)> = store::with(|m| {
        m.plugins
            .actions_for(take)
            .into_iter()
            .map(|(p, a)| (p.id().to_string(), a.id.clone(), a.title.clone()))
            .collect()
    });
    let can_video = take.has("video");
    if can_video || !actions.is_empty() {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    if can_video {
        let busy = store::with(|m| m.plugins.job(&take.id, Some(VIDEO_JOB_ID)).is_some());
        let t = take.clone();
        let entry = item(
            &if busy {
                tr("Remake Video (in progress)")
            } else {
                tr("Remake Video")
            },
            move || store::make_video(std::slice::from_ref(&t), false),
        );
        entry.setEnabled(!busy);
        entry.setToolTip(Some(&ns(&tr("Rebuild the finished video from the screen and camera recordings, with your current camera bubble settings"))));
        menu.addItem(&entry);
    }
    for (plugin, action, title) in actions {
        let busy = store::with(|m| m.plugins.job(&take.id, Some(&plugin)).is_some());
        let t = take.clone();
        let label = if busy {
            trf("{action} (in progress)", &[("action", &title)])
        } else {
            title
        };
        let entry = item(&label, move || {
            store::run_action(&plugin, &action, std::slice::from_ref(&t))
        });
        entry.setEnabled(!busy);
        menu.addItem(&entry);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let t = take.clone();
    menu.addItem(&item(&tr("Move to Trash…"), move || {
        recordings::ask_to_delete(Some(vec![t.clone()]))
    }));
    menu
}

fn take_body(
    b: &mut Build,
    take: &Take,
    transcript: Option<&TranscriptDocument>,
    search: &str,
    width: f64,
) -> f64 {
    let w = width - 2.0 * PAD;
    let mut y = PAD;
    y = media_area(b, take, y, w);
    y += 18.0;
    y = files(b, take, y, w);
    y += 18.0;
    y = transcript_section(b, take, transcript, search, y, w);
    y + PAD
}

fn media_area(b: &mut Build, take: &Take, mut y: f64, w: f64) -> f64 {
    let (media, error) = player::with(|p| (p.media.clone(), p.error.clone()));
    let mut parts = 0;
    if media.as_ref().is_some_and(TakeMedia::is_video) {
        let h = (w * 9.0 / 16.0).min(360.0);
        let view = b.native("video", rect(PAD, y, w, h), || {
            // SAFETY: AVKit's initializer.
            let v = unsafe {
                AVPlayerView::initWithFrame(AVPlayerView::alloc(util::mtm()), rect(0.0, 0.0, w, h))
            };
            // SAFETY: plain AVKit configuration.
            unsafe {
                v.setControlsStyle(AVPlayerViewControlsStyle::Inline);
                v.setShowsFullScreenToggleButton(true);
                v.setAllowsPictureInPicturePlayback(true);
            }
            v.setWantsLayer(true);
            if let Some(l) = v.layer() {
                l.setCornerRadius(8.0);
                l.setMasksToBounds(true);
                l.setBackgroundColor(Some(&NSColor::blackColor().CGColor()));
            }
            v
        });
        let av = player::av();
        // SAFETY: plain AVKit call.
        unsafe {
            if view.player().is_none_or(|p| !std::ptr::eq(&*p, &*av)) {
                view.setPlayer(Some(&av));
            }
        }
        y += h;
        parts += 1;
    } else if media.is_some() {
        y = transport(b, y, w);
        parts += 1;
    }
    if let Some(e) = error {
        if parts > 0 {
            y += 8.0;
        }
        let name = media
            .as_ref()
            .map(|m| m.file.name())
            .unwrap_or_else(|| tr("this file"));
        let t = Text::new(
            trf(
                "Can’t play {file}: {error}",
                &[("file", &name), ("error", &e)],
            ),
            callout(),
            theme::warn_text(),
        )
        .wrapping();
        let h = t.wrapped_height(w - 20.0).max(15.0);
        b.canvas("play-error", rect(PAD, y, w, h))
            .on_draw(move |_| {
                draw::draw_symbol(
                    "exclamationmark.triangle.fill",
                    12.0,
                    Weight::Regular,
                    &theme::warn_text(),
                    8.0,
                    7.5,
                );
                t.draw_in(rect(20.0, 0.0, w - 20.0, h));
            });
        y += h;
        parts += 1;
    }
    let status = video_status(
        b,
        take,
        media.as_ref(),
        if parts > 0 { y + 8.0 } else { y },
        w,
    );
    if status > 0.0 {
        y = status;
    }
    y
}

/// Play/pause, the playhead and the length, for takes without video.
fn transport(b: &mut Build, y: f64, w: f64) -> f64 {
    let h = 32.0;
    let (playing, duration) = player::with(|p| (p.playing, p.duration.max(0.1)));
    b.canvas("transport", rect(PAD, y, w, h)).on_draw(move |r| {
        draw::fill_round(r, 8.0, &theme::face());
        draw::stroke_round(r, 8.0, 0.5, &theme::hairline());
        let time = UI
            .with(|u| u.scrubbing.get())
            .unwrap_or_else(|| player::with(|p| p.time.min(duration)));
        let t = Text::new(format::length(time), theme::mono_data(), secondary());
        t.draw_at(10.0 + 16.0 + 10.0, (r.size.height - t.height()) / 2.0);
        let total = Text::new(format::length(duration), theme::mono_data(), secondary());
        total.draw_at(
            r.size.width - 10.0 - total.width(),
            (r.size.height - total.height()) / 2.0,
        );
    });
    let toggle = b.native(
        "transport-toggle",
        rect(PAD + 10.0, y + 8.0, 16.0, 16.0),
        || {
            let btn = NSButton::new(util::mtm());
            btn.setBordered(false);
            btn
        },
    );
    toggle.setImage(
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &ns(if playing { "pause.fill" } else { "play.fill" }),
            Some(&ns(&if playing { tr("Pause") } else { tr("Play") })),
        )
        .as_deref(),
    );
    on_action(&toggle, player::toggle);
    let label_w = Text::new("00:00", theme::mono_data(), secondary())
        .width()
        .max(Text::new(format::length(duration), theme::mono_data(), secondary()).width());
    let sx = PAD + 10.0 + 16.0 + 10.0 + label_w + 10.0;
    let sw = (w - (sx - PAD) - 10.0 - label_w - 10.0).max(40.0);
    let slider = b.native("transport-slider", rect(sx, y + 8.0, sw, 16.0), || {
        let s = NSSlider::initWithFrame(NSSlider::alloc(util::mtm()), rect(0.0, 0.0, 100.0, 16.0));
        s.setControlSize(NSControlSize::Small);
        s.setMinValue(0.0);
        s.setContinuous(true);
        s
    });
    slider.setMaxValue(duration);
    let s2 = slider.clone();
    on_action(&slider, move || {
        let v = s2.doubleValue();
        // The playhead moves when the drag ends, as SwiftUI's slider does.
        let ending = objc2_app_kit::NSApplication::sharedApplication(util::mtm())
            .currentEvent()
            .is_none_or(|e| e.r#type() == objc2_app_kit::NSEventType::LeftMouseUp);
        if ending {
            UI.with(|u| u.scrubbing.set(None));
            player::seek(v);
        } else {
            UI.with(|u| u.scrubbing.set(Some(v)));
            if let Some(t) = recordings::with(|s| s.detail.body.get("transport")).flatten() {
                t.setNeedsDisplay(true);
            }
        }
    });
    y + h
}

/// What the video shows, and whether it's being made, failed or missing.
fn video_status(b: &mut Build, take: &Take, media: Option<&TakeMedia>, y: f64, w: f64) -> f64 {
    let finished = take.finished_video().is_some();
    let raw = take.has("video");
    let job = store::with(|m| m.plugins.job(&take.id, Some(VIDEO_JOB_ID)).cloned());
    let failure = store::with(|m| m.plugins.failure(&take.id, Some(VIDEO_JOB_ID)).cloned());
    if let Some(job) = job {
        let queued = job.state == JobState::Queued;
        let p = b.native("video-progress", rect(PAD, y + 4.0, 140.0, 12.0), bar);
        p.setDoubleValue(if queued { 0.0 } else { job.fraction });
        let text = if queued {
            tr("Video queued")
        } else {
            trf(
                "Making the video… {percent}%",
                &[("percent", &((job.fraction * 100.0) as i64).to_string())],
            )
        };
        let t = Text::new(text, callout(), secondary());
        b.canvas(
            "video-status",
            rect(PAD + 148.0, y + 2.0, t.width() + 2.0, 15.0),
        )
        .on_draw(move |_| t.draw_at(0.0, 0.0));
        let id = job.id;
        let c = button(
            b,
            "video-cancel",
            &tr("Cancel"),
            None,
            true,
            0.0,
            y,
            move || store::cancel_job(id),
        );
        c.setFrameOrigin(NSPoint::new(PAD + w - c.frame().size.width, y));
        return y + c.frame().size.height.max(20.0);
    }
    if let Some(failed) = failure {
        let reason = failed.failure().unwrap_or_default().to_string();
        let t = Text::new(
            trf(
                "The video couldn’t be made: {reason}",
                &[("reason", &reason)],
            ),
            callout(),
            theme::warn_text(),
        )
        .wrapping();
        let mut right = PAD + w;
        let id = failed.id;
        let x = b.canvas("video-dismiss", rect(right - 14.0, y + 2.0, 14.0, 15.0));
        x.on_draw(|r| {
            draw::draw_symbol(
                "xmark",
                11.0,
                Weight::Regular,
                &secondary(),
                r.size.width / 2.0,
                7.5,
            );
        })
        .on_click(move || store::dismiss_job(id))
        .tip(&tr("Dismiss"));
        right -= 14.0 + 8.0;
        if take.has("video") {
            let tk = take.clone();
            let again = button(
                b,
                "video-again",
                &tr("Try Again"),
                None,
                true,
                0.0,
                y,
                move || store::make_video(std::slice::from_ref(&tk), false),
            );
            right -= again.frame().size.width;
            again.setFrameOrigin(NSPoint::new(right, y));
            right -= 8.0;
        }
        let tw = right - PAD - 20.0;
        let h = t.wrapped_height(tw).max(15.0);
        b.canvas("video-status", rect(PAD, y, right - PAD, h))
            .on_draw(move |_| {
                draw::draw_symbol(
                    "exclamationmark.triangle.fill",
                    12.0,
                    Weight::Regular,
                    &theme::warn_text(),
                    7.0,
                    7.5,
                );
                t.draw_in(rect(20.0, 0.0, tw, h));
            });
        return y + h.max(20.0);
    }
    if raw && !finished {
        let tk = take.clone();
        let make = button(
            b,
            "video-make",
            &tr("Make Video"),
            None,
            true,
            0.0,
            y,
            move || store::make_video(std::slice::from_ref(&tk), false),
        );
        let mw = make.frame().size.width;
        make.setFrameOrigin(NSPoint::new(PAD + w - mw, y));
        let t = Text::new(tr("This take has no finished video yet, so this is the raw recording with the mic audio."), callout(), secondary()).wrapping();
        let tw = w - mw - 8.0;
        let h = t.wrapped_height(tw).max(15.0);
        b.canvas("video-status", rect(PAD, y, tw, h))
            .on_draw(move |r| t.draw_in(r));
        return y + h.max(make.frame().size.height);
    }
    let text = match media.map(|m| m.kind) {
        Some(MediaKind::Video) => Some(video_description(take)),
        Some(MediaKind::Camera) => {
            Some(tr("The raw camera recording, with the mic audio lined up."))
        }
        Some(MediaKind::Screen) => {
            Some(tr("The raw screen recording, with the mic audio lined up."))
        }
        _ => None,
    };
    if let Some(text) = text {
        let t = Text::new(text, callout(), secondary()).wrapping();
        let h = t.wrapped_height(w).max(15.0);
        b.canvas("video-status", rect(PAD, y, w, h))
            .on_draw(move |r| t.draw_in(r));
        return y + h;
    }
    0.0
}

fn bar() -> Retained<NSProgressIndicator> {
    let p = NSProgressIndicator::new(util::mtm());
    p.setStyle(NSProgressIndicatorStyle::Bar);
    p.setIndeterminate(false);
    p.setMaxValue(1.0);
    p.setControlSize(NSControlSize::Small);
    p
}

fn video_description(take: &Take) -> String {
    match (take.has_capture("screen"), take.has_capture("camera")) {
        (true, true) => {
            tr("The finished video: your screen with the camera in a corner, and the mic audio.")
        }
        (true, false) => tr("The finished video: your screen with the mic audio."),
        (false, true) => tr("The finished video: the camera with the mic audio."),
        _ => tr("The finished video, with the mic audio."),
    }
}

/// What a file in a take is, in words.
pub fn file_label(file: &TakeFile) -> String {
    match file.role {
        Role::Stereo => tr("Stereo audio (TX1 left, TX2 right)"),
        Role::Mix => tr("Mono audio (both mics)"),
        Role::Track => trf(
            "TX{index} audio",
            &[("index", &file.mic.unwrap_or(0).to_string())],
        ),
        Role::Derived if file.source.as_deref() == Some("pip") => tr("Finished video"),
        Role::Derived if file.kind.as_deref() == Some("transcript") => tr("Transcript"),
        Role::Derived => file.kind.clone().unwrap_or_else(|| tr("Plugin output")),
        Role::Metadata => tr("Metadata"),
        Role::Capture => match file.source.as_deref() {
            Some("screen") => tr("Screen recording (raw)"),
            Some("camera") => tr("Camera recording (raw)"),
            Some("system-audio") => tr("System audio"),
            _ => trf(
                "Captured {kind}",
                &[("kind", file.kind.as_deref().unwrap_or("media"))],
            ),
        },
    }
}

fn files(b: &mut Build, take: &Take, mut y: f64, w: f64) -> f64 {
    y += caps_label(b, "files-label", &tr("FILES"), PAD, y) + 4.0;
    let media = TakeMedia::all(take);
    let (loaded, playing) = player::with(|p| (p.media.clone(), p.playing));
    let all: Vec<&TakeFile> = take
        .files
        .iter()
        .filter(|f| f.role != Role::Metadata)
        .collect();
    let transcripts: Vec<&TakeFile> = all
        .iter()
        .copied()
        .filter(|f| f.kind.as_deref() == Some("transcript"))
        .collect();
    let mut rows: Vec<(
        String,
        String,
        &'static str,
        Vec<PathBuf>,
        Option<TakeMedia>,
    )> = all
        .iter()
        .filter(|f| f.kind.as_deref() != Some("transcript"))
        .map(|f| {
            let playable = media.iter().find(|m| m.file.path == f.path).cloned();
            (
                f.name(),
                file_label(f),
                if f.kind.as_deref() == Some("video") {
                    "film"
                } else {
                    "waveform"
                },
                vec![f.path.clone()],
                playable,
            )
        })
        .collect();
    if !transcripts.is_empty() {
        let kinds: Vec<String> = transcripts
            .iter()
            .map(|f| {
                f.path
                    .extension()
                    .map(|e| e.to_string_lossy().to_uppercase())
                    .unwrap_or_default()
            })
            .collect();
        rows.push((
            format!("{}.transcript", take.stem),
            trf("Transcript · {formats}", &[("formats", &kinds.join(", "))]),
            "doc.text",
            transcripts.iter().map(|f| f.path.clone()).collect(),
            None,
        ));
    }
    for (i, (name, label, symbol, paths, playable)) in rows.into_iter().enumerate() {
        if i > 0 {
            y += 2.0;
        }
        let missing = !paths.iter().any(|p| p.exists());
        let is_loaded = playable.is_some() && loaded == playable;
        let key = format!("file-{name}");
        let hover_key = key.clone();
        let hovering = UI.with(|u| u.hover.borrow().as_deref() == Some(&key));
        let clickable = playable.is_some();
        let n = Text::new(
            name.clone(),
            callout(),
            if missing { secondary() } else { primary() },
        )
        .middle()
        .strike(missing);
        let l = Text::new(
            if missing {
                tr("Missing")
            } else {
                label.clone()
            },
            caption(),
            if missing {
                theme::warn_text()
            } else {
                secondary()
            },
        );
        let c = b.canvas(&key, rect(PAD, y, w, 23.0));
        c.on_draw(move |r| {
            if is_loaded {
                draw::fill_round(r, 6.0, &theme::alpha(&NSColor::controlAccentColor(), 0.12));
            } else if hovering && clickable {
                draw::fill_round(r, 6.0, &theme::alpha(&NSColor::labelColor(), 0.05));
            }
            let icon = if is_loaded && playing {
                "speaker.wave.2.fill"
            } else {
                symbol
            };
            let tint = if is_loaded {
                NSColor::controlAccentColor()
            } else {
                secondary()
            };
            draw::draw_symbol(icon, 12.0, Weight::Regular, &tint, 6.0 + 8.0, 11.5);
            let lw = l.width();
            l.draw_at(r.size.width - 6.0 - lw, (r.size.height - l.height()) / 2.0);
            n.draw_in(rect(
                6.0 + 16.0 + 8.0,
                4.0,
                r.size.width - 30.0 - lw - 14.0,
                15.0,
            ));
        });
        if let Some(m) = playable.clone()
            && !missing
        {
            let t = take.clone();
            c.on_click(move || player::load(&m, &t, true));
        }
        c.on_hover(move |inside| {
            UI.with(|u| *u.hover.borrow_mut() = inside.then(|| hover_key.clone()));
            if let Some(v) = recordings::with(|s| s.detail.body.get(&hover_key)).flatten() {
                v.setNeedsDisplay(true);
            }
        });
        c.drag_out(paths.clone());
        let menu_paths = paths.clone();
        c.on_menu(move || Some(file_menu(&menu_paths)));
        c.tip(&if missing {
            tr("This file was moved or deleted outside Lavdeck")
        } else if clickable {
            tr("Click to play. Drag to another app, or right-click for more.")
        } else {
            tr("Drag to another app, or right-click for more.")
        });
        y += 23.0;
    }
    y
}

fn file_menu(paths: &[PathBuf]) -> Retained<NSMenu> {
    let mtm = util::mtm();
    let menu = NSMenu::new(mtm);
    if let [one] = paths {
        let (a, b2, c) = (one.clone(), one.clone(), one.clone());
        menu.addItem(&item(&tr("Open"), move || util::open(&a)));
        menu.addItem(&item(&tr("Show in Finder"), move || {
            util::reveal(std::slice::from_ref(&b2))
        }));
        menu.addItem(&item(&tr("Copy Path"), move || {
            util::copy_text(&c.to_string_lossy())
        }));
    } else {
        for p in paths {
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default();
            let path = p.clone();
            menu.addItem(&item(
                &trf("Open .{extension}", &[("extension", &ext)]),
                move || util::open(&path),
            ));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let all = paths.to_vec();
        menu.addItem(&item(&tr("Show in Finder"), move || util::reveal(&all)));
    }
    menu
}

fn transcript_section(
    b: &mut Build,
    take: &Take,
    transcript: Option<&TranscriptDocument>,
    search: &str,
    mut y: f64,
    w: f64,
) -> f64 {
    let action = store::with(|m| {
        m.plugins
            .transcribe_action(Some(take))
            .map(|(p, a)| (p.id().to_string(), a.id.clone(), a.title.clone()))
    });
    let job = action
        .as_ref()
        .and_then(|(p, _, _)| store::with(|m| m.plugins.job(&take.id, Some(p)).cloned()));
    let failure = action
        .as_ref()
        .and_then(|(p, _, _)| store::with(|m| m.plugins.failure(&take.id, Some(p)).cloned()));
    let label_h = caps_label(b, "transcript-label", &tr("TRANSCRIPT"), PAD, y + 3.0);
    let mut row_h = label_h + 6.0;
    if let Some(job) = &job {
        let queued = job.state == JobState::Queued;
        let id = job.id;
        let cancel = button(
            b,
            "transcript-cancel",
            &tr("Cancel"),
            None,
            true,
            0.0,
            y,
            move || store::cancel_job(id),
        );
        let cw = cancel.frame().size.width;
        cancel.setFrameOrigin(NSPoint::new(PAD + w - cw, y));
        let text = if queued {
            tr("Queued")
        } else {
            job.message.clone().unwrap_or_else(|| job.title.clone())
        };
        let t = Text::new(text, caption(), secondary());
        let tw = t.width();
        b.canvas(
            "transcript-status",
            rect(PAD + w - cw - 8.0 - tw, y + 5.0, tw, 13.0),
        )
        .on_draw(move |_| t.draw_at(0.0, 0.0));
        let p = b.native(
            "transcript-progress",
            rect(PAD + w - cw - 8.0 - tw - 8.0 - 120.0, y + 5.0, 120.0, 12.0),
            bar,
        );
        p.setDoubleValue(if queued { 0.0 } else { job.fraction });
        row_h = row_h.max(cancel.frame().size.height);
    } else if transcript.is_some()
        && let Some((plugin, action_id, _)) = action.clone()
    {
        let t = take.clone();
        let regen = button(
            b,
            "transcript-regenerate",
            &tr("Regenerate"),
            None,
            true,
            0.0,
            y,
            move || store::run_action(&plugin, &action_id, std::slice::from_ref(&t)),
        );
        let rw = regen.frame().size.width;
        regen.setFrameOrigin(NSPoint::new(PAD + w - rw, y));
        row_h = row_h.max(regen.frame().size.height);
    }
    y += row_h + 8.0;
    if let Some(failed) = failure {
        let t = Text::new(
            trf(
                "{plugin} failed: {reason}",
                &[
                    ("plugin", &failed.plugin_name),
                    ("reason", failed.failure().unwrap_or_default()),
                ],
            ),
            callout(),
            theme::warn_text(),
        )
        .wrapping();
        let h = t.wrapped_height(w - 30.0).max(15.0);
        b.canvas("transcript-failure", rect(PAD, y, w - 30.0, h))
            .on_draw(move |r| t.draw_in(r));
        let id = failed.id;
        b.canvas("transcript-dismiss", rect(PAD + w - 14.0, y, 14.0, 15.0))
            .on_draw(|r| {
                draw::draw_symbol(
                    "xmark",
                    11.0,
                    Weight::Regular,
                    &secondary(),
                    r.size.width / 2.0,
                    7.5,
                );
            })
            .on_click(move || store::dismiss_job(id))
            .tip(&tr("Dismiss"));
        y += h + 8.0;
    }
    if let Some(doc) = transcript {
        return transcript_list(b, take, doc, search, y, w);
    }
    UI.with(|u| u.lines.borrow_mut().clear());
    if job.is_some() {
        return y - 8.0;
    }
    // Nothing yet: offer to transcribe, or say how to.
    let note = Text::new(
        tr("No transcript for this take yet."),
        body_font(),
        secondary(),
    );
    let mut inner = 12.0 + note.height() + 8.0;
    let box_y = y;
    let mut content: Vec<Retained<NSButton>> = Vec::new();
    let mut extra: Option<Text> = None;
    if let Some((plugin, action_id, title)) = action {
        let t = take.clone();
        content.push(button(
            b,
            "transcribe",
            &title,
            Some("text.quote"),
            false,
            PAD + 12.0,
            box_y + inner,
            move || store::run_action(&plugin, &action_id, std::slice::from_ref(&t)),
        ));
    } else {
        let e = Text::new(
            tr("Turn on a transcription plugin in Settings › Plugins to transcribe recordings."),
            callout(),
            secondary(),
        )
        .wrapping();
        let eh = e.wrapped_height(w - 24.0);
        inner += eh + 8.0;
        extra = Some(e);
        content.push(button(
            b,
            "open-settings",
            &tr("Open Settings"),
            None,
            false,
            PAD + 12.0,
            box_y + inner,
            crate::settings_window::show,
        ));
    }
    inner += content.iter().map(|c| c.frame().size.height).sum::<f64>() + 12.0;
    let bg = b.canvas("no-transcript", rect(PAD, box_y, w, inner));
    bg.on_draw(move |r| {
        draw::fill_round(r, 8.0, &theme::face());
        note.draw_at(12.0, 12.0);
        if let Some(e) = &extra {
            e.draw_in(rect(
                12.0,
                12.0 + note.height() + 8.0,
                r.size.width - 24.0,
                e.wrapped_height(r.size.width - 24.0),
            ));
        }
    });
    send_back(&bg);
    box_y + inner
}

fn send_back(view: &NSView) {
    // SAFETY: the view is in a hierarchy (it was just built into one).
    let Some(parent) = (unsafe { view.superview() }) else {
        return;
    };
    view.removeFromSuperview();
    parent.addSubview_positioned_relativeTo(view, objc2_app_kit::NSWindowOrderingMode::Below, None);
}

/// Byte ranges of `query` in `text`, ignoring case.
fn hits(text: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    let q = query.to_lowercase();
    // Lowercasing can change byte lengths; only highlight when it didn't.
    if lower.len() != text.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find(&q) {
        let start = from + i;
        out.push((start, start + q.len()));
        from = start + q.len();
    }
    out
}

/// Transcript lines; click one to play from there.
fn transcript_list(
    b: &mut Build,
    take: &Take,
    doc: &TranscriptDocument,
    search: &str,
    mut y: f64,
    w: f64,
) -> f64 {
    let multi = doc.speakers.len() > 1;
    let query = search.trim().to_string();
    let long = doc.segments.last().is_some_and(|s| s.end >= 3600.0);
    let stamp_w = Text::new(
        if long { "00:00:00" } else { "00:00" },
        theme::mono_data(),
        secondary(),
    )
    .width()
    .max(36.0);
    let text_x = 6.0 + stamp_w + 10.0 + if multi { 64.0 + 10.0 } else { 0.0 };
    let text_w = w - text_x - 6.0;
    let mut lines = Vec::new();
    for seg in &doc.segments {
        let id = seg.id();
        let stamp = if long {
            format::timestamp(seg.start)
        } else {
            format::length(seg.start.floor())
        };
        let text = Text::new(seg.text.clone(), body_font(), primary()).wrapping();
        let ranges = hits(&seg.text, &query);
        let hit = !ranges.is_empty();
        let h = text.wrapped_height(text_w).max(16.0) + 8.0;
        let stamp_t = Text::new(stamp.clone(), theme::mono_data(), secondary());
        let who = multi.then(|| {
            Text::new(
                doc.speaker_name(&seg.speaker).to_string(),
                theme::label_bold(),
                primary(),
            )
        });
        let line_id = id.clone();
        let c = b.canvas(&format!("line-{id}"), rect(PAD, y, w, h));
        c.on_draw(move |r| {
            let current = UI.with(|u| u.current.borrow().as_deref() == Some(&line_id));
            if hit {
                draw::fill_round(r, 6.0, &theme::alpha(&theme::amber(), 0.14));
            } else if current {
                draw::fill_round(r, 6.0, &theme::alpha(&theme::green(), 0.12));
            }
            let base = 4.0 + text.font.ascender();
            stamp_t.draw_at(
                6.0 + stamp_w - stamp_t.width(),
                base - stamp_t.font.ascender(),
            );
            if let Some(n) = &who {
                n.draw_in(rect(
                    6.0 + stamp_w + 10.0,
                    base - n.font.ascender(),
                    64.0,
                    15.0,
                ));
            }
            if hit {
                let s = draw::highlighted(
                    &text,
                    &ranges,
                    &theme::font(13.0, Weight::Semibold),
                    &theme::alpha(&theme::amber(), 0.35),
                );
                use objc2_app_kit::NSAttributedStringNSExtendedStringDrawing;
                s.drawWithRect_options_context(
                    rect(text_x, 4.0, text_w, r.size.height - 8.0),
                    objc2_app_kit::NSStringDrawingOptions::UsesLineFragmentOrigin,
                    None,
                );
            } else {
                text.draw_in(rect(text_x, 4.0, text_w, r.size.height - 8.0));
            }
        });
        let start = seg.start;
        let t = take.clone();
        c.on_click(move || {
            UI.with(|u| u.following.set(true));
            if player::with(|p| p.media.is_none())
                && let Some(best) = TakeMedia::all(&t).first()
            {
                player::load(best, &t, false);
            }
            player::play_from(start);
        })
        .tip(&trf(
            "Play from {time}",
            &[("time", &format::timestamp(start))],
        ))
        .a11y(
            &if multi {
                format!("{stamp}, {}, {}", doc.speaker_name(&seg.speaker), seg.text)
            } else {
                format!("{stamp}, {}", seg.text)
            },
            None,
        );
        lines.push((id, y, h));
        y += h + 2.0;
    }
    UI.with(|u| *u.lines.borrow_mut() = lines);
    let footer = match &doc.language {
        Some(l) => format!("{} · {l}", doc.model),
        None => doc.model.clone(),
    };
    let f = Text::new(footer, caption(), NSColor::tertiaryLabelColor());
    y += 4.0;
    b.canvas("transcript-footer", rect(PAD, y, w, 13.0))
        .on_draw(move |_| f.draw_at(0.0, 0.0));
    y + 13.0
}

/// "Follow Along", over the bottom-right of the body while the user reads elsewhere.
fn follow_button(b: &mut Build, take: &Take, width: f64, height: f64) {
    let _ = take;
    let btn = button(
        b,
        "follow",
        &tr("Follow Along"),
        Some("text.line.first.and.arrowtriangle.forward"),
        true,
        0.0,
        0.0,
        || {
            UI.with(|u| u.following.set(true));
            if let Some(id) = UI.with(|u| u.current.borrow().clone()) {
                scroll_to_line(&id);
            }
            tick();
        },
    );
    let f = btn.frame().size;
    btn.setFrameOrigin(NSPoint::new(
        width - 12.0 - f.width,
        height - 12.0 - f.height,
    ));
    btn.setToolTip(Some(&ns(&tr("Scroll the transcript with playback again"))));
    btn.setHidden(true);
}

// MARK: Several takes

fn several_header(b: &mut Build, takes: &[Take], width: f64) -> f64 {
    let refs: Vec<&Take> = takes.iter().collect();
    let sum = list::summarize(&refs);
    let title = Text::new(
        trn(sum.count as u64, "{n} recording", "{n} recordings"),
        title3(),
        primary(),
    );
    let mut parts = Vec::new();
    if let Some((first, last)) = sum.span {
        parts.push(if format::same_day(first, last) {
            format::day(first)
        } else {
            trf(
                "{first} – {last}",
                &[("first", &format::day(first)), ("last", &format::day(last))],
            )
        });
    }
    parts.push(trf(
        "{length} total",
        &[("length", &format::length(sum.total))],
    ));
    parts.push(trf(
        "{transcribed} of {count} transcribed",
        &[
            ("transcribed", &format::count(sum.transcribed as u64)),
            ("count", &format::count(sum.count as u64)),
        ],
    ));
    if sum.with_video > 0 {
        parts.push(trn(
            sum.with_video as u64,
            "{n} with video",
            "{n} with video",
        ));
    }
    let line = Text::new(parts.join(" · "), callout(), secondary());
    let h = title.height() + 4.0 + line.height();
    b.canvas("several-info", rect(PAD, PAD, width - 2.0 * PAD, h))
        .on_draw(move |_| {
            title.draw_at(0.0, 0.0);
            line.draw_at(0.0, title.height() + 4.0);
        });
    PAD + h + PAD
}

fn active_jobs(takes: &[Take]) -> Vec<Job> {
    store::with(|m| {
        m.plugins
            .jobs
            .iter()
            .filter(|j| takes.iter().any(|t| t.id == j.take_id) && j.is_active())
            .cloned()
            .collect()
    })
}

fn several_body(
    b: &mut Build,
    takes: &[Take],
    transcripts: &std::collections::HashMap<String, TranscriptDocument>,
    width: f64,
) -> f64 {
    let w = width - 2.0 * PAD;
    let mut y = PAD;
    UI.with(|u| u.lines.borrow_mut().clear());
    let active = active_jobs(takes);
    if !active.is_empty() {
        let running: Vec<&Job> = active
            .iter()
            .filter(|j| j.state == JobState::Running)
            .collect();
        let waiting = active.len() - running.len();
        let title = Text::new(
            running.first().map_or_else(
                || tr("Waiting to start"),
                |j| {
                    trf(
                        "{title}: {message}",
                        &[
                            ("title", &j.title),
                            (
                                "message",
                                &j.message.clone().unwrap_or_else(|| tr("working")),
                            ),
                        ],
                    )
                },
            ),
            theme::label_bold(),
            primary(),
        );
        let more = (waiting > 0).then(|| {
            Text::new(
                trn(waiting as u64, "{n} more queued", "{n} more queued"),
                theme::label(),
                secondary(),
            )
        });
        let h = 10.0 + 15.0 + if more.is_some() { 16.0 } else { 0.0 } + 10.0;
        let ids: Vec<u64> = active.iter().map(|j| j.id).collect();
        let cancel = button(
            b,
            "cancel-all",
            &tr("Cancel All"),
            None,
            true,
            0.0,
            0.0,
            move || {
                for id in &ids {
                    store::cancel_job(*id);
                }
            },
        );
        let cs = cancel.frame().size;
        cancel.setFrameOrigin(NSPoint::new(
            PAD + w - 10.0 - cs.width,
            y + (h - cs.height) / 2.0,
        ));
        let fraction = running.first().map_or(0.0, |j| j.fraction);
        let spin = b.native(
            "several-progress",
            rect(PAD + 10.0, y + (h - 16.0) / 2.0, 16.0, 16.0),
            || {
                let p = NSProgressIndicator::new(util::mtm());
                p.setStyle(NSProgressIndicatorStyle::Spinning);
                p.setControlSize(NSControlSize::Small);
                p.setIndeterminate(false);
                p.setMaxValue(1.0);
                p
            },
        );
        spin.setDoubleValue(fraction);
        let bg = b.canvas("several-jobs", rect(PAD, y, w, h));
        bg.on_draw(move |r| {
            draw::fill_round(r, 8.0, &theme::face());
            title.draw_in(rect(
                36.0,
                10.0,
                r.size.width - 46.0 - cs.width - 10.0,
                15.0,
            ));
            if let Some(m) = &more {
                m.draw_at(36.0, 26.0);
            }
        });
        send_back(&bg);
        y += h + 18.0;
    }

    // ACTIONS: rows that run on all of it.
    y += caps_label(b, "actions-label", &tr("ACTIONS"), PAD, y) + 6.0;
    let n = takes.len();
    let untranscribed: Vec<Take> = takes
        .iter()
        .filter(|t| !t.has_transcript())
        .cloned()
        .collect();
    let transcribed = n - untranscribed.len();
    let all_or = |k: usize| {
        if k == n {
            trf("All {n}", &[("n", &n.to_string())])
        } else {
            trf(
                "{k} of {n}",
                &[("k", &k.to_string()), ("n", &n.to_string())],
            )
        }
    };
    type RowSpec = (String, Option<String>, &'static str, bool, Box<dyn Fn()>);
    let mut rows: Vec<RowSpec> = Vec::new();
    if let Some((plugin, action)) = store::with(|m| {
        m.plugins
            .transcribe_action(None)
            .map(|(p, a)| (p.id().to_string(), a.id.clone()))
    }) && !untranscribed.is_empty()
        && untranscribed.len() < n
    {
        let u = untranscribed.clone();
        rows.push((
            trf(
                "Transcribe the {n} without a transcript",
                &[("n", &u.len().to_string())],
            ),
            None,
            "text.quote",
            false,
            Box::new(move || store::run_action(&plugin, &action, &u)),
        ));
    }
    let videos: Vec<Take> = takes.iter().filter(|t| t.has("video")).cloned().collect();
    if !videos.is_empty() {
        let v = videos.clone();
        rows.push((
            tr("Remake video"),
            Some(all_or(videos.len())),
            "film",
            false,
            Box::new(move || store::make_video(&v, false)),
        ));
    }
    let plugin_actions: Vec<(String, String, String, &'static str, Vec<Take>)> = store::with(|m| {
        m.plugins
            .actions()
            .into_iter()
            .filter_map(|(p, a)| {
                let fit: Vec<Take> = takes.iter().filter(|t| a.applies(t)).cloned().collect();
                let symbol = if p.manifest.produces("transcript") {
                    "text.quote"
                } else if p.manifest.produces("video") {
                    "film"
                } else {
                    "puzzlepiece.extension"
                };
                (!fit.is_empty()).then(|| {
                    (
                        p.id().to_string(),
                        a.id.clone(),
                        a.title.clone(),
                        symbol,
                        fit,
                    )
                })
            })
            .collect()
    });
    for (plugin, action, title, symbol, fit) in plugin_actions {
        let detail = all_or(fit.len());
        rows.push((
            title,
            Some(detail),
            symbol,
            false,
            Box::new(move || store::run_action(&plugin, &action, &fit)),
        ));
    }
    if transcribed > 0 {
        let t = takes.to_vec();
        rows.push((
            tr("Copy transcripts"),
            Some(trf("{n} as text", &[("n", &transcribed.to_string())])),
            "doc.on.doc",
            false,
            Box::new(move || recordings::copy_transcripts(&t)),
        ));
    }
    let t = takes.to_vec();
    rows.push((
        tr("Show in Finder"),
        Some(trn(n as u64, "{n} take", "{n} takes")),
        "folder",
        false,
        Box::new(move || recordings::reveal(&t)),
    ));
    let t = takes.to_vec();
    let files: usize = takes.iter().map(|t| t.files.len()).sum();
    rows.push((
        tr("Move to Trash…"),
        Some(trn(files as u64, "{n} file", "{n} files")),
        "trash",
        true,
        Box::new(move || recordings::ask_to_delete(Some(t.clone()))),
    ));

    let row_h = 8.0 + 15.0 + 8.0;
    let list_h = rows.len() as f64 * row_h + (rows.len() as f64 - 1.0) * 1.0;
    let list_top = y;
    b.canvas("actions-box", rect(PAD, y, w, list_h))
        .on_draw(|r| {
            draw::fill_round(r, 8.0, &theme::face());
        });
    let count = rows.len();
    for (i, (title, detail, symbol, destructive, run)) in rows.into_iter().enumerate() {
        let key = format!("action-{i}");
        let hovering = UI.with(|u| u.hover.borrow().as_deref() == Some(&key));
        let title_t = Text::new(
            title,
            callout(),
            if destructive {
                theme::rec_text()
            } else {
                primary()
            },
        );
        let detail_t = detail.map(|d| Text::new(d, caption(), secondary()));
        let last = i + 1 == count;
        let c = b.canvas(&key, rect(PAD, y, w, row_h + if last { 0.0 } else { 1.0 }));
        c.on_draw(move |r| {
            if hovering {
                draw::fill_rect(
                    rect(0.0, 0.0, r.size.width, row_h),
                    &theme::alpha(&NSColor::labelColor(), 0.06),
                );
            }
            let tint = if destructive {
                theme::rec_text()
            } else {
                secondary()
            };
            draw::draw_symbol(
                symbol,
                12.0,
                Weight::Regular,
                &tint,
                10.0 + 8.0,
                row_h / 2.0,
            );
            title_t.draw_at(10.0 + 16.0 + 10.0, 8.0);
            if let Some(d) = &detail_t {
                d.draw_at(r.size.width - 10.0 - d.width(), (row_h - d.height()) / 2.0);
            }
            if !last {
                draw::fill_rect(
                    rect(36.0, row_h, r.size.width - 36.0, 1.0),
                    &NSColor::separatorColor(),
                );
            }
        });
        let hk = key.clone();
        c.on_click(move || run()).on_hover(move |inside| {
            UI.with(|u| *u.hover.borrow_mut() = inside.then(|| hk.clone()));
            if let Some(v) = recordings::with(|s| s.detail.body.get(&hk)).flatten() {
                v.setNeedsDisplay(true);
            }
        });
        y += row_h + if last { 0.0 } else { 1.0 };
    }
    b.canvas("actions-border", rect(PAD, list_top, w, list_h))
        .on_draw(|r| draw::stroke_round(r, 8.0, 0.5, &theme::hairline()));
    y += 18.0;

    // SELECTED: each take; click to show only it.
    y += caps_label(b, "selected-label", &tr("SELECTED"), PAD, y) + 6.0;
    for take in takes {
        let snippet = list::snippet(transcripts.get(&take.id));
        let when = Text::new(
            take.started
                .map(format::day_time)
                .unwrap_or_else(|| take.id.clone()),
            callout(),
            primary(),
        );
        let length = Text::new(
            format::length(take.duration.unwrap_or(0.0)),
            theme::digits(12.0, Weight::Regular),
            secondary(),
        );
        let (badge, _, quiet) = recordings::media_badge(take);
        let what = Text::new(
            if snippet.is_empty() {
                take.speaker_names()
            } else {
                snippet
            },
            callout(),
            secondary(),
        );
        let job = store::with(|m| m.plugins.job(&take.id, None).cloned());
        let status = job.map(|j| {
            Text::new(
                if j.state == JobState::Queued {
                    tr("Queued")
                } else {
                    j.message.unwrap_or_else(|| tr("Working"))
                },
                caption(),
                secondary(),
            )
        });
        let h = 5.0 + 15.0 + 5.0;
        let id = take.id.clone();
        b.canvas(&format!("selected-{}", take.id), rect(PAD, y, w, h))
            .on_draw(move |r| {
                when.draw_in(rect(8.0, 5.0, 130.0, 15.0));
                length.draw_at(8.0 + 130.0 + 10.0, 5.0);
                if let Some(name) = badge {
                    let tint = if quiet { secondary() } else { primary() };
                    draw::draw_symbol(
                        name,
                        12.0,
                        Weight::Regular,
                        &tint,
                        8.0 + 130.0 + 10.0 + 48.0 + 10.0 + 9.0,
                        h / 2.0,
                    );
                }
                let sx = 8.0 + 130.0 + 10.0 + 48.0 + 10.0 + 18.0 + 10.0;
                let sw = status.as_ref().map_or(0.0, |s| s.width() + 10.0);
                what.draw_in(rect(
                    sx,
                    5.0,
                    (r.size.width - 8.0 - sx - sw).max(10.0),
                    15.0,
                ));
                if let Some(s) = &status {
                    s.draw_at(r.size.width - 8.0 - s.width(), (h - s.height()) / 2.0);
                }
            })
            .on_click(move || recordings::select_only(&id))
            .tip(&tr("Show only this recording"));
        y += h;
    }
    y + PAD
}

#[cfg(test)]
mod tests {
    use super::hits;

    #[test]
    fn search_hits_ignore_case() {
        assert_eq!(hits("Mic and mic", "MIC"), vec![(0, 3), (8, 11)]);
        assert!(hits("anything", "").is_empty());
        assert!(hits("abc", "x").is_empty());
    }
}
