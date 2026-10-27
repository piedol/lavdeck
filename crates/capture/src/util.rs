//! Small helpers shared by the recorders and the composer.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use dispatch2::{DispatchQueue, DispatchTime};
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::CFString;
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

pub fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&ns(&path.to_string_lossy()))
}

/// A toll-free bridged CF string as the NSString it is.
pub fn cf_ns(s: &CFString) -> &NSString {
    // SAFETY: CFString and NSString are toll-free bridged.
    unsafe { &*(s as *const CFString).cast::<NSString>() }
}

pub fn any<T: Message>(object: &T) -> &AnyObject {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { &*(object as *const T).cast::<AnyObject>() }
}

pub fn number(value: f64) -> Retained<NSNumber> {
    NSNumber::new_f64(value)
}

pub fn integer(value: i64) -> Retained<NSNumber> {
    NSNumber::new_i64(value)
}

/// `[String: Any]` for AVFoundation settings dictionaries.
pub fn dictionary(
    pairs: &[(&NSString, &AnyObject)],
) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<&NSString> = pairs.iter().map(|(k, _)| *k).collect();
    let values: Vec<&AnyObject> = pairs.iter().map(|(_, v)| *v).collect();
    NSDictionary::from_slices(&keys, &values)
}

/// Runs `f` on the main queue soon.
pub fn main_async(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

/// Runs `f` on the main queue after `delay`.
pub fn main_after(delay: Duration, f: impl FnOnce() + Send + 'static) {
    let when = DispatchTime::NOW.time(delay.as_nanos() as i64);
    let _ = DispatchQueue::main().after(when, f);
}

/// Calls `start` with a sender and blocks until a completion handler sends through it. For completion
/// handlers that run on a framework queue (never the calling thread), so waiting can't deadlock.
pub fn wait<T: Send + 'static>(start: impl FnOnce(mpsc::Sender<T>)) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    start(tx);
    rx.recv().ok()
}

/// A value shared with a framework callback queue. AVFoundation writers, inputs and sessions aren't marked
/// thread-safe in objc2, but each is only touched under a mutex or on one serial queue, as in the Swift
/// classes (`@unchecked Sendable`).
pub struct Shared<T>(pub T);
// SAFETY: see the type's docs; every use is serialized.
unsafe impl<T> Send for Shared<T> {}
// SAFETY: as above.
unsafe impl<T> Sync for Shared<T> {}

impl<T> std::ops::Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
