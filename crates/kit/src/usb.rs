//! Bulk link to the RX's vendor interface 6 (IN 0x86 / OUT 0x06).
//!
//! Opened with IOUSBHost **without** seizing it, so macOS's accessoryd keeps its own client on
//! the interface. Reads run on a dedicated thread. Private to the crate: outside it, only the
//! allowlisted commands in [`crate::MicController`] can reach the receiver.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_core_foundation::{CFDictionary, CFMutableDictionary, CFNumber, CFRetained, CFString};
use objc2_foundation::NSMutableData;
use objc2_io_kit::{
    IOObjectRelease, IOServiceGetMatchingService, IOServiceMatching, io_service_t,
    kIOMainPortDefault,
};
use objc2_io_usb_host::{IOUSBHostInterface, IOUSBHostObjectInitOptions, IOUSBHostPipe};

use crate::controller::{Link, LinkError, LinkEvents};

pub const VENDOR_ID: i32 = 0x2ca3;
pub const PRODUCT_ID: i32 = 0x4011;
pub const INTERFACE_NUMBER: i32 = 6;
const IN_ENDPOINT: usize = 0x86;
const OUT_ENDPOINT: usize = 0x06;
/// `kIOReturnTimeout`: a read that found nothing to read.
const IO_RETURN_TIMEOUT: isize = 0xE000_02D6_u32 as i32 as isize;

/// An `io_service_t` we hold a reference on.
struct Service(io_service_t);

impl Drop for Service {
    fn drop(&mut self) {
        IOObjectRelease(self.0);
    }
}

/// The open interface and its pipes. IOUSBHost objects may be used from any thread.
struct Open {
    interface: Retained<IOUSBHostInterface>,
    in_pipe: Retained<IOUSBHostPipe>,
    out_pipe: Retained<IOUSBHostPipe>,
    /// Released after the interface, as IOUSBHost asks (fields drop in order).
    _service: Service,
}

// SAFETY: IOUSBHostInterface and IOUSBHostPipe are documented as thread-safe; the Swift version
// shares them between its reader thread and callers the same way.
unsafe impl Send for Open {}
unsafe impl Sync for Open {}

#[derive(Default)]
pub(crate) struct UsbLink {
    open: Mutex<Option<Arc<Open>>>,
}

impl UsbLink {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn find_service() -> io_service_t {
        // SAFETY: plain IOKit matching calls; the matching dictionary is consumed by the lookup.
        unsafe {
            let Some(matching) = IOServiceMatching(c"IOUSBHostInterface".as_ptr()) else {
                return 0;
            };
            let properties = CFMutableDictionary::<CFString, CFNumber>::empty();
            for (key, value) in [
                ("idVendor", VENDOR_ID),
                ("idProduct", PRODUCT_ID),
                ("bInterfaceNumber", INTERFACE_NUMBER),
            ] {
                properties.set(&CFString::from_str(key), &CFNumber::new_i32(value));
            }
            let key = CFString::from_str("IOPropertyMatch");
            CFMutableDictionary::set_value(
                Some(&matching),
                (&*key as *const CFString).cast(),
                (&*properties as *const CFMutableDictionary<CFString, CFNumber>).cast(),
            );
            IOServiceGetMatchingService(
                kIOMainPortDefault,
                Some(CFRetained::cast_unchecked::<CFDictionary>(matching)),
            )
        }
    }

    fn read_loop(self: Arc<Self>, open: Arc<Open>, events: Arc<dyn LinkEvents>) {
        let Some(buffer) = NSMutableData::dataWithLength(1024) else {
            return;
        };
        let mut consecutive_errors = 0;
        loop {
            if !self.is_current(&open) {
                return;
            }
            let mut count: usize = 0;
            // SAFETY: `buffer` outlives the synchronous request; `count` is a valid out pointer.
            let result = unsafe {
                open.in_pipe
                    .sendIORequestWithData_bytesTransferred_completionTimeout_error(
                        Some(&buffer),
                        &mut count,
                        0.5,
                    )
            };
            match result {
                Ok(()) => {
                    consecutive_errors = 0;
                    if count > 0 {
                        // SAFETY: nothing else touches the buffer while we read what the pipe wrote.
                        let bytes = unsafe { buffer.as_bytes_unchecked() };
                        events.bytes(&bytes[..count.min(bytes.len())]);
                    }
                }
                Err(error) if error.code() == IO_RETURN_TIMEOUT => continue,
                Err(_) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= 3 || !Self::present() {
                        let was_running = self.is_current(&open);
                        self.close_open(&open);
                        if was_running {
                            events.disconnected();
                        }
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    fn is_current(&self, open: &Arc<Open>) -> bool {
        self.open
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, open))
    }

    /// Closes `open` if it is still the current interface.
    fn close_open(&self, open: &Arc<Open>) {
        let taken = {
            let mut guard = self.open.lock().unwrap();
            if guard
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, open))
            {
                guard.take()
            } else {
                None
            }
        };
        if let Some(open) = taken {
            destroy(&open);
        }
    }

    fn present() -> bool {
        let service = Self::find_service();
        drop(Service(service));
        service != 0
    }
}

fn destroy(open: &Open) {
    // SAFETY: destroying aborts outstanding I/O, which the reader treats as a closed link.
    unsafe { open.interface.destroy() };
}

impl Link for UsbLink {
    fn is_present(&self) -> bool {
        Self::present()
    }

    fn is_open(&self) -> bool {
        self.open.lock().unwrap().is_some()
    }

    fn open(self: Arc<Self>, events: Arc<dyn LinkEvents>) -> Result<(), LinkError> {
        let service = Service(Self::find_service());
        if service.0 == 0 {
            return Err(LinkError::NotFound);
        }
        let opened = (|| {
            let mut error = None;
            // SAFETY: no seize option (accessoryd keeps its client), default queue, no interest handler.
            let interface = unsafe {
                IOUSBHostInterface::initWithIOService_options_queue_error_interestHandler(
                    IOUSBHostInterface::alloc(),
                    service.0,
                    IOUSBHostObjectInitOptions::None,
                    None,
                    Some(&mut error),
                    std::ptr::null_mut(),
                )
            };
            let interface = interface.ok_or_else(|| describe(error.as_deref()))?;
            // SAFETY: plain method calls on the open interface; on failure the user client we
            // just made is destroyed again.
            let pipes = unsafe {
                interface
                    .copyPipeWithAddress_error(IN_ENDPOINT)
                    .and_then(|i| Ok((i, interface.copyPipeWithAddress_error(OUT_ENDPOINT)?)))
            };
            match pipes {
                Ok((in_pipe, out_pipe)) => Ok((interface, in_pipe, out_pipe)),
                Err(e) => {
                    unsafe { interface.destroy() };
                    Err(describe(Some(&e)))
                }
            }
        })();
        let open = match opened {
            Ok((interface, in_pipe, out_pipe)) => Arc::new(Open {
                interface,
                in_pipe,
                out_pipe,
                _service: service,
            }),
            Err(message) => return Err(LinkError::Open(message)),
        };
        *self.open.lock().unwrap() = Some(open.clone());
        let link = self.clone();
        thread::Builder::new()
            .name("lavdeck-kit.usb.read".into())
            .spawn(move || link.read_loop(open, events))
            .map_err(|e| LinkError::Open(e.to_string()))?;
        Ok(())
    }

    fn close(&self) {
        let taken = self.open.lock().unwrap().take();
        if let Some(open) = taken {
            destroy(&open);
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<(), LinkError> {
        let Some(open) = self.open.lock().unwrap().clone() else {
            return Err(LinkError::NotOpen);
        };
        let data = NSMutableData::with_bytes(bytes);
        let mut sent: usize = 0;
        // SAFETY: `data` outlives the synchronous request; `sent` is a valid out pointer.
        unsafe {
            open.out_pipe
                .sendIORequestWithData_bytesTransferred_completionTimeout_error(
                    Some(&data),
                    &mut sent,
                    1.0,
                )
        }
        .map_err(|e| LinkError::Write(describe(Some(&e))))
    }
}

fn describe(error: Option<&objc2_foundation::NSError>) -> String {
    match error {
        Some(error) => format!(
            "{} (0x{:08x})",
            error.localizedDescription(),
            error.code() as i32 as u32
        ),
        None => "IOUSBHost returned no interface".into(),
    }
}
