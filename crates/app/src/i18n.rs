//! Translated text. The English text is the key: `tr("Quit Lavdeck")` returns it in the
//! language macOS chose for Lavdeck (the first of your preferred languages that Lavdeck has,
//! or the one set for Lavdeck in System Settings › General › Language & Region), and the
//! English text itself otherwise.
//!
//! - `tr("Save")` for fixed text.
//! - `trf("Saved to {folder}", &[("folder", &name)])` for text with values in it.
//! - `trn(n, "{n} file", "{n} files")` for counts. Both forms must be literals.
//!
//! Translations will be in `locales/<lang>.json`, turned by the bundle script into
//! `Contents/Resources/<lang>.lproj/Localizable.strings`, which `NSBundle` reads. English is
//! the only language for now.

use std::sync::OnceLock;

use objc2_foundation::{NSBundle, NSString};

use crate::i18n_text::{fill, is_singular};

pub fn tr(key: &str) -> String {
    let key_ns = NSString::from_str(key);
    NSBundle::mainBundle()
        .localizedStringForKey_value_table(&key_ns, None, None)
        .to_string()
}

pub fn trf(key: &str, args: &[(&str, &str)]) -> String {
    fill(&tr(key), args)
}

/// `{n}` gets the count with the region's thousands separator ("1,234").
pub fn trn(n: u64, one: &str, other: &str) -> String {
    let key = if is_singular(language(), n) {
        one
    } else {
        other
    };
    trf(key, &[("n", &crate::format::count(n))])
}

/// The language macOS picked for this run, such as "de" or "pt-BR"; "en" outside a bundle.
pub fn language() -> &'static str {
    static LANG: OnceLock<String> = OnceLock::new();
    LANG.get_or_init(|| {
        NSBundle::mainBundle()
            .preferredLocalizations()
            .firstObject()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "en".into())
    })
}
