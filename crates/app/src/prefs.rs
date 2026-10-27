//! The app's settings in the user defaults, under the Swift app's keys, so existing settings
//! and mic names carry over: `recordingsRoot`, `format`, `splitTracks`, `tapToRecord`, `armed`,
//! `micNames` and `video.settings`, in the `io.github.piedol.lavdeck` domain (the standard one in
//! the app bundle, the suite elsewhere, as the kit and CLI open it). The demo never reads or
//! writes it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUserDefaults};

use crate::model::settings::{RecordingFormat, Settings};
use crate::util::{self, ns};

fn defaults() -> Retained<NSUserDefaults> {
    lavdeck_kit::defaults::domain()
}

pub fn load() -> Settings {
    let d = defaults();
    let mut s = Settings::defaults(&util::home());
    if let Some(root) = d.stringForKey(&ns("recordingsRoot")) {
        s.recordings_root = PathBuf::from(root.to_string());
    }
    if let Some(f) = d
        .stringForKey(&ns("format"))
        .and_then(|f| RecordingFormat::from_key(&f.to_string()))
    {
        s.format = f;
    }
    let bool_or = |key: &str, fallback: bool| {
        if d.objectForKey(&ns(key)).is_some() {
            d.boolForKey(&ns(key))
        } else {
            fallback
        }
    };
    s.split_tracks = bool_or("splitTracks", true);
    s.tap_to_record = bool_or("tapToRecord", true);
    if let Some(armed) = d.arrayForKey(&ns("armed")) {
        s.armed = armed
            .iter()
            .filter_map(|o| o.downcast::<NSNumber>().ok())
            .map(|n| n.integerValue())
            .filter(|i| (1..=2).contains(i))
            .map(|i| i as u8)
            .collect::<BTreeSet<u8>>();
    }
    if let Some(names) = d.dictionaryForKey(&ns("micNames")) {
        let (keys, values) = names.to_vecs();
        s.mic_names = keys
            .into_iter()
            .zip(values)
            .filter_map(|(k, v)| Some((k.to_string(), v.downcast::<NSString>().ok()?.to_string())))
            .collect::<BTreeMap<_, _>>();
    }
    // Includes the move from the Screen & Camera plugin's settings, as the Swift app did.
    s.video = crate::convert::video_from(&lavdeck_capture::VideoSettings::load_from(&d));
    s
}

pub fn save(s: &Settings) {
    let d = defaults();
    let set = |key: &str, value: &AnyObject| {
        // SAFETY: property-list values only.
        unsafe { d.setObject_forKey(Some(value), &ns(key)) }
    };
    set("recordingsRoot", &ns(&s.recordings_root.to_string_lossy()));
    set("format", &ns(s.format.key()));
    d.setBool_forKey(s.split_tracks, &ns("splitTracks"));
    d.setBool_forKey(s.tap_to_record, &ns("tapToRecord"));
    let armed: Vec<Retained<NSNumber>> = s
        .armed
        .iter()
        .map(|i| NSNumber::new_isize(*i as isize))
        .collect();
    set("armed", &NSArray::from_retained_slice(&armed));
    let keys: Vec<Retained<NSString>> = s.mic_names.keys().map(|k| ns(k)).collect();
    let values: Vec<Retained<NSString>> = s.mic_names.values().map(|v| ns(v)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let names = NSDictionary::from_retained_objects(&key_refs, &values);
    set("micNames", &names);
    crate::convert::video(&s.video).save_to(&d);
}
