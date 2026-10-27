//! Installed plugins: finding them, installing one (a symlink into the plugins folder), and their
//! enabled state and settings in the app's defaults domain, so the app and the CLI share them.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSFileManager, NSNumber, NSString, NSURL, NSUserDefaults};

use crate::defaults;
use crate::protocol::{
    JsonValue, ManifestError, PROTOCOL_VERSION, PluginManifest, PluginRequest, SUPPORTED_PROTOCOLS,
    TakeDescriptor,
};

/// An installed plugin: its folder and parsed manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct InstalledPlugin {
    pub folder: PathBuf,
    pub manifest: PluginManifest,
}

impl InstalledPlugin {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn executable_path(&self) -> PathBuf {
        self.folder.join(&self.manifest.executable)
    }

    pub fn handles(&self, event: &str) -> bool {
        self.manifest.events.iter().flatten().any(|e| e == event)
    }

    /// A post-job request in the protocol version this plugin declared, so older plugins keep working.
    pub fn request(
        &self,
        kind: &str,
        name: &str,
        take: &TakeDescriptor,
        settings: BTreeMap<String, JsonValue>,
    ) -> PluginRequest {
        let folder = self.folder.to_string_lossy();
        let mut request =
            PluginRequest::new(kind, name, take.clone(), settings, &take.folder, &folder);
        request.protocol = self.manifest.protocol.min(PROTOCOL_VERSION);
        request
    }
}

/// A plugin folder that was skipped.
#[derive(Debug)]
pub enum PluginProblem {
    /// The manifest asks for a protocol this Lavdeck doesn't speak.
    UnsupportedProtocol { name: String, protocol: i64 },
    /// `plugin.json` is missing or unreadable; `entry` is the folder's name.
    BadManifest { entry: String, error: ManifestError },
}

impl fmt::Display for PluginProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProtocol { name, protocol } => {
                write!(
                    f,
                    "{name}: needs plugin protocol {protocol}, Lavdeck speaks up to {PROTOCOL_VERSION}"
                )
            }
            Self::BadManifest { entry, error } => write!(f, "{entry}: {error}"),
        }
    }
}

#[derive(Debug)]
pub enum InstallError {
    Manifest(ManifestError),
    Io(std::io::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InstallError {}

/// Plugins on disk and their state in defaults.
pub struct PluginStore {
    defaults: Retained<NSUserDefaults>,
}

impl Default for PluginStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginStore {
    /// The store in Lavdeck's own defaults domain.
    pub fn new() -> Self {
        Self::with_defaults(defaults::domain())
    }

    pub fn with_defaults(defaults: Retained<NSUserDefaults>) -> Self {
        Self { defaults }
    }

    /// `~/Library/Application Support/Lavdeck/Plugins`. Each child folder (or symlink) is one plugin.
    pub fn plugins_folder() -> PathBuf {
        defaults::home().join("Library/Application Support/Lavdeck/Plugins")
    }

    /// Plugins that parse and speak a supported protocol version, by name, and the problems found.
    pub fn discover(&self) -> (Vec<InstalledPlugin>, Vec<PluginProblem>) {
        Self::discover_in(&Self::plugins_folder())
    }

    pub(crate) fn discover_in(folder: &Path) -> (Vec<InstalledPlugin>, Vec<PluginProblem>) {
        let _ = std::fs::create_dir_all(folder);
        let mut entries: Vec<_> = std::fs::read_dir(folder)
            .map(|e| e.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        entries.sort();
        let mut plugins = Vec::new();
        let mut problems = Vec::new();
        for entry in entries {
            let name = entry
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            let folder = std::fs::canonicalize(&entry).unwrap_or(entry);
            match PluginManifest::load(&folder) {
                Ok(manifest) if !SUPPORTED_PROTOCOLS.contains(&manifest.protocol) => {
                    problems.push(PluginProblem::UnsupportedProtocol {
                        name: manifest.name,
                        protocol: manifest.protocol,
                    });
                }
                Ok(manifest) => plugins.push(InstalledPlugin { folder, manifest }),
                Err(error) => problems.push(PluginProblem::BadManifest { entry: name, error }),
            }
        }
        plugins.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        (plugins, problems)
    }

    /// Installs a plugin folder by symlinking it into the plugins folder, replacing an older link.
    pub fn install(&self, folder: &Path) -> Result<InstalledPlugin, InstallError> {
        Self::install_in(&Self::plugins_folder(), folder)
    }

    pub(crate) fn install_in(
        plugins: &Path,
        folder: &Path,
    ) -> Result<InstalledPlugin, InstallError> {
        let manifest = PluginManifest::load(folder).map_err(InstallError::Manifest)?;
        let link = plugins.join(&manifest.id);
        std::fs::create_dir_all(plugins).map_err(InstallError::Io)?;
        match std::fs::symlink_metadata(&link) {
            // An old link, working or broken, is just replaced.
            Ok(meta) if meta.file_type().is_symlink() => {
                std::fs::remove_file(&link).map_err(InstallError::Io)?
            }
            // A real folder or file by that name goes to the Trash rather than being deleted.
            Ok(_) => trash(&link).map_err(InstallError::Io)?,
            Err(_) => {}
        }
        std::os::unix::fs::symlink(folder, &link).map_err(InstallError::Io)?;
        Ok(InstalledPlugin {
            folder: folder.to_path_buf(),
            manifest,
        })
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        defaults::bool_dictionary(&self.defaults, "plugins.enabled")
            .and_then(|all| all.get(id).copied())
            .unwrap_or(false)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) {
        let mut all =
            defaults::bool_dictionary(&self.defaults, "plugins.enabled").unwrap_or_default();
        all.insert(id.into(), enabled);
        let values: Vec<(String, Retained<AnyObject>)> = all
            .into_iter()
            .map(|(k, v)| (k, any(NSNumber::new_bool(v))))
            .collect();
        defaults::set_dictionary(&self.defaults, "plugins.enabled", &values);
    }

    /// Effective settings: the manifest defaults overlaid with the user's values.
    pub fn settings(&self, plugin: &InstalledPlugin) -> BTreeMap<String, JsonValue> {
        let mut values = BTreeMap::new();
        for setting in plugin.manifest.settings.iter().flatten() {
            values.insert(
                setting.key.clone(),
                setting.default.clone().unwrap_or(JsonValue::Null),
            );
        }
        let stored =
            defaults::dictionary(&self.defaults, &settings_key(plugin.id())).unwrap_or_default();
        for (key, raw) in stored {
            if let Some(value) = json_value(&raw) {
                values.insert(key, value);
            }
        }
        values
    }

    pub fn set_setting(&self, value: &JsonValue, key: &str, plugin_id: &str) {
        let name = settings_key(plugin_id);
        let mut stored: Vec<(String, Retained<AnyObject>)> =
            defaults::dictionary(&self.defaults, &name)
                .unwrap_or_default()
                .into_iter()
                .filter(|(k, _)| k != key)
                .collect();
        let object: Option<Retained<AnyObject>> = match value {
            JsonValue::Bool(b) => Some(any(NSNumber::new_bool(*b))),
            JsonValue::Number(n) => Some(any(NSNumber::new_f64(*n))),
            JsonValue::String(s) => Some(any(NSString::from_str(s))),
            JsonValue::Null => None,
        };
        if let Some(object) = object {
            stored.push((key.into(), object));
        }
        defaults::set_dictionary(&self.defaults, &name, &stored);
    }
}

fn any<T: objc2::Message>(object: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { Retained::cast_unchecked(object) }
}

fn settings_key(plugin_id: &str) -> String {
    format!("plugins.settings.{plugin_id}")
}

/// A stored setting: booleans, numbers and strings; anything else is ignored.
fn json_value(object: &AnyObject) -> Option<JsonValue> {
    if let Some(string) = object.downcast_ref::<NSString>() {
        return Some(JsonValue::String(string.to_string()));
    }
    let number = object.downcast_ref::<NSNumber>()?;
    if defaults::is_boolean(object) {
        Some(JsonValue::Bool(number.as_bool()))
    } else {
        Some(JsonValue::Number(number.as_f64()))
    }
}

fn trash(path: &Path) -> std::io::Result<()> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|e| std::io::Error::other(e.localizedDescription().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PluginSettingKind;
    use crate::testing::scratch_folder;

    fn write_manifest(folder: &Path, json: &str) {
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(folder.join("plugin.json"), json).unwrap();
    }

    #[test]
    fn post_requests_use_the_plugins_declared_protocol() {
        let v1 = InstalledPlugin {
            folder: "/tmp".into(),
            manifest: PluginManifest::new("a", "A", "1", 1, "x"),
        };
        let take = TakeDescriptor::new("d/s", "/tmp", "s");
        assert_eq!(
            v1.request("action", "go", &take, BTreeMap::new()).protocol,
            1
        );
        let v3 = InstalledPlugin {
            folder: "/tmp".into(),
            manifest: PluginManifest::new("a", "A", "1", 3, "x"),
        };
        assert_eq!(
            v3.request("action", "go", &take, BTreeMap::new()).protocol,
            2
        );
    }

    #[test]
    fn discovery_sorts_by_name_and_reports_problems() {
        let root = scratch_folder();
        let plugins = root.join("Plugins");
        write_manifest(
            &root.join("src/zed"),
            r#"{"id":"z","name":"Zed","version":"1","protocol":1,"executable":"run"}"#,
        );
        write_manifest(
            &plugins.join("alpha"),
            r#"{"id":"a","name":"Alpha","version":"2","protocol":2,"executable":"run"}"#,
        );
        write_manifest(
            &plugins.join("future"),
            r#"{"id":"f","name":"Future","version":"1","protocol":9,"executable":"run"}"#,
        );
        write_manifest(&plugins.join("broken"), r#"{"id":"b","name":"Broken"}"#);
        std::fs::create_dir_all(plugins.join("empty")).unwrap();
        std::fs::create_dir_all(plugins.join(".hidden")).unwrap();
        PluginStore::install_in(&plugins, &root.join("src/zed")).unwrap();
        // Installing again replaces the link.
        PluginStore::install_in(&plugins, &root.join("src/zed")).unwrap();
        let (found, problems) = PluginStore::discover_in(&plugins);
        assert_eq!(
            found
                .iter()
                .map(|p| p.manifest.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "Zed"]
        );
        assert_eq!(
            found[1].folder,
            std::fs::canonicalize(root.join("src/zed")).unwrap()
        );
        let problems: Vec<String> = problems.iter().map(ToString::to_string).collect();
        assert_eq!(
            problems,
            [
                "broken: The data couldn’t be read because it is missing.",
                "empty: The file “plugin.json” couldn’t be opened because there is no such file.",
                "Future: needs plugin protocol 9, Lavdeck speaks up to 2",
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enabled_state_and_settings_live_in_defaults() {
        let suite = "io.github.piedol.lavdeck.kit-tests";
        let store = PluginStore::with_defaults(defaults::suite(suite));
        store
            .defaults
            .removePersistentDomainForName(&NSString::from_str(suite));
        let mut manifest = PluginManifest::new("t.p", "T", "1", 1, "run");
        manifest.settings = Some(vec![
            serde_json::from_str(
                r#"{"key":"bleed_db","title":"Bleed","type":"number","default":6}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"key":"subtitles","title":"Subs","type":"bool","default":true}"#,
            )
            .unwrap(),
            serde_json::from_str(r#"{"key":"model","title":"Model","type":"path"}"#).unwrap(),
        ]);
        assert_eq!(
            manifest.settings.as_ref().unwrap()[0].kind,
            PluginSettingKind::Number
        );
        let plugin = InstalledPlugin {
            folder: "/tmp".into(),
            manifest,
        };

        assert!(!store.is_enabled("t.p"));
        store.set_enabled("t.p", true);
        store.set_enabled("other", false);
        assert!(store.is_enabled("t.p") && !store.is_enabled("other"));

        let initial = store.settings(&plugin);
        assert_eq!(initial["bleed_db"], JsonValue::Number(6.0));
        assert_eq!(initial["subtitles"], JsonValue::Bool(true));
        assert_eq!(initial["model"], JsonValue::Null);
        store.set_setting(&JsonValue::Number(1.0), "bleed_db", "t.p");
        store.set_setting(&JsonValue::Bool(false), "subtitles", "t.p");
        store.set_setting(&JsonValue::String("~/m".into()), "model", "t.p");
        let changed = store.settings(&plugin);
        // A number stays a number even when it is 1 (Swift's bridging read it back as `true`).
        assert_eq!(changed["bleed_db"], JsonValue::Number(1.0));
        assert_eq!(changed["subtitles"], JsonValue::Bool(false));
        assert_eq!(changed["model"], JsonValue::String("~/m".into()));
        store.set_setting(&JsonValue::Null, "model", "t.p");
        assert_eq!(store.settings(&plugin)["model"], JsonValue::Null);
        store
            .defaults
            .removePersistentDomainForName(&NSString::from_str(suite));
    }
}
