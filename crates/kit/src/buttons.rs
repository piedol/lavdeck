//! Media/telephony button events the RX reports on its HID interface (report 0x06, consumer page).
//!
//! With `seize` the device is opened exclusively so macOS does not also act on the keys (a
//! transmitter's link button arrives as volume-up and would otherwise change the system volume).
//! Lavdeck only ever reads input reports here; nothing is sent to the HID device.

use std::cell::{Cell, RefCell, UnsafeCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::SystemTime;

use objc2_core_foundation::{
    CFDictionary, CFNumber, CFRetained, CFRunLoop, CFString, kCFRunLoopDefaultMode,
};
use objc2_io_kit::{
    IOHIDDevice, IOHIDManager, IOHIDReportType, IOReturn, kIOHIDOptionsTypeNone,
    kIOHIDOptionsTypeSeizeDevice, kIOHIDProductIDKey, kIOHIDVendorIDKey,
};

use crate::usb::{PRODUCT_ID, VENDOR_ID};

/// Bit order follows the RX's report descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MicButton {
    VolumeUp = 0,
    VolumeDown,
    Mute,
    PlayPause,
    NextTrack,
    PreviousTrack,
    Pause,
    Stop,
}

impl MicButton {
    pub const ALL: [MicButton; 8] = [
        Self::VolumeUp,
        Self::VolumeDown,
        Self::Mute,
        Self::PlayPause,
        Self::NextTrack,
        Self::PreviousTrack,
        Self::Pause,
        Self::Stop,
    ];

    /// The Swift case name, as the CLI prints it.
    pub fn name(self) -> &'static str {
        match self {
            Self::VolumeUp => "volumeUp",
            Self::VolumeDown => "volumeDown",
            Self::Mute => "mute",
            Self::PlayPause => "playPause",
            Self::NextTrack => "nextTrack",
            Self::PreviousTrack => "previousTrack",
            Self::Pause => "pause",
            Self::Stop => "stop",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MicButtonEvent {
    pub button: MicButton,
    pub is_down: bool,
    pub timestamp: SystemTime,
}

type EventCallback = Box<dyn Fn(MicButtonEvent)>;
type RawReportCallback = Box<dyn Fn(u8, &[u8])>;

/// What the HID callbacks reach through their context pointer; boxed so its address is stable.
struct Handler {
    last_bits: Cell<u8>,
    on_event: RefCell<Option<EventCallback>>,
    on_raw_report: RefCell<Option<RawReportCallback>>,
    /// IOKit writes input reports here; only IOKit touches it.
    report: UnsafeCell<[u8; 128]>,
}

impl Default for Handler {
    fn default() -> Self {
        Self {
            last_bits: Cell::new(0),
            on_event: RefCell::default(),
            on_raw_report: RefCell::default(),
            report: UnsafeCell::new([0; 128]),
        }
    }
}

impl Handler {
    fn handle(&self, report_id: u8, bytes: &[u8]) {
        if let Some(raw) = self.on_raw_report.borrow().as_ref() {
            raw(report_id, bytes);
        }
        if report_id != 0x06 {
            return;
        }
        // macOS may include the report ID as the first byte.
        let data = if bytes.first() == Some(&0x06) && bytes.len() >= 2 {
            &bytes[1..]
        } else {
            bytes
        };
        let Some(&bits) = data.first() else { return };
        let changed = bits ^ self.last_bits.get();
        self.last_bits.set(bits);
        let on_event = self.on_event.borrow();
        for button in MicButton::ALL {
            let bit = button as u8;
            if changed >> bit & 1 == 1
                && let Some(callback) = on_event.as_ref()
            {
                callback(MicButtonEvent {
                    button,
                    is_down: bits >> bit & 1 == 1,
                    timestamp: SystemTime::now(),
                });
            }
        }
    }
}

/// Listens to the RX's HID consumer-control reports. Not `Send`: it lives on the thread whose run
/// loop it was started on, and its callbacks run there.
pub struct ButtonMonitor {
    seize: bool,
    handler: Box<Handler>,
    running: Option<(CFRetained<IOHIDManager>, CFRetained<CFRunLoop>)>,
}

impl ButtonMonitor {
    pub fn new(seize: bool) -> Self {
        Self {
            seize,
            handler: Box::default(),
            running: None,
        }
    }

    pub fn set_on_event(&mut self, callback: impl Fn(MicButtonEvent) + 'static) {
        *self.handler.on_event.borrow_mut() = Some(Box::new(callback));
    }

    /// Raw input reports (report ID, bytes) for diagnostics.
    pub fn set_on_raw_report(&mut self, callback: impl Fn(u8, &[u8]) + 'static) {
        *self.handler.on_raw_report.borrow_mut() = Some(Box::new(callback));
    }

    /// Opens the receiver's HID device on the current thread's run loop (which must be running, as
    /// the main thread's is). Returns IOHIDManagerOpen's result.
    pub fn start(&mut self) -> IOReturn {
        self.stop();
        let manager = IOHIDManager::new(None, kIOHIDOptionsTypeNone);
        let Some(run_loop) = CFRunLoop::current() else {
            return -1;
        };
        let matching = CFDictionary::<CFString, CFNumber>::from_slices(
            &[
                &cf_string(kIOHIDVendorIDKey),
                &cf_string(kIOHIDProductIDKey),
            ],
            &[
                &CFNumber::new_i32(VENDOR_ID),
                &CFNumber::new_i32(PRODUCT_ID),
            ],
        );
        let context = (&*self.handler as *const Handler)
            .cast_mut()
            .cast::<c_void>();
        // SAFETY: the handler is boxed and outlives the manager (stop() runs before it is dropped);
        // the matching dictionary holds CFString keys and CFNumber values as IOKit expects.
        unsafe {
            manager.set_device_matching(Some(matching.as_opaque()));
            manager.register_device_matching_callback(Some(device_matched), context);
            if let Some(mode) = kCFRunLoopDefaultMode {
                manager.schedule_with_run_loop(&run_loop, mode);
            }
        }
        let options = if self.seize {
            kIOHIDOptionsTypeSeizeDevice
        } else {
            kIOHIDOptionsTypeNone
        };
        let result = IOHIDManager::open(&manager, options);
        self.running = Some((manager, run_loop));
        result
    }

    pub fn stop(&mut self) {
        let Some((manager, run_loop)) = self.running.take() else {
            return;
        };
        // SAFETY: unscheduling from the run loop it was scheduled on.
        unsafe {
            if let Some(mode) = kCFRunLoopDefaultMode {
                manager.unschedule_from_run_loop(&run_loop, mode);
            }
        }
        IOHIDManager::close(&manager, kIOHIDOptionsTypeNone);
    }

    #[cfg(test)]
    pub(crate) fn handle(&self, report_id: u8, bytes: &[u8]) {
        self.handler.handle(report_id, bytes);
    }
}

impl Drop for ButtonMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn cf_string(key: &std::ffi::CStr) -> CFRetained<CFString> {
    CFString::from_str(key.to_str().unwrap_or_default())
}

unsafe extern "C-unwind" fn device_matched(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    device: NonNull<IOHIDDevice>,
) {
    let Some(handler) = (unsafe { context.cast::<Handler>().as_ref() }) else {
        return;
    };
    let Some(buffer) = NonNull::new(handler.report.get().cast::<u8>()) else {
        return;
    };
    // SAFETY: the report buffer lives in the boxed handler, which outlives the device callbacks.
    unsafe {
        device
            .as_ref()
            .register_input_report_callback(buffer, 128, Some(input_report), context);
    }
}

unsafe extern "C-unwind" fn input_report(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    _kind: IOHIDReportType,
    report_id: u32,
    report: NonNull<u8>,
    length: isize,
) {
    let Some(handler) = (unsafe { context.cast::<Handler>().as_ref() }) else {
        return;
    };
    // SAFETY: IOKit passes `length` valid bytes at `report`.
    let bytes =
        unsafe { std::slice::from_raw_parts(report.as_ptr(), length.max(0) as usize) }.to_vec();
    handler.handle(report_id as u8, &bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn button_monitor_decodes_consumer_bits() {
        let mut monitor = ButtonMonitor::new(false);
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        monitor.set_on_event(move |event| sink.borrow_mut().push(event));
        monitor.handle(0x06, &[0x06, 0x01, 0x00]);
        monitor.handle(0x06, &[0x06, 0x00, 0x00]);
        let events = events.borrow();
        assert_eq!(
            events.iter().map(|e| e.button).collect::<Vec<_>>(),
            vec![MicButton::VolumeUp, MicButton::VolumeUp]
        );
        assert_eq!(
            events.iter().map(|e| e.is_down).collect::<Vec<_>>(),
            vec![true, false]
        );
    }

    #[test]
    fn other_reports_only_reach_the_raw_callback() {
        let mut monitor = ButtonMonitor::new(false);
        let raw = Rc::new(RefCell::new(Vec::new()));
        let events = Rc::new(Cell::new(0));
        let (raw_sink, event_sink) = (raw.clone(), events.clone());
        monitor
            .set_on_raw_report(move |id, bytes| raw_sink.borrow_mut().push((id, bytes.to_vec())));
        monitor.set_on_event(move |_| event_sink.set(event_sink.get() + 1));
        monitor.handle(0x07, &[0x07, 0x01]);
        monitor.handle(0x06, &[0x04]);
        assert_eq!(raw.borrow().len(), 2);
        assert_eq!(events.get(), 1);
    }
}
