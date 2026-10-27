//! How Lavdeck records video alongside the mics, and how it makes each take's finished video. Stored as
//! JSON under `video.settings` in Lavdeck's defaults, byte-compatible with the Swift app's `JSONEncoder`.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSData, NSDictionary, NSNumber, NSString, NSUserDefaults};
use serde::{Deserialize, Serialize};

/// The camera preview's size when recording starts. Double-clicking the preview cycles through them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewSize {
    /// A round bubble.
    #[default]
    Bubble,
    /// A 16:9 view of the whole frame.
    Large,
    /// The screen's usable area.
    Full,
}

impl PreviewSize {
    pub const ALL: [PreviewSize; 3] = [PreviewSize::Bubble, PreviewSize::Large, PreviewSize::Full];

    /// The `rawValue` the Swift app stored.
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewSize::Bubble => "bubble",
            PreviewSize::Large => "large",
            PreviewSize::Full => "full",
        }
    }

    pub fn from_raw(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    /// The size a double-click switches to.
    pub fn next(self) -> Self {
        match self {
            PreviewSize::Bubble => PreviewSize::Large,
            PreviewSize::Large => PreviewSize::Full,
            PreviewSize::Full => PreviewSize::Bubble,
        }
    }

    /// The hint the preview flashes at this size (English; the app words it).
    pub fn hint(self) -> &'static str {
        match self {
            PreviewSize::Bubble => "Double-click to enlarge",
            PreviewSize::Large => "Double-click for full screen",
            PreviewSize::Full => "Double-click to shrink",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    #[default]
    Hevc,
    H264,
}

impl Codec {
    pub fn as_str(self) -> &'static str {
        match self {
            Codec::Hevc => "hevc",
            Codec::H264 => "h264",
        }
    }

    pub fn from_raw(raw: &str) -> Option<Self> {
        [Codec::Hevc, Codec::H264]
            .into_iter()
            .find(|c| c.as_str() == raw)
    }
}

/// Where the camera bubble sits in the finished video.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    #[default]
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

impl Corner {
    pub const ALL: [Corner; 4] = [
        Corner::BottomRight,
        Corner::BottomLeft,
        Corner::TopRight,
        Corner::TopLeft,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Corner::BottomRight => "bottom-right",
            Corner::BottomLeft => "bottom-left",
            Corner::TopRight => "top-right",
            Corner::TopLeft => "top-left",
        }
    }

    pub fn from_raw(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == raw)
    }

    pub fn is_left(self) -> bool {
        matches!(self, Corner::BottomLeft | Corner::TopLeft)
    }

    pub fn is_top(self) -> bool {
        matches!(self, Corner::TopRight | Corner::TopLeft)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BubbleSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl BubbleSize {
    pub fn as_str(self) -> &'static str {
        match self {
            BubbleSize::Small => "small",
            BubbleSize::Medium => "medium",
            BubbleSize::Large => "large",
        }
    }

    pub fn from_raw(raw: &str) -> Option<Self> {
        [BubbleSize::Small, BubbleSize::Medium, BubbleSize::Large]
            .into_iter()
            .find(|s| s.as_str() == raw)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BubbleShape {
    #[default]
    Circle,
    Rounded,
}

impl BubbleShape {
    pub fn as_str(self) -> &'static str {
        match self {
            BubbleShape::Circle => "circle",
            BubbleShape::Rounded => "rounded",
        }
    }

    pub fn from_raw(raw: &str) -> Option<Self> {
        [BubbleShape::Circle, BubbleShape::Rounded]
            .into_iter()
            .find(|s| s.as_str() == raw)
    }
}

/// How Lavdeck records video alongside the mics, and how it makes each take's finished video.
///
/// Every key is required when decoding, as with Swift's synthesized `Codable`: a blob missing one is
/// ignored and the settings fall back to the plugin migration or the defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSettings {
    /// Record the main display.
    pub screen: bool,
    /// Record the default camera.
    pub camera: bool,
    /// Show a floating camera preview while recording (never in the screen recording).
    pub camera_preview: bool,
    pub preview_size: PreviewSize,
    /// Record system audio into the screen recording.
    pub system_audio: bool,
    #[serde(rename = "screenFPS")]
    pub screen_fps: u32,
    pub codec: Codec,
    /// Make `<stem> PiP.mp4` after each take: the screen with the camera in a corner, and the mic audio.
    pub make_video: bool,
    pub bubble_corner: Corner,
    pub bubble_size: BubbleSize,
    pub bubble_shape: BubbleShape,
}

impl Default for VideoSettings {
    fn default() -> Self {
        VideoSettings {
            screen: false,
            camera: false,
            camera_preview: true,
            preview_size: PreviewSize::Bubble,
            system_audio: false,
            screen_fps: 30,
            codec: Codec::Hevc,
            make_video: true,
            bubble_corner: Corner::BottomRight,
            bubble_size: BubbleSize::Medium,
            bubble_shape: BubbleShape::Circle,
        }
    }
}

const KEY: &str = "video.settings";
/// The Screen & Camera plugin these settings replace.
const PLUGIN_ID: &str = "io.github.piedol.lavdeck.capture";

impl VideoSettings {
    pub fn hevc(&self) -> bool {
        self.codec == Codec::Hevc
    }

    pub fn records_video(&self) -> bool {
        self.screen || self.camera
    }

    /// Loads from the standard defaults (`io.github.piedol.lavdeck` in the app bundle).
    pub fn load() -> VideoSettings {
        Self::load_from(&NSUserDefaults::standardUserDefaults())
    }

    pub fn load_from(defaults: &NSUserDefaults) -> VideoSettings {
        if let Some(data) = defaults.dataForKey(&NSString::from_str(KEY))
            && let Ok(saved) = serde_json::from_slice::<VideoSettings>(&data.to_vec())
        {
            return saved;
        }
        migrated(defaults).unwrap_or_default()
    }

    /// Saves to the standard defaults.
    pub fn save(&self) {
        self.save_to(&NSUserDefaults::standardUserDefaults());
    }

    pub fn save_to(&self, defaults: &NSUserDefaults) {
        // Swift stored `Data`; keep it an NSData blob so either app reads the other's settings.
        if let Ok(json) = serde_json::to_vec(self) {
            let data = NSData::from_vec(json);
            unsafe { defaults.setObject_forKey(Some(&data), &NSString::from_str(KEY)) };
        }
    }
}

/// Settings from the Screen & Camera plugin this replaces, so switching to the built-in keeps people's
/// choices.
fn migrated(defaults: &NSUserDefaults) -> Option<VideoSettings> {
    let enabled = defaults
        .dictionaryForKey(&NSString::from_str("plugins.enabled"))
        .and_then(|d| bool_map_value(&d, PLUGIN_ID))
        .unwrap_or(false);
    let old = defaults.dictionaryForKey(&NSString::from_str(&format!(
        "plugins.settings.{PLUGIN_ID}"
    )));
    if old.is_none() && !enabled {
        return None;
    }
    let get = |k: &str| {
        old.as_ref()
            .and_then(|d| d.objectForKey(&NSString::from_str(k)))
    };
    let boolean = |k: &str, fallback: bool| get(k).and_then(|v| as_bool(&v)).unwrap_or(fallback);
    let string = |k: &str| get(k).and_then(|v| as_string(&v));

    Some(VideoSettings {
        // A switched-off plugin recorded nothing; keep it that way.
        screen: enabled && boolean("screen", true),
        camera: enabled && boolean("camera", true),
        camera_preview: boolean("camera_preview", true),
        preview_size: string("camera_preview_size")
            .and_then(|v| PreviewSize::from_raw(&v))
            .unwrap_or_default(),
        system_audio: boolean("system_audio", false),
        screen_fps: string("fps").and_then(|v| v.parse().ok()).unwrap_or(30),
        codec: string("codec")
            .and_then(|v| Codec::from_raw(&v))
            .unwrap_or_default(),
        make_video: boolean("pip", true),
        bubble_corner: string("pip_corner")
            .and_then(|v| Corner::from_raw(&v))
            .unwrap_or_default(),
        bubble_size: string("pip_size")
            .and_then(|v| BubbleSize::from_raw(&v))
            .unwrap_or_default(),
        bubble_shape: string("pip_shape")
            .and_then(|v| BubbleShape::from_raw(&v))
            .unwrap_or_default(),
    })
}

/// Swift's `value as? Bool`: an NSNumber that is exactly 0 or 1.
fn as_bool(value: &AnyObject) -> Option<bool> {
    let n = value.downcast_ref::<NSNumber>()?;
    match n.as_f64() {
        0.0 => Some(false),
        1.0 => Some(true),
        _ => None,
    }
}

/// Swift's `value as? String ?? (value as? NSNumber).map { "\($0.intValue)" }`.
fn as_string(value: &AnyObject) -> Option<String> {
    if let Some(s) = value.downcast_ref::<NSString>() {
        return Some(s.to_string());
    }
    value
        .downcast_ref::<NSNumber>()
        .map(|n| n.as_i32().to_string())
}

/// `(dictionary as? [String: Bool])?[key]`: the cast fails, and so does the lookup, unless every value is a
/// Bool.
fn bool_map_value(dict: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<bool> {
    let values: Vec<Retained<AnyObject>> = dict.allValues().to_vec();
    if !values.iter().all(|v| as_bool(v).is_some()) {
        return None;
    }
    dict.objectForKey(&NSString::from_str(key))
        .and_then(|v| as_bool(&v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::AnyThread;
    use objc2::runtime::AnyObject;

    fn dict(pairs: &[(&str, Retained<AnyObject>)]) -> Retained<NSDictionary<NSString, AnyObject>> {
        let keys: Vec<Retained<NSString>> =
            pairs.iter().map(|(k, _)| NSString::from_str(k)).collect();
        let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let values: Vec<&AnyObject> = pairs.iter().map(|(_, v)| &**v).collect();
        NSDictionary::from_slices(&keys, &values)
    }

    fn obj<T: objc2::Message>(v: Retained<T>) -> Retained<AnyObject> {
        unsafe { Retained::cast_unchecked(v) }
    }

    #[test]
    fn json_matches_the_swift_encoder() {
        let json = serde_json::to_value(VideoSettings::default()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "screen": false, "camera": false, "cameraPreview": true, "previewSize": "bubble",
                "systemAudio": false, "screenFPS": 30, "codec": "hevc", "makeVideo": true,
                "bubbleCorner": "bottom-right", "bubbleSize": "medium", "bubbleShape": "circle",
            })
        );
        // What the Swift app wrote (JSONEncoder escapes slashes and orders keys its own way).
        let swift = br#"{"bubbleShape":"rounded","codec":"h264","camera":true,"screenFPS":60,"systemAudio":true,"makeVideo":false,"previewSize":"full","bubbleCorner":"top-left","cameraPreview":false,"screen":true,"bubbleSize":"small"}"#;
        let s: VideoSettings = serde_json::from_slice(swift).unwrap();
        assert!(s.screen && s.camera && !s.camera_preview && s.system_audio && !s.make_video);
        assert_eq!(s.preview_size, PreviewSize::Full);
        assert_eq!((s.screen_fps, s.codec), (60, Codec::H264));
        assert_eq!(
            (s.bubble_corner, s.bubble_size, s.bubble_shape),
            (Corner::TopLeft, BubbleSize::Small, BubbleShape::Rounded)
        );
        // A missing key or an unknown value fails the whole blob, as in Swift.
        assert!(serde_json::from_str::<VideoSettings>(r#"{"screen":true}"#).is_err());
        let unknown = String::from_utf8(swift.to_vec())
            .unwrap()
            .replace("top-left", "middle");
        assert!(serde_json::from_str::<VideoSettings>(&unknown).is_err());
    }

    #[test]
    fn defaults_and_derived() {
        let s = VideoSettings::default();
        assert!(!s.records_video() && s.hevc() && s.make_video && s.camera_preview);
        assert_eq!(s.screen_fps, 30);
        assert_eq!(PreviewSize::Bubble.next(), PreviewSize::Large);
        assert_eq!(PreviewSize::Large.next(), PreviewSize::Full);
        assert_eq!(PreviewSize::Full.next(), PreviewSize::Bubble);
        assert_eq!(PreviewSize::Large.hint(), "Double-click for full screen");
        for c in Corner::ALL {
            assert_eq!(Corner::from_raw(c.as_str()), Some(c));
        }
    }

    #[test]
    fn value_casts_follow_swift_bridging() {
        assert_eq!(as_bool(&obj(NSNumber::new_bool(true))), Some(true));
        assert_eq!(as_bool(&obj(NSNumber::new_f64(0.0))), Some(false));
        assert_eq!(as_bool(&obj(NSNumber::new_f64(60.0))), None);
        assert_eq!(as_bool(&obj(NSString::from_str("yes"))), None);
        assert_eq!(
            as_string(&obj(NSNumber::new_f64(60.0))).as_deref(),
            Some("60")
        );
        assert_eq!(
            as_string(&obj(NSString::from_str("h264"))).as_deref(),
            Some("h264")
        );
        let mixed = dict(&[
            ("a", obj(NSNumber::new_bool(true))),
            ("b", obj(NSString::from_str("x"))),
        ]);
        assert_eq!(bool_map_value(&mixed, "a"), None);
        let bools = dict(&[("a", obj(NSNumber::new_bool(true)))]);
        assert_eq!(bool_map_value(&bools, "a"), Some(true));
        assert_eq!(bool_map_value(&bools, "b"), None);
    }

    /// The Swift test `videoSettingsCarryOverFromTheScreenAndCameraPlugin`.
    #[test]
    fn carry_over_from_the_screen_and_camera_plugin() {
        // One fixed suite, emptied before and after, so runs don't leave a preferences file each.
        let suite = NSString::from_str("lavdeck.tests.video.rust");
        let defaults =
            NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&suite)).unwrap();
        defaults.removePersistentDomainForName(&suite);
        struct Cleanup<'a>(&'a NSUserDefaults, &'a NSString);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                self.0.removePersistentDomainForName(self.1);
            }
        }
        let _cleanup = Cleanup(&defaults, &suite);

        // No plugin and nothing saved: video stays off.
        assert_eq!(
            VideoSettings::load_from(&defaults),
            VideoSettings::default()
        );
        let enabled = dict(&[(PLUGIN_ID, obj(NSNumber::new_bool(true)))]);
        unsafe {
            defaults.setObject_forKey(Some(&enabled), &NSString::from_str("plugins.enabled"))
        };
        let old = dict(&[
            ("screen", obj(NSNumber::new_bool(false))),
            ("camera", obj(NSNumber::new_bool(true))),
            ("fps", obj(NSNumber::new_f64(60.0))),
            ("pip_corner", obj(NSString::from_str("top-left"))),
            ("camera_preview_size", obj(NSString::from_str("large"))),
        ]);
        unsafe {
            defaults.setObject_forKey(
                Some(&old),
                &NSString::from_str(&format!("plugins.settings.{PLUGIN_ID}")),
            )
        };
        let carried = VideoSettings::load_from(&defaults);
        assert!(!carried.screen && carried.camera);
        assert_eq!(carried.screen_fps, 60);
        assert_eq!(carried.bubble_corner, Corner::TopLeft);
        assert_eq!(carried.preview_size, PreviewSize::Large);
        // Once saved, the built-in settings win.
        let mut changed = carried;
        changed.screen = true;
        changed.save_to(&defaults);
        assert!(VideoSettings::load_from(&defaults).screen);

        // A switched-off plugin recorded nothing; its other choices still carry over.
        defaults.removePersistentDomainForName(&suite);
        let old = dict(&[
            ("screen", obj(NSNumber::new_bool(true))),
            ("pip_shape", obj(NSString::from_str("rounded"))),
        ]);
        unsafe {
            defaults.setObject_forKey(
                Some(&old),
                &NSString::from_str(&format!("plugins.settings.{PLUGIN_ID}")),
            )
        };
        let off = VideoSettings::load_from(&defaults);
        assert!(!off.screen && !off.camera);
        assert_eq!(off.bubble_shape, BubbleShape::Rounded);
    }
}
