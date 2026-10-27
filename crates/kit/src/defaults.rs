//! Lavdeck's settings domain, `io.github.piedol.lavdeck`, shared by the app and the CLI. The app
//! reads it as its standard defaults; any other process opens it as a suite.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFBoolean, CFType};
use objc2_foundation::{NSBundle, NSDictionary, NSNumber, NSString, NSUserDefaults};

pub const DEFAULTS_SUITE: &str = "io.github.piedol.lavdeck";

/// The settings domain: standard defaults inside the app bundle, the suite anywhere else.
pub fn domain() -> Retained<NSUserDefaults> {
    let bundle_id = NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|id| id.to_string());
    if bundle_id.as_deref() == Some(DEFAULTS_SUITE) {
        return NSUserDefaults::standardUserDefaults();
    }
    suite(DEFAULTS_SUITE)
}

/// A named defaults suite (tests use their own).
pub fn suite(name: &str) -> Retained<NSUserDefaults> {
    NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&NSString::from_str(name)))
        .unwrap_or_else(NSUserDefaults::standardUserDefaults)
}

/// The user's home folder.
pub fn home() -> PathBuf {
    PathBuf::from(objc2_foundation::NSHomeDirectory().to_string())
}

/// `recordingsRoot`, else `~/Recordings/Lavdeck`.
pub fn recordings_root(defaults: &NSUserDefaults) -> PathBuf {
    defaults
        .stringForKey(&NSString::from_str("recordingsRoot"))
        .map(|s| PathBuf::from(s.to_string()))
        .unwrap_or_else(|| home().join("Recordings/Lavdeck"))
}

/// `micNames`: the user's names for physical mics, keyed by Bluetooth ID.
pub fn mic_names(defaults: &NSUserDefaults) -> HashMap<String, String> {
    let Some(dict) = dictionary(defaults, "micNames") else {
        return HashMap::new();
    };
    let mut names = HashMap::new();
    for (key, value) in dict {
        // Like Swift's `as? [String: String]`: one odd value and the whole dictionary is ignored.
        let Ok(value) = value.downcast::<NSString>() else {
            return HashMap::new();
        };
        names.insert(key, value.to_string());
    }
    names
}

/// A dictionary setting, keys as strings.
pub(crate) fn dictionary(
    defaults: &NSUserDefaults,
    key: &str,
) -> Option<BTreeMap<String, Retained<AnyObject>>> {
    let dict: Retained<NSDictionary<NSString, AnyObject>> =
        defaults.dictionaryForKey(&NSString::from_str(key))?;
    let mut out = BTreeMap::new();
    for key in dict.allKeys().iter() {
        if let Some(value) = dict.objectForKey(&key) {
            out.insert(key.to_string(), value);
        }
    }
    Some(out)
}

/// Like Swift's `as? [String: Bool]`: every value must be a number equal to 0 or 1.
pub(crate) fn bool_dictionary(
    defaults: &NSUserDefaults,
    key: &str,
) -> Option<BTreeMap<String, bool>> {
    let mut out = BTreeMap::new();
    for (key, value) in dictionary(defaults, key)? {
        let number = value.downcast::<NSNumber>().ok()?;
        let n = number.as_f64();
        if n != 0.0 && n != 1.0 {
            return None;
        }
        out.insert(key, n == 1.0);
    }
    Some(out)
}

/// True for an `NSNumber` that holds a boolean (`kCFBooleanTrue` / `kCFBooleanFalse`).
pub(crate) fn is_boolean(object: &AnyObject) -> bool {
    // SAFETY: every Objective-C object is a CFType for CFGetTypeID's purposes.
    let cf: &CFType = unsafe { &*(object as *const AnyObject).cast::<CFType>() };
    cf.downcast_ref::<CFBoolean>().is_some()
}

/// Writes a dictionary setting.
pub(crate) fn set_dictionary(
    defaults: &NSUserDefaults,
    key: &str,
    values: &[(String, Retained<AnyObject>)],
) {
    let keys: Vec<Retained<NSString>> = values.iter().map(|(k, _)| NSString::from_str(k)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let objects: Vec<&AnyObject> = values.iter().map(|(_, v)| &**v).collect();
    let dict = NSDictionary::from_slices(&key_refs, &objects);
    // SAFETY: a property-list dictionary of strings, numbers and booleans.
    unsafe { defaults.setObject_forKey(Some(&dict), &NSString::from_str(key)) };
}
