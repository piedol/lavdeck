//! The Recordings window, as `RecordingsView.swift`: a sortable table of takes with a filter,
//! search, context menus and multi-selection, next to the selected take (player, files,
//! transcript) or the selection's shared actions ([`crate::detail`]).
//!
//! ⌘-click adds or removes a take, ⇧-click selects a range, ⌘A selects every visible take.
//! Space plays or pauses anywhere in the window except a text field; Return or a double-click
//! plays; Delete asks to move the selection to the Trash; Escape keeps only the first.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSAppearanceCustomization, NSAutoresizingMaskOptions, NSControlSize,
    NSControlTextEditingDelegate, NSEvent, NSEventMask, NSEventModifierFlags, NSImage, NSImageView,
    NSLineBreakMode, NSMenu, NSMenuItem, NSPopUpButton, NSProgressIndicator,
    NSProgressIndicatorStyle, NSScrollView, NSSearchField, NSSearchToolbarItem, NSSplitView,
    NSSplitViewDelegate, NSSplitViewDividerStyle, NSTableCellView, NSTableColumn, NSTableView,
    NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle, NSTextField, NSToolbar,
    NSToolbarDelegate, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask, NSWindowToolbarStyle,
};
use objc2_foundation::{
    NSArray, NSIndexSet, NSInteger, NSNotification, NSObject, NSObjectProtocol, NSSortDescriptor,
    NSString,
};

use crate::canvas::{FieldDelegate, item};
use crate::draw::rect;
use crate::i18n::{tr, trf, trn};
use crate::model::plugins::VIDEO_JOB_ID;
use crate::model::recordings::{self as list, Column, Filter, Row, SortKey};
use crate::model::takes::{Take, TakeMedia, TranscriptDocument};
use crate::theme::Weight;
use crate::util::{self, ns};
use crate::{detail, format, player, store, theme};

/// Everything the window shows that isn't the store's.
pub struct State {
    pub window: Retained<NSWindow>,
    table: Retained<NSTableView>,
    _controller: Retained<Controller>,
    search_field: Retained<NSSearchField>,
    filter_button: Retained<NSPopUpButton>,
    pub detail: std::rc::Rc<detail::Pane>,
    overlay: std::rc::Rc<crate::canvas::Layer>,
    pub takes: Vec<Take>,
    pub transcripts: HashMap<String, TranscriptDocument>,
    pub rows: Vec<Row>,
    /// Take ids. Hidden takes stay selected but are never acted on.
    pub selection: BTreeSet<String>,
    pub search: String,
    pub filter: Filter,
    sort: Vec<SortKey>,
    pub loaded: bool,
    seen: (u64, u64),
    key_monitor: Option<Retained<AnyObject>>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Ignores the table's selection callback while the window sets the selection itself.
    static SYNCING: Cell<bool> = const { Cell::new(false) };
}

pub fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

pub fn is_open() -> bool {
    STATE.with(|s| s.borrow().is_some())
}

/// Selected takes the filter and search show, in table order.
pub fn selected_takes(s: &State) -> Vec<Take> {
    s.rows
        .iter()
        .filter(|r| s.selection.contains(r.id()))
        .map(|r| r.take.clone())
        .collect()
}

// MARK: Classes

define_class!(
    // SAFETY: NSTableView subclass that only adds key handling and a context menu.
    #[unsafe(super = NSTableView)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckRecordingsTable"]
    struct Table;

    impl Table {
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            match event.keyCode() {
                // Delete and forward delete.
                51 | 117 => ask_to_delete(None),
                // Escape: keep only the first of several.
                53 => collapse_selection(),
                // Return and Enter play the take.
                36 | 76 => play_selected(),
                // SAFETY: plain call to the superclass implementation.
                _ => unsafe { msg_send![super(self), keyDown: event] },
            }
        }

        #[unsafe(method(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> *mut NSMenu {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let row = self.rowAtPoint(p);
            match context_menu(row) {
                Some(m) => Retained::autorelease_return(m),
                None => std::ptr::null_mut(),
            }
        }
    }
);

define_class!(
    // SAFETY: NSObject subclass acting as the window's table, split, toolbar and window delegate.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "LavdeckRecordingsController"]
    struct Controller;

    unsafe impl NSObjectProtocol for Controller {}

    unsafe impl NSTableViewDataSource for Controller {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _t: &NSTableView) -> NSInteger {
            with(|s| s.rows.len() as NSInteger).unwrap_or(0)
        }

        #[unsafe(method(tableView:sortDescriptorsDidChange:))]
        fn sort_changed(&self, table: &NSTableView, _old: &NSArray<NSSortDescriptor>) {
            let sort: Vec<SortKey> = table
                .sortDescriptors()
                .iter()
                .filter_map(|d| {
                    let column = Column::from_key(&d.key()?.to_string())?;
                    Some(SortKey { column, ascending: d.ascending() })
                })
                .collect();
            with(|s| s.sort = sort);
            refresh_rows();
        }
    }

    unsafe impl NSControlTextEditingDelegate for Controller {}

    unsafe impl NSTableViewDelegate for Controller {
        #[unsafe(method(tableView:viewForTableColumn:row:))]
        fn view_for(&self, _t: &NSTableView, column: Option<&NSTableColumn>, row: NSInteger) -> *mut NSView {
            let key = column.map(|c| c.identifier().to_string()).unwrap_or_default();
            match cell(&key, row as usize) {
                Some(v) => Retained::autorelease_return(v),
                None => std::ptr::null_mut(),
            }
        }

        #[unsafe(method(tableView:heightOfRow:))]
        fn height_of_row(&self, _t: &NSTableView, row: NSInteger) -> f64 {
            let busy = with(|s| s.rows.get(row as usize).map(|r| r.take.id.clone())).flatten()
                .is_some_and(|id| store::with(|m| m.plugins.job(&id, None).is_some()));
            if busy { 42.0 } else { 24.0 }
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _n: &NSNotification) {
            if !SYNCING.with(|s| s.get()) {
                table_selection_changed();
            }
        }
    }

    unsafe impl NSSplitViewDelegate for Controller {
        #[unsafe(method(splitView:constrainMinCoordinate:ofSubviewAt:))]
        fn min_coordinate(&self, _s: &NSSplitView, _proposed: f64, _i: NSInteger) -> f64 {
            460.0
        }

        #[unsafe(method(splitView:constrainMaxCoordinate:ofSubviewAt:))]
        fn max_coordinate(&self, split: &NSSplitView, _proposed: f64, _i: NSInteger) -> f64 {
            split.bounds().size.width - 460.0
        }

        #[unsafe(method(splitView:resizeSubviewsWithOldSize:))]
        fn resize_subviews(&self, split: &NSSplitView, _old: objc2_foundation::NSSize) {
            // The table keeps its width; the detail takes the rest (down to its minimum).
            let views = split.subviews();
            if views.len() != 2 {
                return;
            }
            let b = split.bounds();
            let d = split.dividerThickness();
            let left = views.objectAtIndex(0);
            let lw = left.frame().size.width.min(b.size.width - 460.0 - d).max(460.0);
            left.setFrame(rect(0.0, 0.0, lw, b.size.height));
            views.objectAtIndex(1).setFrame(rect(lw + d, 0.0, (b.size.width - lw - d).max(0.0), b.size.height));
            // The Transcript column takes what's left of the table's width.
            util::later(0.0, || {
                if let Some(t) = with(|s| s.table.clone()) {
                    t.sizeLastColumnToFit();
                }
            });
        }
    }

    unsafe impl NSWindowDelegate for Controller {
        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, _n: &NSNotification) {
            // Files can change in Finder while the window is in the background.
            reload();
        }

        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _n: &NSNotification) {
            util::later(0.0, closed);
        }
    }

    impl Controller {
        #[unsafe(method(doubleClicked:))]
        fn double_clicked(&self, _sender: Option<&AnyObject>) {
            play_selected();
        }
    }

    unsafe impl NSToolbarDelegate for Controller {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn toolbar_item(&self, _t: &NSToolbar, id: &NSString, _insert: bool) -> Option<Retained<NSToolbarItem>> {
            toolbar_item(&id.to_string())
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_items(&self, _t: &NSToolbar) -> Retained<NSArray<NSString>> {
            toolbar_ids()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_items(&self, _t: &NSToolbar) -> Retained<NSArray<NSString>> {
            toolbar_ids()
        }
    }
);

fn toolbar_ids() -> Retained<NSArray<NSString>> {
    // SAFETY: AppKit's flexible space identifier.
    let flex = unsafe { NSToolbarFlexibleSpaceItemIdentifier };
    let ids = [
        ns("filter"),
        flex.retain(),
        ns("refresh"),
        ns("reveal"),
        ns("search"),
    ];
    NSArray::from_retained_slice(&ids)
}

fn symbol(name: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), None)
}

thread_local! {
    static PENDING_ITEMS: RefCell<HashMap<String, Retained<NSToolbarItem>>> = RefCell::new(HashMap::new());
}

fn toolbar_item(id: &str) -> Option<Retained<NSToolbarItem>> {
    PENDING_ITEMS.with(|p| p.borrow_mut().remove(id))
}

// MARK: Opening

/// Opens Recordings (or brings it forward).
pub fn show() {
    crate::panel::close();
    crate::app::activate();
    if let Some(w) = with(|s| s.window.clone()) {
        w.makeKeyAndOrderFront(None);
        reload();
        return;
    }
    build();
    if let Some(w) = with(|s| s.window.clone()) {
        w.makeKeyAndOrderFront(None);
    }
    reload();
}

fn build() {
    let mtm = util::mtm();
    let controller: Retained<Controller> = {
        let this = Controller::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    };
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::Miniaturizable;
    let window = crate::shots::window(rect(0.0, 0.0, 1040.0, 640.0), style);
    // SAFETY: kept in STATE until it closes.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&ns(&tr("Recordings")));
    window.setContentMinSize(objc2_foundation::NSSize::new(940.0, 580.0));
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    window.center();
    window.setFrameAutosaveName(&ns("recordings"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*controller)));

    // Toolbar items, made now so their controls can be kept.
    let filter_button = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        rect(0.0, 0.0, 80.0, 24.0),
        true,
    );
    filter_button.setToolTip(Some(&ns(&tr("Filter recordings"))));
    let filter_item =
        NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), &ns("filter"));
    filter_item.setView(Some(&filter_button));
    filter_item.setNavigational(true);
    filter_item.setLabel(&ns(&tr("Filter")));
    let button_item = |id: &str, symbol_name: &str, label: String, f: fn()| {
        let i = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), &ns(id));
        i.setImage(symbol(symbol_name).as_deref());
        i.setLabel(&ns(&label));
        i.setToolTip(Some(&ns(&label)));
        i.setBordered(true);
        crate::canvas::on_toolbar_item(&i, f);
        i
    };
    let refresh_item = button_item("refresh", "arrow.clockwise", tr("Refresh"), reload);
    let reveal_item = button_item(
        "reveal",
        "folder",
        tr("Show in Finder"),
        store::reveal_recordings,
    );
    let search_item =
        NSSearchToolbarItem::initWithItemIdentifier(NSSearchToolbarItem::alloc(mtm), &ns("search"));
    let search_field = search_item.searchField();
    search_field.setPlaceholderString(Some(&ns(&tr("Search transcripts, names and dates"))));
    search_item.setPreferredWidthForSearchField(260.0);
    let d = FieldDelegate::new();
    *d.ivars().changed.borrow_mut() = Some(Box::new(|text| {
        with(|s| s.search = text);
        refresh_rows();
    }));
    d.attach(&search_field);
    PENDING_ITEMS.with(|p| {
        let mut p = p.borrow_mut();
        p.insert("filter".into(), filter_item);
        p.insert("refresh".into(), refresh_item);
        p.insert("reveal".into(), reveal_item);
        p.insert("search".into(), Retained::into_super(search_item));
    });
    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &ns("recordings"));
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    toolbar.setAllowsUserCustomization(false);
    toolbar.setDisplayMode(objc2_app_kit::NSToolbarDisplayMode::IconOnly);
    window.setToolbar(Some(&toolbar));

    // The split: table on the left, the take (or selection) on the right.
    let content = window.contentView().expect("content view");
    let split = NSSplitView::initWithFrame(NSSplitView::alloc(mtm), content.bounds());
    split.setVertical(true);
    split.setDividerStyle(NSSplitViewDividerStyle::Thin);
    split.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    split.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    content.addSubview(&split);

    let table: Retained<NSTableView> = {
        let this = Table::alloc(mtm).set_ivars(());
        let t: Retained<Table> =
            unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, 540.0, 600.0)] };
        Retained::into_super(t)
    };
    table.setUsesAlternatingRowBackgroundColors(true);
    table.setAllowsMultipleSelection(true);
    table.setAllowsEmptySelection(true);
    table.setColumnAutoresizingStyle(
        objc2_app_kit::NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
    );
    for (column, title, min, ideal) in [
        (Column::Recorded, tr("Recorded"), 120.0, 136.0),
        (Column::Length, tr("Length"), 48.0, 56.0),
        (Column::Video, tr("Video"), 40.0, 44.0),
        (Column::Mics, tr("Mics"), 50.0, 80.0),
        (Column::Transcript, tr("Transcript"), 60.0, 100.0),
    ] {
        let c = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns(column.key()));
        c.setTitle(&ns(&title));
        c.setMinWidth(min);
        c.setWidth(ideal);
        let ascending = column != Column::Recorded;
        // SAFETY: a key the controller sorts by.
        let proto =
            NSSortDescriptor::sortDescriptorWithKey_ascending(Some(&ns(column.key())), ascending);
        c.setSortDescriptorPrototype(Some(&proto));
        table.addTableColumn(&c);
    }
    // SAFETY: the controller lives as long as the window's state.
    unsafe {
        table.setDataSource(Some(ProtocolObject::from_ref(&*controller)));
        table.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
        table.setTarget(Some(&controller));
        table.setDoubleAction(Some(sel!(doubleClicked:)));
    }
    let initial =
        NSSortDescriptor::sortDescriptorWithKey_ascending(Some(&ns(Column::Recorded.key())), false);
    table.setSortDescriptors(&NSArray::from_retained_slice(&[initial]));
    let scroll =
        NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0.0, 0.0, 540.0, 600.0));
    scroll.setDocumentView(Some(&table));
    table.setStyle(NSTableViewStyle::Inset);
    table.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    table.sizeLastColumnToFit();
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    // Over the table: what to do when nothing matches the filter or search.
    let left = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 500.0, 600.0));
    scroll.setFrame(left.bounds());
    scroll.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    left.addSubview(&scroll);
    let overlay = crate::canvas::Canvas::new(mtm, left.bounds());
    overlay.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    left.addSubview(&overlay);
    overlay.on_resize(|| util::later(0.0, update_overlay));
    split.addSubview(&left);

    let pane = detail::Pane::new(mtm, rect(0.0, 0.0, 500.0, 600.0));
    split.addSubview(&pane.root);
    split.adjustSubviews();
    if let Some(left) = split.subviews().firstObject() {
        // The table starts at its ideal width.
        let h = split.bounds().size.height;
        left.setFrame(rect(0.0, 0.0, 500.0, h));
        split.adjustSubviews();
    }

    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            window,
            table,
            _controller: controller,
            search_field,
            filter_button,
            detail: std::rc::Rc::new(pane),
            overlay: std::rc::Rc::new(crate::canvas::Layer::new(Retained::into_super(overlay))),
            takes: Vec::new(),
            transcripts: HashMap::new(),
            rows: Vec::new(),
            selection: BTreeSet::new(),
            search: String::new(),
            filter: Filter::All,
            sort: vec![list::DEFAULT_SORT],
            loaded: false,
            seen: (u64::MAX, u64::MAX),
            key_monitor: None,
        })
    });
    install_space_key();
    update_filter_menu();
}

fn closed() {
    let Some(s) = STATE.with(|s| s.borrow_mut().take()) else {
        return;
    };
    if let Some(m) = s.key_monitor {
        // SAFETY: a monitor this window installed.
        unsafe { NSEvent::removeMonitor(&m) };
    }
    player::stop();
    s.window.setDelegate(None);
}

/// Space plays or pauses the selected take anywhere in this window, except while typing.
fn install_space_key() {
    let handler = RcBlock::new(|e: NonNull<NSEvent>| -> *mut NSEvent {
        let ev = unsafe { e.as_ref() };
        let mods = ev.modifierFlags()
            & (NSEventModifierFlags::Command
                | NSEventModifierFlags::Option
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Shift);
        if ev.keyCode() != 49 || !mods.is_empty() {
            return e.as_ptr();
        }
        let Some(window) = with(|s| s.window.clone()) else {
            return e.as_ptr();
        };
        let ours = ev
            .window(util::mtm())
            .is_some_and(|w| w.windowNumber() == window.windowNumber());
        let typing = window
            .firstResponder()
            .is_some_and(|r| r.isKindOfClass(objc2_app_kit::NSText::class()));
        if !ours || typing {
            return e.as_ptr();
        }
        if player::with(|p| p.media.is_some()) {
            player::toggle();
        } else {
            play_selected();
        }
        std::ptr::null_mut()
    });
    let m = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
    };
    with(|s| s.key_monitor = m);
}

use objc2::ClassType;

// MARK: Data

/// Rescans the takes and their transcripts (only changed transcripts are re-read).
pub fn reload() {
    if !is_open() {
        return;
    }
    let (root, names) = store::with(|s| {
        (
            s.settings.recordings_root.clone(),
            s.settings.mic_names.clone(),
        )
    });
    let backend = store::backend();
    let takes = backend.scan_takes(&root, &names);
    let mut docs = HashMap::new();
    for t in takes.iter().filter(|t| t.has_transcript()) {
        if let Some(d) = cached_transcript(t, &*backend) {
            docs.insert(t.id.clone(), d);
        }
    }
    let first_load = with(|s| {
        s.takes = takes;
        s.transcripts = docs;
        !s.loaded
    })
    .unwrap_or(false);
    refresh_rows();
    with(|s| {
        let ids: BTreeSet<String> = s.takes.iter().map(|t| t.id.clone()).collect();
        s.selection.retain(|id| ids.contains(id));
        // Start on the newest take; later reloads leave an empty selection alone.
        if first_load
            && s.selection.is_empty()
            && let Some(first) = s.rows.first()
        {
            s.selection.insert(first.id().to_string());
        }
        s.loaded = true;
    });
    focus_requested_take();
    apply_selection();
}

thread_local! {
    /// Parsed transcripts by path and modification time.
    static TRANSCRIPTS: RefCell<HashMap<PathBuf, (std::time::SystemTime, TranscriptDocument)>> = RefCell::new(HashMap::new());
}

fn cached_transcript(
    take: &Take,
    backend: &dyn crate::backend::Backend,
) -> Option<TranscriptDocument> {
    let path = take.transcript_path();
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    if let Some(doc) = TRANSCRIPTS.with(|c| {
        c.borrow()
            .get(&path)
            .filter(|(m, _)| *m == modified)
            .map(|(_, d)| d.clone())
    }) {
        return Some(doc);
    }
    let doc = backend.load_transcript(take)?;
    TRANSCRIPTS.with(|c| c.borrow_mut().insert(path, (modified, doc.clone())));
    Some(doc)
}

/// Rebuilds the filtered, sorted rows (on takes, transcripts, search, filter or sort changes).
fn refresh_rows() {
    // AppKit calls back into the window's state while it reloads, so no borrow is held then.
    let table = with(|s| {
        s.rows = list::rows(
            &s.takes,
            &s.transcripts,
            s.filter,
            &s.search,
            &s.sort,
            format::date_text,
        );
        s.table.clone()
    });
    if let Some(table) = table {
        table.reloadData();
        apply_selection();
    }
    update_overlay();
}

/// "Nothing matches", or no search results, over an empty table.
fn update_overlay() {
    let Some((layer, empty, search, filter)) = with(|s| {
        (
            s.overlay.clone(),
            s.loaded && !s.takes.is_empty() && s.rows.is_empty(),
            s.search.trim().to_string(),
            s.filter,
        )
    }) else {
        return;
    };
    let size = layer.root.frame().size;
    let mut b = layer.build();
    if empty && search.is_empty() {
        let title = trf(
            "Nothing matches “{filter}”",
            &[("filter", &filter_title(filter))],
        );
        detail::unavailable(
            &mut b,
            "line.3.horizontal.decrease.circle",
            &title,
            None,
            size.width,
            size.height - 40.0,
        );
        let show = b.native("show-all", rect(0.0, 0.0, 10.0, 10.0), || {
            objc2_app_kit::NSButton::new(util::mtm())
        });
        show.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
        show.setTitle(&ns(&tr("Show All")));
        show.sizeToFit();
        let w = show.frame().size.width;
        show.setFrameOrigin(objc2_foundation::NSPoint::new(
            (size.width - w) / 2.0,
            size.height / 2.0 + 30.0,
        ));
        crate::canvas::on_action(&show, || set_filter(Filter::All));
    } else if empty {
        let title = trf("No Results for “{query}”", &[("query", &search)]);
        detail::unavailable(
            &mut b,
            "magnifyingglass",
            &title,
            Some(&tr("Check the spelling or try a new search.")),
            size.width,
            size.height,
        );
    }
    b.finish();
}

/// Puts the selection set into the table, and redraws what depends on it.
fn apply_selection() {
    let picked = with(|s| {
        let indexes = objc2_foundation::NSMutableIndexSet::new();
        for (i, r) in s.rows.iter().enumerate() {
            if s.selection.contains(r.id()) {
                indexes.addIndex(i);
            }
        }
        (s.table.clone(), indexes)
    });
    if let Some((table, indexes)) = picked {
        SYNCING.with(|y| y.set(true));
        table.selectRowIndexes_byExtendingSelection(&indexes, false);
        SYNCING.with(|y| y.set(false));
    }
    update_title();
    detail::render();
}

fn table_selection_changed() {
    with(|s| {
        let visible: BTreeSet<String> = s.rows.iter().map(|r| r.id().to_string()).collect();
        let selected = s.table.selectedRowIndexes();
        let mut next: BTreeSet<String> = s
            .selection
            .iter()
            .filter(|id| !visible.contains(*id))
            .cloned()
            .collect();
        for (i, r) in s.rows.iter().enumerate() {
            if selected.containsIndex(i) {
                next.insert(r.id().to_string());
            }
        }
        s.selection = next;
    });
    update_title();
    detail::render();
}

fn update_title() {
    with(|s| {
        let selected = selected_takes(s).len();
        let shown = format::count(s.rows.len() as u64);
        let total = format::count(s.takes.len() as u64);
        let subtitle = if selected > 1 {
            trf(
                "{selected} selected · {shown} of {total}",
                &[
                    ("selected", &format::count(selected as u64)),
                    ("shown", &shown),
                    ("total", &total),
                ],
            )
        } else {
            trf(
                "{shown} of {total}",
                &[("shown", &shown), ("total", &total)],
            )
        };
        s.window.setSubtitle(&ns(&subtitle));
    });
}

fn update_filter_menu() {
    with(|s| {
        let menu = NSMenu::new(util::mtm());
        // A pull-down's first item is its title.
        let head = item(&filter_title(s.filter), || {});
        head.setImage(
            symbol(if s.filter == Filter::All {
                "line.3.horizontal.decrease.circle"
            } else {
                "line.3.horizontal.decrease.circle.fill"
            })
            .as_deref(),
        );
        menu.addItem(&head);
        for f in Filter::ALL {
            let entry = item(&filter_title(f), move || set_filter(f));
            entry.setState(if f == s.filter {
                objc2_app_kit::NSControlStateValueOn
            } else {
                objc2_app_kit::NSControlStateValueOff
            });
            menu.addItem(&entry);
        }
        s.filter_button.setMenu(Some(&menu));
        s.filter_button.sizeToFit();
    });
}

pub fn filter_title(f: Filter) -> String {
    match f {
        Filter::All => tr("All"),
        Filter::Video => tr("With video"),
        Filter::Transcribed => tr("Transcribed"),
        Filter::Untranscribed => tr("Not transcribed"),
        Filter::TwoSpeaker => tr("Two speakers"),
    }
}

pub fn set_filter(f: Filter) {
    with(|s| s.filter = f);
    update_filter_menu();
    refresh_rows();
}

/// Called when the model changes: reload when takes or job outputs changed, then redraw.
pub fn refresh() {
    let Some(seen) = with(|s| s.seen) else { return };
    let now = store::with(|m| (m.library_revision, m.plugins.outputs_revision));
    if now != seen {
        with(|s| s.seen = now);
        reload();
        return;
    }
    if store::with(|m| m.focus_take_id.is_some()) {
        focus_requested_take();
        apply_selection();
        return;
    }
    // Jobs move: row heights and the progress in the Recorded column.
    if let Some((table, n)) = with(|s| (s.table.clone(), s.rows.len())) {
        let all = NSIndexSet::indexSetWithIndexesInRange(objc2_foundation::NSRange::new(0, n));
        table.noteHeightOfRowsWithIndexesChanged(&all);
        let cols = NSIndexSet::indexSetWithIndex(0);
        table.reloadDataForRowIndexes_columnIndexes(&all, &cols);
    }
    detail::render();
}

/// Selects the take a "Show" asked for, clearing the filter and search.
fn focus_requested_take() {
    let Some(id) = store::with(|m| m.focus_take_id.clone()) else {
        return;
    };
    let found = with(|s| s.takes.iter().any(|t| t.id == id)).unwrap_or(false);
    if !found {
        return;
    }
    let views = with(|s| {
        s.filter = Filter::All;
        s.search.clear();
        s.rows = list::rows(
            &s.takes,
            &s.transcripts,
            s.filter,
            &s.search,
            &s.sort,
            format::date_text,
        );
        s.selection = [id].into();
        (s.table.clone(), s.search_field.clone())
    });
    if let Some((table, field)) = views {
        field.setStringValue(&ns(""));
        table.reloadData();
    }
    update_filter_menu();
    store::update(|m| m.focus_take_id = None);
}

/// Called by the player when what it plays, or whether it plays, changes.
pub fn player_changed() {
    detail::render();
}

/// Called by the player ten times a second.
pub fn clock_tick() {
    detail::tick();
}

// MARK: Table cells

fn label(text: &str, secondary: bool, mono_digits: bool) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(text), util::mtm());
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    if mono_digits {
        l.setFont(Some(&theme::digits(13.0, Weight::Regular)));
    } else {
        l.setFont(Some(&theme::font(13.0, Weight::Regular)));
    }
    if secondary {
        l.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
    }
    l
}

fn cell(column: &str, row: usize) -> Option<Retained<NSView>> {
    let mtm = util::mtm();
    let r = with(|s| s.rows.get(row).cloned()).flatten()?;
    let job = store::with(|m| m.plugins.job(r.id(), None).cloned());
    let view =
        NSTableCellView::initWithFrame(NSTableCellView::alloc(mtm), rect(0.0, 0.0, 100.0, 24.0));
    let add_label = |l: &NSTextField, y: f64| {
        l.setFrame(rect(2.0, y, view.frame().size.width - 4.0, 17.0));
        l.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        view.addSubview(l);
    };
    let top = |height: f64| {
        if job.is_some() {
            42.0 - 3.0 - 17.0
        } else {
            (24.0 - height) / 2.0
        }
    };
    match Column::from_key(column)? {
        Column::Recorded => {
            let l = label(
                &r.take
                    .started
                    .map(format::day_time)
                    .unwrap_or_else(|| r.take.id.clone()),
                false,
                false,
            );
            add_label(&l, top(17.0));
            // SAFETY: the label is a subview of the cell.
            unsafe { view.setTextField(Some(&l)) };
            if let Some(job) = &job {
                let caption = if job.state == crate::model::plugins::JobState::Queued {
                    trf("{title}: queued", &[("title", &job.title)])
                } else {
                    job.message.clone().unwrap_or_else(|| job.title.clone())
                };
                let c = NSTextField::labelWithString(&ns(&caption), mtm);
                c.setFont(Some(&theme::font(10.0, Weight::Regular)));
                c.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
                c.setFrame(rect(2.0, 2.0, view.frame().size.width - 4.0, 13.0));
                c.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                view.addSubview(&c);
                let p = NSProgressIndicator::initWithFrame(
                    NSProgressIndicator::alloc(mtm),
                    rect(2.0, 14.0, view.frame().size.width - 4.0, 6.0),
                );
                p.setStyle(NSProgressIndicatorStyle::Bar);
                p.setControlSize(NSControlSize::Mini);
                p.setIndeterminate(false);
                p.setMaxValue(1.0);
                p.setDoubleValue(if job.state == crate::model::plugins::JobState::Queued {
                    0.0
                } else {
                    job.fraction
                });
                p.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                view.addSubview(&p);
            }
        }
        Column::Length => {
            let l = label(&format::length(r.duration()), false, true);
            add_label(&l, top(17.0));
            // SAFETY: the label is a subview of the cell.
            unsafe { view.setTextField(Some(&l)) };
        }
        Column::Video => {
            let (symbol_name, tip, secondary) = media_badge(&r.take);
            if let Some(name) = symbol_name {
                let iv = NSImageView::initWithFrame(
                    NSImageView::alloc(mtm),
                    rect(2.0, top(17.0), 20.0, 17.0),
                );
                iv.setImage(symbol(name).as_deref());
                let tint = if secondary {
                    objc2_app_kit::NSColor::secondaryLabelColor()
                } else {
                    objc2_app_kit::NSColor::labelColor()
                };
                iv.setContentTintColor(Some(&tint));
                iv.setToolTip(Some(&ns(&tip)));
                view.addSubview(&iv);
                // SAFETY: the image view is a subview of the cell.
                unsafe { view.setImageView(Some(&iv)) };
            }
            view.setToolTip(Some(&ns(&tip)));
        }
        Column::Mics => {
            let names = r.speakers();
            let l = label(if names.is_empty() { "—" } else { &names }, false, false);
            add_label(&l, top(17.0));
            // SAFETY: the label is a subview of the cell.
            unsafe { view.setTextField(Some(&l)) };
        }
        Column::Transcript => {
            let l = label(
                if r.snippet.is_empty() {
                    "—"
                } else {
                    &r.snippet
                },
                true,
                false,
            );
            l.setToolTip(Some(&ns(&r.snippet)));
            add_label(&l, top(17.0));
            // SAFETY: the label is a subview of the cell.
            unsafe { view.setTextField(Some(&l)) };
        }
    }
    Some(Retained::into_super(view))
}

/// One glyph saying what video a take has, its tooltip, and whether it's quiet.
pub fn media_badge(take: &Take) -> (Option<&'static str>, String, bool) {
    let screen = take.has_capture("screen");
    let camera = take.has_capture("camera");
    if take.finished_video().is_some() {
        (Some("play.rectangle.fill"), tr("Video ready"), false)
    } else if screen || camera {
        let tip = if screen && camera {
            tr("Screen and camera recorded; video not made")
        } else if screen {
            tr("Screen recorded")
        } else {
            tr("Camera recorded")
        };
        (Some(if screen { "display" } else { "video" }), tip, true)
    } else {
        (None, tr("Audio only"), true)
    }
}

// MARK: Actions

/// Double-click or Return plays a single take.
pub fn play_selected() {
    let Some(takes) = with(|s| selected_takes(s)) else {
        return;
    };
    if let [take] = takes.as_slice()
        && let Some(best) = TakeMedia::all(take).first()
    {
        player::load(best, take, true);
    }
}

fn collapse_selection() {
    let first = with(|s| selected_takes(s)).unwrap_or_default();
    if first.len() > 1 {
        with(|s| s.selection = [first[0].id.clone()].into());
        apply_selection();
    }
}

pub fn select_only(id: &str) {
    with(|s| s.selection = [id.to_string()].into());
    apply_selection();
}

/// Asks before moving takes' files to the Trash (the selection when `takes` is None).
pub fn ask_to_delete(takes: Option<Vec<Take>>) {
    let Some(doomed) = takes.or_else(|| with(|s| selected_takes(s))) else {
        return;
    };
    if doomed.is_empty() {
        return;
    }
    let Some(window) = with(|s| s.window.clone()) else {
        return;
    };
    let mtm = util::mtm();
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    let n = doomed.len();
    alert.setMessageText(&ns(&if n > 1 {
        trf(
            "Move {n} recordings to the Trash?",
            &[("n", &format::count(n as u64))],
        )
    } else {
        tr("Move this recording to the Trash?")
    }));
    let busy = store::with(|m| doomed.iter().any(|t| m.plugins.job(&t.id, None).is_some()));
    let mut message = if n > 1 {
        trf(
            "All files of these {n} takes, including transcripts and videos, go to the Trash.",
            &[("n", &format::count(n as u64))],
        )
    } else {
        trf(
            "All files of the {stem} take, including transcripts and videos, go to the Trash.",
            &[("stem", &doomed[0].stem)],
        )
    };
    if busy {
        message.push(' ');
        message.push_str(&tr("Work still running on them is cancelled."));
    }
    message.push(' ');
    message.push_str(&tr("You can restore them from there."));
    alert.setInformativeText(&ns(&message));
    let files: usize = doomed.iter().map(|t| t.files.len()).sum();
    let confirm = alert.addButtonWithTitle(&ns(&trn(
        files as u64,
        "Move {n} file to Trash",
        "Move {n} files to Trash",
    )));
    confirm.setHasDestructiveAction(true);
    alert.addButtonWithTitle(&ns(&tr("Cancel")));
    let handler = RcBlock::new(move |response: objc2_app_kit::NSModalResponse| {
        if response == objc2_app_kit::NSAlertFirstButtonReturn {
            delete(&doomed);
        }
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
}

fn delete(doomed: &[Take]) {
    let ids: Vec<String> = doomed.iter().map(|t| t.id.clone()).collect();
    // A job still writing into the take's folder would leave an orphan file behind.
    store::cancel_jobs_for(&ids);
    player::stop();
    // Afterwards select the row that took the first removed take's place.
    let visible: Vec<String> =
        with(|s| s.rows.iter().map(|r| r.id().to_string()).collect()).unwrap_or_default();
    let next = list::next_after_delete(&visible, &ids);
    let backend = store::backend();
    for take in doomed {
        if let Err(e) = backend.trash(take) {
            let alert = NSAlert::new(util::mtm());
            alert.setMessageText(&ns(&trf(
                "Couldn’t move {stem} to the Trash",
                &[("stem", &take.stem)],
            )));
            alert.setInformativeText(&ns(&e));
            alert.runModal();
            break;
        }
    }
    with(|s| s.selection = next.into_iter().collect());
    store::note_library_changed();
}

/// Puts the takes' transcripts on the clipboard, oldest first, each under its date.
pub fn copy_transcripts(takes: &[Take]) {
    let text = with(|s| {
        let refs: Vec<&Take> = takes.iter().collect();
        list::copied_transcripts(&refs, &s.transcripts, format::date_text)
    })
    .flatten();
    if let Some(text) = text {
        util::copy_text(&text);
    }
}

pub fn reveal(takes: &[Take]) {
    let paths: Vec<PathBuf> = takes.iter().filter_map(Take::share_path).collect();
    util::reveal(&paths);
}

/// Right-clicking inside the selection acts on all of it; elsewhere, on that row.
fn context_menu(row: isize) -> Option<Retained<NSMenu>> {
    let chosen: Vec<Take> = with(|s| {
        let clicked = (row >= 0).then(|| s.rows.get(row as usize)).flatten()?;
        Some(if s.selection.contains(clicked.id()) {
            selected_takes(s)
        } else {
            vec![clicked.take.clone()]
        })
    })
    .flatten()?;
    if chosen.is_empty() {
        return None;
    }
    let mtm = util::mtm();
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    if let [take] = chosen.as_slice()
        && let Some(path) = take.share_path()
    {
        let title = if path.extension().is_some_and(|e| e == "mp4") {
            tr("Open Video")
        } else {
            tr("Open Audio")
        };
        menu.addItem(&item(&title, move || util::open(&path)));
    }
    let c = chosen.clone();
    menu.addItem(&item(&tr("Show in Finder"), move || reveal(&c)));
    if chosen.iter().any(Take::has_transcript) {
        let c = chosen.clone();
        let title = if chosen.len() > 1 {
            tr("Copy Transcripts")
        } else {
            tr("Copy Transcript")
        };
        menu.addItem(&item(&title, move || copy_transcripts(&c)));
    }
    let several = chosen.len() > 1;
    let actions: Vec<(String, String, String, Vec<Take>)> = store::with(|m| {
        m.plugins
            .actions()
            .into_iter()
            .filter_map(|(p, a)| {
                let fit: Vec<Take> = chosen.iter().filter(|t| a.applies(t)).cloned().collect();
                (!fit.is_empty()).then(|| (p.id().to_string(), a.id.clone(), a.title.clone(), fit))
            })
            .collect()
    });
    let videos: Vec<Take> = chosen.iter().filter(|t| t.has("video")).cloned().collect();
    if !actions.is_empty() || !videos.is_empty() {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    if !videos.is_empty() {
        let busy = store::with(|m| {
            videos
                .iter()
                .all(|t| m.plugins.job(&t.id, Some(VIDEO_JOB_ID)).is_some())
        });
        let title = if busy {
            tr("Remake Video (in progress)")
        } else if several {
            trf("Remake Video ({n})", &[("n", &videos.len().to_string())])
        } else {
            tr("Remake Video")
        };
        let v = videos.clone();
        let entry = item(&title, move || store::make_video(&v, false));
        entry.setEnabled(!busy);
        menu.addItem(&entry);
    }
    for (plugin, action, title, takes) in actions {
        let busy = store::with(|m| {
            takes
                .iter()
                .all(|t| m.plugins.job(&t.id, Some(&plugin)).is_some())
        });
        let title = if busy {
            trf("{action} (in progress)", &[("action", &title)])
        } else if several {
            trf(
                "{action} ({n})",
                &[("action", &title), ("n", &takes.len().to_string())],
            )
        } else {
            title
        };
        let entry = item(&title, move || store::run_action(&plugin, &action, &takes));
        entry.setEnabled(!busy);
        menu.addItem(&entry);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let c = chosen.clone();
    menu.addItem(&item(&tr("Move to Trash…"), move || {
        ask_to_delete(Some(c.clone()))
    }));
    Some(menu)
}

// MARK: Snapshots

/// Opens the window for a snapshot: behind everything, never key, in light or dark, with the
/// first take selected (or the first `select` takes).
pub fn show_for_shots(dark: bool, select: Option<usize>) {
    if !is_open() {
        build();
        reload();
    }
    let Some(window) = with(|s| {
        let ids: Vec<String> = s
            .rows
            .iter()
            .take(select.unwrap_or(1))
            .map(|r| r.id().to_string())
            .collect();
        s.selection = ids.into_iter().collect();
        s.window.clone()
    }) else {
        return;
    };
    window.setAppearance(crate::shots::appearance(if dark { "dark" } else { "light" }).as_deref());
    window.setFrame_display(rect(40.0, 40.0, 1040.0, 720.0), false);
    window.orderBack(None);
    apply_selection();
}

pub fn draw_for_shots(path: &Path) {
    // A focused table in the key window draws its selection in the accent colour.
    if let Some(table) = with(|s| s.table.clone()) {
        let rows = table.numberOfRows();
        for i in 0..rows {
            if let Some(row) = table.rowViewAtRow_makeIfNecessary(i, false) {
                row.setEmphasized(true);
            }
        }
    }
    if let Some(w) = with(|s| s.window.clone()) {
        crate::shots::write_window(&w, path);
    }
}

// MARK: Scripted checks

pub fn search_for_drive(text: &str) {
    if let Some(field) = with(|s| {
        s.search = text.to_string();
        s.search_field.clone()
    }) {
        field.setStringValue(&ns(text));
    }
    refresh_rows();
}

/// A view of the table's overlay by key.
pub fn overlay_view_for_drive(key: &str) -> Option<Retained<NSView>> {
    with(|s| s.overlay.get(key)).flatten()
}

pub fn sort_for_drive(descriptors: &NSArray<NSSortDescriptor>) {
    if let Some(table) = with(|s| s.table.clone()) {
        table.setSortDescriptors(descriptors);
    }
}

pub fn select_rows_for_drive(rows: &[usize]) {
    if let Some(table) = with(|s| s.table.clone()) {
        let set = objc2_foundation::NSMutableIndexSet::new();
        for r in rows {
            set.addIndex(*r);
        }
        table.selectRowIndexes_byExtendingSelection(&set, false);
    }
}
