//! Settings, as `SettingsView.swift`: General, Video and Plugins tabs in a toolbar-tabbed
//! window, each a grouped form (section headers, rounded boxes of rows with a label on the
//! left and a switch, menu or button on the right, quiet notes) like SwiftUI's
//! `.formStyle(.grouped)`.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAlert, NSAppearanceCustomization, NSBezelStyle, NSButton, NSColor, NSControlSize,
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSModalResponseOK, NSOpenPanel,
    NSPopUpButton, NSScrollView, NSSwitch, NSTabViewController, NSTabViewControllerTabStyle,
    NSTabViewItem, NSTextField, NSViewController, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize};

use crate::canvas::{Build, Canvas, FieldDelegate, Layer, on_action};
use crate::draw::{self, Text, rect};
use crate::i18n::tr;
use crate::model::plugins::{JsonValue, Plugin, PluginSetting, SettingKind};
use crate::model::settings::RecordingFormat;
use crate::permissions::{self, State as Permission};
use crate::theme::{self, Weight};
use crate::util::{self, ns};
use crate::{store, util::tilde};

const WIDTH: f64 = 520.0;
const MARGIN: f64 = 20.0;
const INSET: f64 = 10.0;
const ROW: f64 = 36.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Video,
    Plugins,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::General, Tab::Video, Tab::Plugins];
}

struct Page {
    tab: Tab,
    scroll: Retained<NSScrollView>,
    layer: Layer,
    controller: Retained<NSViewController>,
}

struct State {
    window: Retained<NSWindow>,
    tabs: Retained<NSTabViewController>,
    pages: Vec<Page>,
    _delegate: Retained<Delegate>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static REBUILDING: Cell<bool> = const { Cell::new(false) };
}

define_class!(
    // SAFETY: NSObject subclass acting as the window delegate; no Drop impl.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckSettingsDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _n: &NSNotification) {
            util::later(0.0, || {
                STATE.with(|s| s.borrow_mut().take());
            });
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, _n: &NSNotification) {
            // Permissions may have changed in System Settings.
            util::later(0.0, refresh);
        }
    }
);

define_class!(
    // SAFETY: NSTabViewController subclass that only resizes the window to the chosen tab.
    #[unsafe(super = NSTabViewController)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckSettingsTabs"]
    struct Tabs;

    impl Tabs {
        #[unsafe(method(tabView:didSelectTabViewItem:))]
        fn did_select(&self, tab_view: &objc2_app_kit::NSTabView, item: Option<&NSTabViewItem>) {
            // SAFETY: plain call to the superclass implementation.
            let _: () = unsafe { msg_send![super(self), tabView: tab_view, didSelectTabViewItem: item] };
            util::later(0.0, fit_window);
        }
    }
);

/// Sizes the window to the selected tab's form, keeping its top edge.
fn fit_window() {
    let Some((window, tabs)) = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|s| (s.window.clone(), s.tabs.clone()))
    }) else {
        return;
    };
    let i = tabs.selectedTabViewItemIndex();
    let Some(item) = (i >= 0).then(|| tabs.tabViewItems().objectAtIndex(i as usize)) else {
        return;
    };
    let Some(vc) = item.viewController(util::mtm()) else {
        return;
    };
    let size = vc.preferredContentSize();
    let frame = window.frame();
    let content = window.contentRectForFrameRect(frame);
    let extra = frame.size.height - content.size.height;
    let h = size.height + extra;
    if (frame.size.height - h).abs() > 0.5 {
        let top = frame.origin.y + frame.size.height;
        window.setFrame_display_animate(
            rect(frame.origin.x, top - h, WIDTH, h),
            true,
            window.isVisible() && !util::reduce_motion(),
        );
    }
}

// MARK: Colours of the grouped form

fn pair(light: u32, dark: u32) -> Retained<NSColor> {
    theme::pair(light, dark)
}

fn form_bg() -> Retained<NSColor> {
    pair(0xF6F6F6, 0x282828)
}

fn box_fill() -> Retained<NSColor> {
    pair(0xF2F2F2, 0x2B2B2B)
}

fn box_border() -> Retained<NSColor> {
    pair(0xE4E4E4, 0x454545)
}

fn separator() -> Retained<NSColor> {
    pair(0xE7E7E7, 0x353535)
}

fn body() -> Retained<objc2_app_kit::NSFont> {
    theme::font(13.0, Weight::Regular)
}

// MARK: Opening

/// Opens Settings (or brings it forward).
pub fn show() {
    crate::panel::close();
    crate::app::activate();
    if let Some(w) = STATE.with(|s| s.borrow().as_ref().map(|s| s.window.clone())) {
        w.makeKeyAndOrderFront(None);
        return;
    }
    build();
    refresh();
    if let Some(w) = STATE.with(|s| s.borrow().as_ref().map(|s| s.window.clone())) {
        w.center();
        w.makeKeyAndOrderFront(None);
    }
}

fn build() {
    let mtm = util::mtm();
    let tabs: Retained<NSTabViewController> = {
        let this = Tabs::alloc(mtm).set_ivars(());
        let t: Retained<Tabs> = unsafe { msg_send![super(this), init] };
        Retained::into_super(t)
    };
    tabs.setTabStyle(NSTabViewControllerTabStyle::Toolbar);
    let mut pages = Vec::new();
    for tab in Tab::ALL {
        let (title, symbol) = match tab {
            Tab::General => (tr("General"), "gearshape"),
            Tab::Video => (tr("Video"), "video"),
            Tab::Plugins => (tr("Plugins"), "puzzlepiece.extension"),
        };
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0.0, 0.0, WIDTH, 400.0));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        let root = Canvas::new(mtm, rect(0.0, 0.0, WIDTH, 400.0));
        root.on_draw(|r| draw::fill_rect(r, &form_bg()));
        scroll.setDocumentView(Some(&root));
        let controller = NSViewController::new(mtm);
        controller.setView(&scroll);
        controller.setTitle(Some(&ns(&title)));
        let item = NSTabViewItem::tabViewItemWithViewController(&controller);
        item.setLabel(&ns(&title));
        item.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(symbol), None)
                .as_deref(),
        );
        tabs.addTabViewItem(&item);
        pages.push(Page {
            tab,
            scroll,
            layer: Layer::new(Retained::into_super(root)),
            controller,
        });
    }
    let window = crate::shots::window(
        rect(0.0, 0.0, WIDTH, 400.0),
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable,
    );
    window.setContentViewController(Some(&tabs));
    // SAFETY: kept in STATE until it closes.
    unsafe { window.setReleasedWhenClosed(false) };
    let delegate: Retained<Delegate> = {
        let this = Delegate::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    };
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            window,
            tabs,
            pages,
            _delegate: delegate,
        })
    });
}

/// Rebuilds every tab from the model.
pub fn refresh() {
    if REBUILDING.with(|r| r.replace(true)) {
        return;
    }
    let pages: Vec<(Tab, Retained<NSScrollView>, Retained<NSViewController>)> = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|s| {
                s.pages
                    .iter()
                    .map(|p| (p.tab, p.scroll.clone(), p.controller.clone()))
                    .collect()
            })
            .unwrap_or_default()
    });
    for (tab, scroll, controller) in pages {
        let height = STATE.with(|s| {
            let s = s.borrow();
            let page = s.as_ref()?.pages.iter().find(|p| p.tab == tab)?;
            let mut f = Form::new(page.layer.build());
            match tab {
                Tab::General => general(&mut f),
                Tab::Video => video(&mut f),
                Tab::Plugins => plugins(&mut f),
            }
            let h = f.finish();
            page.layer.root.setFrame(rect(0.0, 0.0, WIDTH, h));
            Some(h)
        });
        let Some(height) = height else { continue };
        // The Video tab is tall; it scrolls past a comfortable height.
        let min = match tab {
            Tab::Video => 520.0,
            Tab::Plugins => 360.0,
            Tab::General => 0.0,
        };
        let shown = height.max(min).min(720.0);
        controller.setPreferredContentSize(NSSize::new(WIDTH, shown));
        if let Some(doc) = scroll.documentView() {
            doc.setFrameSize(NSSize::new(WIDTH, height.max(shown)));
        }
    }
    REBUILDING.with(|r| r.set(false));
    fit_window();
}

// MARK: The form builder

/// Lays out a grouped form top-down: sections of rows in rounded boxes.
struct Form<'a> {
    b: Build<'a>,
    y: f64,
    sections: usize,
    /// The open box: its key, top, and the separators drawn between its rows.
    open: Option<(String, f64, Vec<f64>)>,
    rows: usize,
}

impl<'a> Form<'a> {
    fn new(b: Build<'a>) -> Form<'a> {
        Form {
            b,
            y: 0.0,
            sections: 0,
            open: None,
            rows: 0,
        }
    }

    /// Starts a section, with a header when it has one.
    fn section(&mut self, header: Option<&str>) {
        self.sections += 1;
        let gap = if self.sections == 1 { 0.0 } else { 10.5 };
        let key = format!("box-{}", self.sections);
        // The box goes in first, so its rows draw over it.
        let bg = self
            .b
            .canvas(&key, rect(MARGIN, 0.0, WIDTH - 2.0 * MARGIN, 0.0));
        let _ = bg;
        match header {
            Some(text) => {
                let t = Text::new(
                    text.to_string(),
                    theme::font(13.0, Weight::Bold),
                    NSColor::labelColor(),
                );
                // The header's baseline sits 32 pt below the section's top, 14 pt above its box.
                let top = self.y + gap + 32.0 - t.font.ascender();
                let th = t.height();
                self.b
                    .canvas(
                        &format!("header-{}", self.sections),
                        rect(MARGIN + INSET, top, WIDTH - 2.0 * MARGIN, th),
                    )
                    .on_draw(move |_| t.draw_at(0.0, 0.0));
                self.y += gap + 46.0;
            }
            // Boxes without a header sit 10 pt apart.
            None => self.y += if self.sections == 1 { MARGIN } else { 10.0 },
        }
        self.open = Some((key, self.y, Vec::new()));
        self.rows = 0;
    }

    /// Closes the section's box, with a note under it (trailing, as macOS forms align footers).
    fn end(&mut self, footer: Option<&str>) {
        if let Some((key, top, separators)) = self.open.take() {
            let h = self.y - top;
            let bg = self
                .b
                .canvas(&key, rect(MARGIN, top, WIDTH - 2.0 * MARGIN, h));
            bg.on_draw(move |r| {
                draw::fill_round(r, 6.0, &box_fill());
                draw::stroke_round(r, 6.0, 0.5, &box_border());
                for y in &separators {
                    draw::fill_rect(
                        rect(INSET, y - top, r.size.width - 2.0 * INSET, 1.0),
                        &separator(),
                    );
                }
            });
        }
        if let Some(text) = footer {
            let w = WIDTH - 2.0 * MARGIN - INSET - 5.0;
            let t = Text::new(
                text.to_string(),
                theme::font(10.0, Weight::Regular),
                NSColor::secondaryLabelColor(),
            )
            .wrapping();
            let h = t.wrapped_height(w);
            let y = self.y + 8.0;
            let key = format!("footer-{}", self.sections);
            self.b
                .canvas(&key, rect(MARGIN + 5.0, y, w, h))
                .on_draw(move |r| {
                    // Each line ends at the right edge.
                    let style = objc2_app_kit::NSMutableParagraphStyle::new();
                    style.setAlignment(objc2_app_kit::NSTextAlignment::Right);
                    let a = t.attributed();
                    let m = objc2_foundation::NSMutableAttributedString::initWithAttributedString(
                        objc2_foundation::NSMutableAttributedString::alloc(),
                        &a,
                    );
                    // SAFETY: a paragraph style over the whole string.
                    unsafe {
                        m.addAttribute_value_range(
                            objc2_app_kit::NSParagraphStyleAttributeName,
                            &style,
                            objc2_foundation::NSRange::new(0, m.length()),
                        )
                    };
                    use objc2_app_kit::NSAttributedStringNSExtendedStringDrawing;
                    m.drawWithRect_options_context(
                        r,
                        objc2_app_kit::NSStringDrawingOptions::UsesLineFragmentOrigin,
                        None,
                    );
                });
            self.y = y + h;
        }
    }

    /// Starts a row of `height`; returns its top. Rows after the first get a separator.
    fn row(&mut self, height: f64) -> f64 {
        if self.rows > 0 {
            if let Some((_, _, seps)) = self.open.as_mut() {
                seps.push(self.y);
            }
            self.y += 1.0;
        }
        self.rows += 1;
        let top = self.y;
        self.y += height;
        top
    }

    fn label(&mut self, key: &str, text: &str, color: Retained<NSColor>, x: f64, y: f64, w: f64) {
        let t = Text::new(text.to_string(), body(), color);
        self.b
            .canvas(key, rect(x, y, w, t.height()))
            .on_draw(move |r| t.draw_in(r));
    }

    /// A label and a switch.
    fn toggle(
        &mut self,
        key: &str,
        label: &str,
        on: bool,
        enabled: bool,
        f: impl Fn(bool) + 'static,
    ) -> Retained<NSSwitch> {
        let top = self.row(ROW);
        let right = WIDTH - MARGIN - INSET;
        let sw = self.switch(key, on, enabled, right, top + (ROW - 15.0) / 2.0, f);
        let label_color = if enabled {
            NSColor::labelColor()
        } else {
            NSColor::disabledControlTextColor()
        };
        let h = Text::new(label.to_string(), body(), label_color.clone()).height();
        self.label(
            &format!("{key}-label"),
            label,
            label_color,
            MARGIN + INSET,
            top + (ROW - h) / 2.0,
            sw.frame().origin.x - MARGIN - INSET - 8.0,
        );
        sw
    }

    /// Places a mini switch ending at `right`.
    fn switch(
        &mut self,
        key: &str,
        on: bool,
        enabled: bool,
        right: f64,
        y: f64,
        f: impl Fn(bool) + 'static,
    ) -> Retained<NSSwitch> {
        let s = self.b.native(key, rect(right - 26.0, y, 26.0, 15.0), || {
            let s = NSSwitch::new(util::mtm());
            s.setControlSize(NSControlSize::Mini);
            s
        });
        s.setFrame(rect(right - 26.0, y, 26.0, 15.0));
        s.setState(if on {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        s.setEnabled(enabled);
        let s2 = s.clone();
        on_action(&s, move || f(s2.state() == NSControlStateValueOn));
        s
    }

    /// A label and a menu of choices.
    fn picker(
        &mut self,
        key: &str,
        label: &str,
        options: &[String],
        selected: Option<usize>,
        enabled: bool,
        f: impl Fn(usize) + 'static,
    ) {
        let top = self.row(ROW);
        let p = self.b.native(key, rect(0.0, 0.0, 10.0, 10.0), || {
            let p = NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(util::mtm()),
                rect(0.0, 0.0, 100.0, 22.0),
                false,
            );
            p.setBordered(false);
            p
        });
        p.removeAllItems();
        for o in options {
            p.addItemWithTitle(&ns(o));
        }
        if let Some(i) = selected {
            p.selectItemAtIndex(i as isize);
        }
        p.setEnabled(enabled);
        p.sizeToFit();
        // As wide as the chosen title, not the longest one.
        let title = selected
            .and_then(|i| options.get(i))
            .cloned()
            .unwrap_or_default();
        let pw = Text::new(title, body(), NSColor::labelColor()).width() + 30.0;
        let ph = p.frame().size.height;
        p.setFrame(rect(
            WIDTH - MARGIN - INSET - pw + 4.0,
            top + (ROW - ph) / 2.0,
            pw,
            ph,
        ));
        let p2 = p.clone();
        on_action(&p, move || f(p2.indexOfSelectedItem().max(0) as usize));
        let color = if enabled {
            NSColor::labelColor()
        } else {
            NSColor::disabledControlTextColor()
        };
        let h = Text::new(label.to_string(), body(), color.clone()).height();
        self.label(
            &format!("{key}-label"),
            label,
            color,
            MARGIN + INSET,
            top + (ROW - h) / 2.0,
            WIDTH - 2.0 * MARGIN - 2.0 * INSET - pw - 8.0,
        );
    }

    /// A quiet explanation inside the box.
    fn note(&mut self, key: &str, text: &str) {
        let w = WIDTH - 2.0 * MARGIN - 2.0 * INSET;
        let t = Text::new(text.to_string(), body(), NSColor::secondaryLabelColor()).wrapping();
        let h = t.wrapped_height(w);
        let top = self.row(h + 20.0);
        self.b
            .canvas(key, rect(MARGIN + INSET, top + 10.0, w, h))
            .on_draw(move |r| t.draw_in(r));
    }

    /// A push button.
    fn button(
        &mut self,
        key: &str,
        title: &str,
        small: bool,
        x: f64,
        y: f64,
        f: impl Fn() + 'static,
    ) -> Retained<NSButton> {
        let b = self.b.native(key, rect(x, y, 10.0, 10.0), || {
            let b = NSButton::new(util::mtm());
            b.setBezelStyle(NSBezelStyle::Push);
            b
        });
        b.setTitle(&ns(title));
        b.setControlSize(if small {
            NSControlSize::Small
        } else {
            NSControlSize::Regular
        });
        b.sizeToFit();
        let s = b.frame().size;
        b.setFrame(rect(x, y, s.width, s.height));
        on_action(&b, f);
        b
    }

    fn finish(mut self) -> f64 {
        if self.open.is_some() {
            self.end(None);
        }
        let h = self.y + MARGIN;
        self.b.finish();
        h
    }
}

// MARK: General

fn general(f: &mut Form) {
    let s = store::with(|s| s.clone());
    f.section(Some(&tr("Recordings")));
    // Folder: the path and Choose…, under the label when they don't fit beside it.
    let top = f.row(60.0);
    f.label(
        "folder-label",
        &tr("Folder"),
        NSColor::labelColor(),
        MARGIN + INSET,
        top + 10.0,
        200.0,
    );
    let choose = f.button(
        "folder-choose",
        &tr("Choose…"),
        false,
        0.0,
        0.0,
        choose_folder,
    );
    let cs = choose.frame().size;
    let right = WIDTH - MARGIN - INSET;
    choose.setFrameOrigin(NSPoint::new(right - cs.width, top + 30.0));
    // A folder reads with its trailing slash, as Foundation writes directory URLs.
    let path = Text::new(
        format!(
            "{}/",
            tilde(&s.settings.recordings_root).trim_end_matches('/')
        ),
        body(),
        NSColor::secondaryLabelColor(),
    )
    .middle();
    let pw = (right - cs.width - 8.0 - (MARGIN + INSET + 4.0)).max(40.0);
    let shown = path.width().min(pw);
    let px = right - cs.width - 8.0 - shown;
    f.b.canvas("folder-path", rect(px, top + 33.0, shown, path.height()))
        .on_draw(move |r| path.draw_in(r));

    let formats = [tr("WAV 24-bit (lossless)"), tr("M4A (AAC 256 kbps)")];
    let selected = if s.settings.format == RecordingFormat::Wav {
        0
    } else {
        1
    };
    f.picker(
        "format",
        &tr("Format"),
        &formats,
        Some(selected),
        true,
        |i| {
            store::set_settings(|s| {
                s.format = if i == 0 {
                    RecordingFormat::Wav
                } else {
                    RecordingFormat::M4a
                }
            })
        },
    );
    f.toggle(
        "split",
        &tr("Save TX1 and TX2 as separate files in stereo"),
        s.settings.split_tracks,
        s.settings.format == RecordingFormat::Wav,
        |on| store::set_settings(|s| s.split_tracks = on),
    );
    f.note("recordings-note", &tr("Files go into a dated folder, named by start time. Split files need WAV and stereo mode."));
    f.end(None);

    f.section(Some(&tr("Controls")));
    f.toggle(
        "tap",
        &tr("Tap a mic’s link button to start or stop recording"),
        s.settings.tap_to_record,
        true,
        |on| store::set_settings(|s| s.tap_to_record = on),
    );
    f.note("controls-note", &tr("While on, the receiver’s button presses no longer change the Mac’s volume. Shortcut: ⌥⌘R."));
    f.toggle(
        "login",
        &tr("Open at login"),
        s.launch_at_login,
        true,
        store::set_launch_at_login,
    );
    f.end(None);
}

fn choose_folder() {
    let mtm = util::mtm();
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setCanCreateDirectories(true);
    let root = store::with(|s| s.settings.recordings_root.clone());
    panel.setDirectoryURL(Some(&util::file_url(&root)));
    if panel.runModal() == NSModalResponseOK
        && let Some(url) = panel.URL()
        && let Some(path) = url.path()
    {
        let path = PathBuf::from(path.to_string());
        store::set_settings(move |s| s.recordings_root = path);
    }
}

// MARK: Video

fn video(f: &mut Form) {
    let v = store::with(|s| s.settings.video.clone());
    f.section(Some(&tr("Record with the mics")));
    f.toggle("screen", &tr("Record the screen"), v.screen, true, |on| {
        store::set_video_source("screen", on)
    });
    f.toggle("camera", &tr("Record the camera"), v.camera, true, |on| {
        store::set_video_source("camera", on)
    });
    permission_row(
        f,
        "video-permissions",
        &[permissions::SCREEN, permissions::CAMERA],
        &tr("Permissions"),
    );
    f.end(Some(&tr(
        "Also on the deck, as SCREEN and CAMERA. Lavdeck’s own windows and the camera preview are left out of the screen recording.",
    )));

    f.section(Some(&tr("Camera preview")));
    f.toggle(
        "preview",
        &tr("Show a camera preview while recording"),
        v.camera_preview,
        true,
        |on| store::set_settings(|s| s.video.camera_preview = on),
    );
    let sizes = [
        ("bubble", tr("Bubble")),
        ("large", tr("Large")),
        ("full", tr("Full screen")),
    ];
    choice(
        f,
        "preview-size",
        &tr("Starts as"),
        &sizes,
        &v.preview_size,
        v.camera_preview,
        |s, k| s.video.preview_size = k,
    );
    f.note("preview-note", &tr("Double-click the preview to switch between a bubble, a large view of the whole frame, and full screen."));
    f.end(None);

    f.section(Some(&tr("Screen recording")));
    let audio = f.toggle(
        "system-audio",
        &tr("Also record system audio"),
        v.system_audio,
        true,
        |on| store::set_settings(|s| s.video.system_audio = on),
    );
    audio.setToolTip(Some(&ns(&tr(
        "Sound from apps (a video you’re demoing, a call) as a second audio track in the screen recording and the finished video.",
    ))));
    let fps = [("30", tr("30 fps")), ("60", tr("60 fps"))];
    choice(
        f,
        "fps",
        &tr("Frame rate"),
        &fps,
        &v.screen_fps.to_string(),
        true,
        |s, k| s.video.screen_fps = k.parse().unwrap_or(30),
    );
    let codecs = [
        ("hevc", tr("HEVC (smaller files)")),
        ("h264", tr("H.264 (plays everywhere)")),
    ];
    choice(
        f,
        "codec",
        &tr("Video format"),
        &codecs,
        &v.codec,
        true,
        |s, k| s.video.codec = k,
    );
    f.end(None);

    f.section(Some(&tr("Finished video")));
    f.toggle(
        "make-video",
        &tr("Make a finished video after each take"),
        v.make_video,
        true,
        |on| store::set_settings(|s| s.video.make_video = on),
    );
    let corners = [
        ("bottom-right", tr("Bottom right")),
        ("bottom-left", tr("Bottom left")),
        ("top-right", tr("Top right")),
        ("top-left", tr("Top left")),
    ];
    choice(
        f,
        "corner",
        &tr("Camera bubble position"),
        &corners,
        &v.bubble_corner,
        true,
        |s, k| s.video.bubble_corner = k,
    );
    let bubble_sizes = [
        ("small", tr("Small")),
        ("medium", tr("Medium")),
        ("large", tr("Large")),
    ];
    choice(
        f,
        "bubble-size",
        &tr("Camera bubble size"),
        &bubble_sizes,
        &v.bubble_size,
        true,
        |s, k| s.video.bubble_size = k,
    );
    let shapes = [
        ("circle", tr("Circle")),
        ("rounded", tr("Rounded rectangle")),
    ];
    choice(
        f,
        "shape",
        &tr("Camera bubble shape"),
        &shapes,
        &v.bubble_shape,
        true,
        |s, k| s.video.bubble_shape = k,
    );
    f.end(Some(&tr(
        "One MP4 per take: your screen with the camera in a corner (or the camera alone), and the mic audio. To apply new bubble settings to an earlier take, use Remake Video in Recordings.",
    )));
}

/// A picker over keyed choices that writes the chosen key into the settings.
fn choice(
    f: &mut Form,
    key: &str,
    label: &str,
    options: &[(&str, String)],
    current: &str,
    enabled: bool,
    set: fn(&mut crate::model::settings::Settings, String),
) {
    let titles: Vec<String> = options.iter().map(|(_, t)| t.clone()).collect();
    let keys: Vec<String> = options.iter().map(|(k, _)| k.to_string()).collect();
    let selected = keys.iter().position(|k| k == current);
    f.picker(key, label, &titles, selected, enabled, move |i| {
        if let Some(k) = keys.get(i).cloned() {
            store::set_settings(move |s| set(s, k));
        }
    });
}

/// The permissions something relies on, with Lavdeck's current grant for each, and Allow….
fn permission_row(f: &mut Form, key: &str, needed: &[&str], title: &str) {
    let top = f.row(ROW);
    let states: Vec<(String, Permission)> = needed
        .iter()
        .map(|p| (p.to_string(), permissions::state(p)))
        .collect();
    let mut x = MARGIN + INSET;
    let label = Text::new(title.to_string(), body(), NSColor::secondaryLabelColor());
    let lw = label.width();
    let mid = top + ROW / 2.0;
    f.b.canvas(&format!("{key}-title"), rect(x, top, 26.0 + lw + 2.0, ROW))
        .on_draw(move |r| {
            draw::draw_symbol(
                "lock.shield",
                12.0,
                Weight::Regular,
                &NSColor::secondaryLabelColor(),
                7.0,
                r.size.height / 2.0,
            );
            label.draw_at(20.0 + 6.0, (r.size.height - label.height()) / 2.0);
        });
    x += 26.0 + lw + 14.0;
    for (p, state) in &states {
        let granted = *state == Permission::Granted;
        let name = Text::new(permissions::name(p), body(), NSColor::labelColor());
        let w = 20.0 + 4.0 + name.width();
        f.b.canvas(&format!("{key}-{p}"), rect(x, top, w, ROW))
            .on_draw(move |r| {
                let (symbol, color) = if granted {
                    ("checkmark.circle.fill", NSColor::systemGreenColor())
                } else {
                    ("exclamationmark.circle", NSColor::systemOrangeColor())
                };
                draw::draw_symbol(
                    symbol,
                    13.0,
                    Weight::Regular,
                    &color,
                    9.0,
                    r.size.height / 2.0,
                );
                name.draw_at(24.0, (r.size.height - name.height()) / 2.0);
            })
            .tip(&if granted {
                tr("Allowed")
            } else {
                tr("Not allowed yet")
            });
        x += w + 14.0;
    }
    let _ = mid;
    if states.iter().any(|(_, s)| *s != Permission::Granted) {
        let list: Vec<String> = needed.iter().map(|p| p.to_string()).collect();
        let b = f.button(
            &format!("{key}-allow"),
            &tr("Allow…"),
            true,
            0.0,
            0.0,
            move || {
                let refs: Vec<&str> = list.iter().map(String::as_str).collect();
                permissions::request(&refs);
                if let Some(blocked) = refs
                    .iter()
                    .find(|p| permissions::state(p) == Permission::Denied)
                {
                    permissions::open_settings(blocked);
                }
            },
        );
        let s = b.frame().size;
        b.setFrameOrigin(NSPoint::new(
            WIDTH - MARGIN - INSET - s.width + 2.0,
            top + (ROW - s.height) / 2.0,
        ));
    }
}

// MARK: Plugins

fn plugins(f: &mut Form) {
    let host = store::with(|s| s.plugins.clone());
    if host.plugins.is_empty() {
        f.section(None);
        f.note(
            "no-plugins",
            &tr("No plugins installed. Plugins add optional features such as transcription. Install one from a folder, or drop it into the plugins folder."),
        );
        f.end(None);
    }
    for p in &host.plugins {
        f.section(None);
        plugin_header(f, p, host.is_enabled(p.id()));
        if !p.manifest.permissions.is_empty() {
            let needed: Vec<&str> = p.manifest.permissions.iter().map(String::as_str).collect();
            permission_row(
                f,
                &format!("perm-{}", p.id()),
                &needed,
                &tr("Uses Lavdeck’s"),
            );
        }
        if host.is_enabled(p.id()) {
            for setting in &p.manifest.settings {
                setting_row(f, p, setting, host.value(p, &setting.key));
            }
        }
        f.end(None);
    }
    if !host.problems.is_empty() {
        f.section(Some(&tr("Couldn’t load")));
        for (i, problem) in host.problems.iter().enumerate() {
            f.note(&format!("problem-{i}"), problem);
        }
        f.end(None);
    }
    f.section(None);
    let top = f.row(40.0);
    let install = f.button(
        "install",
        &tr("Install Plugin…"),
        false,
        MARGIN + INSET,
        0.0,
        install_plugin,
    );
    let ih = install.frame().size.height;
    install.setFrameOrigin(NSPoint::new(MARGIN + INSET - 2.0, top + (40.0 - ih) / 2.0));
    let iw = install.frame().size.width;
    let open = f.button(
        "open-folder",
        &tr("Open Plugins Folder"),
        false,
        0.0,
        0.0,
        || {
            let folder = store::backend().plugins_folder();
            let _ = std::fs::create_dir_all(&folder);
            util::open(&folder);
        },
    );
    open.setFrameOrigin(NSPoint::new(
        MARGIN + INSET - 2.0 + iw + 6.0,
        top + (40.0 - ih) / 2.0,
    ));
    let reload = f.button(
        "reload",
        &tr("Reload"),
        false,
        0.0,
        0.0,
        store::reload_plugins,
    );
    let rw = reload.frame().size.width;
    reload.setFrameOrigin(NSPoint::new(
        WIDTH - MARGIN - INSET - rw + 2.0,
        top + (40.0 - ih) / 2.0,
    ));
    f.end(Some(&tr(
        "Plugins run as separate programs with your permissions and Lavdeck’s. Lavdeck asks macOS for each permission once, whichever plugins need it. Only install plugins you trust.",
    )));
}

/// The plugin's switch, with its name, summary, and version and id.
fn plugin_header(f: &mut Form, p: &Plugin, enabled: bool) {
    let w = WIDTH - 2.0 * MARGIN - 2.0 * INSET - 26.0 - 12.0;
    let name = Text::new(
        p.manifest.name.clone(),
        theme::font(13.0, Weight::Bold),
        NSColor::labelColor(),
    );
    let summary = p.manifest.summary.clone().map(|s| {
        Text::new(
            s,
            theme::font(12.0, Weight::Regular),
            NSColor::secondaryLabelColor(),
        )
        .wrapping()
    });
    let sh = summary.as_ref().map_or(0.0, |s| s.wrapped_height(w) + 2.0);
    let meta = Text::new(
        format!("{} · {}", p.manifest.version, p.id()),
        theme::font(10.0, Weight::Regular),
        NSColor::tertiaryLabelColor(),
    );
    let h = name.height() + 2.0 + sh + meta.height();
    let top = f.row(h + 20.0);
    let id = p.id().to_string();
    f.switch(
        &format!("enable-{id}"),
        enabled,
        true,
        WIDTH - MARGIN - INSET,
        top + 10.0 + (name.height() - 15.0) / 2.0,
        move |on| store::set_plugin_enabled(&id, on),
    );
    f.b.canvas(
        &format!("plugin-{}", p.id()),
        rect(MARGIN + INSET, top + 10.0, w, h),
    )
    .on_draw(move |r| {
        name.draw_at(0.0, 0.0);
        let mut y = name.height() + 2.0;
        if let Some(s) = &summary {
            let sh = s.wrapped_height(r.size.width);
            s.draw_in(rect(0.0, y, r.size.width, sh));
            y += sh + 2.0;
        }
        meta.draw_at(0.0, y);
    });
}

/// One of a plugin's settings, by kind.
fn setting_row(f: &mut Form, p: &Plugin, setting: &PluginSetting, value: JsonValue) {
    let key = format!("{}-{}", p.id(), setting.key);
    let id = p.id().to_string();
    let skey = setting.key.clone();
    match setting.kind {
        SettingKind::Bool => {
            f.toggle(
                &key,
                &setting.title,
                value.as_bool().unwrap_or(false),
                true,
                move |on| store::set_plugin_value(&id, &skey, JsonValue::Bool(on)),
            );
        }
        SettingKind::Choice => {
            let titles: Vec<String> = setting.options.iter().map(|(_, t)| t.clone()).collect();
            let values: Vec<String> = setting.options.iter().map(|(v, _)| v.clone()).collect();
            let selected = values.iter().position(|v| *v == value.text());
            f.picker(&key, &setting.title, &titles, selected, true, move |i| {
                if let Some(v) = values.get(i) {
                    store::set_plugin_value(&id, &skey, JsonValue::String(v.clone()));
                }
            });
        }
        SettingKind::Number | SettingKind::String | SettingKind::Path => {
            let top = f.row(ROW);
            let label = Text::new(setting.title.clone(), body(), NSColor::labelColor());
            let lw = label.width().min(200.0);
            f.label(
                &format!("{key}-label"),
                &setting.title,
                NSColor::labelColor(),
                MARGIN + INSET,
                top + (ROW - label.height()) / 2.0,
                lw,
            );
            let right = WIDTH - MARGIN - INSET;
            let mut field_right = right;
            if setting.kind == SettingKind::Path {
                let current = value.text();
                let (id2, k2) = (id.clone(), skey.clone());
                let choose = f.button(
                    &format!("{key}-choose"),
                    &tr("Choose…"),
                    false,
                    0.0,
                    0.0,
                    move || choose_path(&id2, &k2, &current),
                );
                let cs = choose.frame().size;
                choose.setFrameOrigin(NSPoint::new(
                    right - cs.width + 4.0,
                    top + (ROW - cs.height) / 2.0,
                ));
                field_right = right - cs.width - 4.0;
            }
            let x = MARGIN + INSET + lw + 12.0;
            let number = setting.kind == SettingKind::Number;
            let field = f.b.native(
                &format!("{key}-field"),
                rect(x, top + 7.0, field_right - x, 22.0),
                || {
                    let t = NSTextField::new(util::mtm());
                    t.setBezelStyle(objc2_app_kit::NSTextFieldBezelStyle::RoundedBezel);
                    t
                },
            );
            if field.currentEditor().is_none() {
                field.setStringValue(&ns(&value.text()));
            }
            let d = FieldDelegate::new();
            *d.ivars().ended.borrow_mut() = Some(Box::new(move |text| {
                let v = if number {
                    match text.trim().parse::<f64>() {
                        Ok(n) => JsonValue::Number(n),
                        Err(_) => return,
                    }
                } else {
                    JsonValue::String(text)
                };
                store::set_plugin_value(&id, &skey, v);
            }));
            d.attach(&field);
            if let Some(help) = &setting.help {
                field.setToolTip(Some(&ns(help)));
            }
        }
    }
}

fn choose_path(plugin: &str, key: &str, current: &str) {
    let mtm = util::mtm();
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(true);
    let expanded = current
        .strip_prefix("~/")
        .map(|rest| util::home().join(rest))
        .unwrap_or_else(|| PathBuf::from(current));
    if let Some(parent) = expanded.parent() {
        panel.setDirectoryURL(Some(&util::file_url(parent)));
    }
    if panel.runModal() == NSModalResponseOK
        && let Some(path) = panel.URL().and_then(|u| u.path())
    {
        store::set_plugin_value(plugin, key, JsonValue::String(path.to_string()));
    }
}

fn install_plugin() {
    let mtm = util::mtm();
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setMessage(Some(&ns(&tr(
        "Choose a plugin folder (it contains plugin.json)",
    ))));
    if panel.runModal() != NSModalResponseOK {
        return;
    }
    let Some(path) = panel.URL().and_then(|u| u.path()) else {
        return;
    };
    match store::backend().install_plugin(&PathBuf::from(path.to_string())) {
        Ok(()) => store::reload_plugins(),
        Err(e) => {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&ns(&tr("That folder isn’t a Lavdeck plugin")));
            alert.setInformativeText(&ns(&e));
            alert.runModal();
        }
    }
}

// MARK: Snapshots

/// Shows Settings on `tab` for a snapshot, behind everything and never key.
pub fn show_for_shots(dark: bool, tab: usize) {
    if STATE.with(|s| s.borrow().is_none()) {
        build();
    }
    refresh();
    let Some((window, tabs)) = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|s| (s.window.clone(), s.tabs.clone()))
    }) else {
        return;
    };
    window.setAppearance(crate::shots::appearance(if dark { "dark" } else { "light" }).as_deref());
    tabs.setSelectedTabViewItemIndex(tab as isize);
    fit_window();
    window.setFrameOrigin(NSPoint::new(40.0, 40.0));
    window.orderBack(None);
}

pub fn draw_for_shots(path: &std::path::Path) {
    if let Some(w) = STATE.with(|s| s.borrow().as_ref().map(|s| s.window.clone())) {
        crate::shots::write_window(&w, path);
    }
}
