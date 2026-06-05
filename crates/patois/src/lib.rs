//! Patois - lightweight i18n for Rust desktop apps and utility crates.
//!
//! # Quick start
//!
//! In library crates, no translation infrastructure is needed:
//! ```rust,no_run
//! use patois::t;
//! fn update_message() -> String { t("Checking for updates...") }
//! ```
//! The host app registers a default domain; `t()` finds the translation there.
//!
//! App crates require you to register a domain at startup:
//! ```rust,no_run
//! patois::init("paperback", "./langs", "de_DE");
//! ```
//! All subsequent `t()` calls (in any crate) look up in `paperback.mo`.
//!
//! # Advanced: per-crate domain inference
//!
//! `t!` and `nt!` expand `env!("CARGO_PKG_NAME")` at compile time, falling back to the default domain when no crate-specific catalog is registered.

mod catalog;
mod registry;

pub use inventory;
pub use patois_macros::embed_domain;

use std::path::Path;

/// Registered by [`embed_domain!`] for each library crate that embeds its locale files.
///
/// This type is public so the macro output can name it, but you should never construct it manually. Use `embed_domain!()` instead.
pub struct EmbeddedDomain {
	pub name: &'static str,
	pub files: &'static [(&'static str, &'static [u8])],
}

inventory::collect!(EmbeddedDomain);

/// Translate a string literal using the calling crate's package name as the domain.
///
/// Returns the translation for the current locale, or the original string if no translation is found.
#[macro_export]
macro_rules! t {
	($msgid:literal) => {
		$crate::__translate(env!("CARGO_PKG_NAME"), $msgid)
	};
}

/// Translate a plural form using the calling crate's package name as the domain.
///
/// Selects the singular or plural translation based on `n` and the current locale.
#[macro_export]
macro_rules! nt {
	($singular:literal, $plural:literal, $n:expr) => {
		$crate::__translate_plural(env!("CARGO_PKG_NAME"), $singular, $plural, ($n) as u64)
	};
}

/// Initialise patois for the application.
///
/// Registers the app domain's locale directory, sets it as the default domain, and activates the given locale. Call once at startup before any `t!` or `nt!` calls.
///
/// Locale files are expected at `<locale_dir>/<locale>/LC_MESSAGES/<domain>.mo`.
pub fn init(domain: &str, locale_dir: impl AsRef<Path>, locale_id: &str) {
	let mut reg = registry::global().lock().unwrap();
	reg.register_path(domain, locale_dir.as_ref().to_path_buf());
	reg.set_default_domain(domain);
	reg.set_locale(locale_id);
}

/// Translate a string using the currently registered default domain.
///
/// Use this in library crates that let the host app own all translations.
/// Returns the original string unchanged if no default domain is set or no translation is found.
pub fn t(s: &str) -> String {
	let mut reg = registry::global().lock().unwrap();
	let domain = reg.default_domain().map(str::to_string);
	match domain {
		Some(d) => reg.translate(&d, s),
		None => s.to_string(),
	}
}

/// Plural form of [`t`] using the currently registered default domain.
pub fn nt(singular: &str, plural: &str, n: u64) -> String {
	let mut reg = registry::global().lock().unwrap();
	let domain = reg.default_domain().map(str::to_string);
	match domain {
		Some(d) => reg.translate_plural(&d, singular, plural, n),
		None => {
			if n == 1 {
				singular.to_string()
			} else {
				plural.to_string()
			}
		}
	}
}

/// Change the active locale. Clears all cached catalogs.
///
/// All subsequent `t!` and `nt!` calls will use the new locale.
pub fn set_locale(locale_id: &str) {
	registry::global().lock().unwrap().set_locale(locale_id);
}

/// Returns the currently active locale string.
pub fn get_locale() -> String {
	registry::global().lock().unwrap().locale().to_string()
}

#[doc(hidden)]
pub fn __translate(domain: &str, msgid: &str) -> String {
	registry::global().lock().unwrap().translate(domain, msgid)
}

#[doc(hidden)]
pub fn __translate_plural(domain: &str, singular: &str, plural: &str, n: u64) -> String {
	registry::global().lock().unwrap().translate_plural(domain, singular, plural, n)
}
