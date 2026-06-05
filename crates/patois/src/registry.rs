use std::{
	collections::HashMap,
	fs::File,
	io::Cursor,
	path::PathBuf,
	sync::{Mutex, OnceLock},
};

use crate::{EmbeddedDomain, catalog::Catalog};

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

pub(crate) fn global() -> &'static Mutex<Registry> {
	REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

#[derive(Default)]
pub(crate) struct Registry {
	/// App domain paths (populated by `init()`). Library domains use embedded bytes instead.
	paths: HashMap<String, PathBuf>,
	default_domain: Option<String>,
	locale: String,
	cache: HashMap<(String, String), Option<Catalog>>,
}

impl Registry {
	pub(crate) fn register_path(&mut self, domain: &str, path: PathBuf) {
		if self.paths.get(domain) == Some(&path) {
			return;
		}
		self.cache.retain(|(_, d), _| d != domain);
		self.paths.insert(domain.to_string(), path);
	}

	pub fn set_default_domain(&mut self, domain: &str) {
		self.default_domain = Some(domain.to_string());
	}

	pub fn default_domain(&self) -> Option<&str> {
		self.default_domain.as_deref()
	}

	pub fn set_locale(&mut self, locale_id: &str) {
		let normalized = normalize_locale(locale_id);
		if self.locale != normalized {
			self.locale = normalized;
			self.cache.clear();
		}
	}

	pub fn locale(&self) -> &str {
		&self.locale
	}

	pub fn translate(&mut self, domain: &str, msgid: &str) -> String {
		self.ensure_catalog(domain);
		let key = (self.locale.clone(), domain.to_string());
		if let Some(Some(catalog)) = self.cache.get(&key) {
			return catalog.gettext(msgid).to_string();
		}
		// Fall back to the default domain when the specific domain has no catalog.
		if let Some(default) = self.default_domain.clone() {
			if default != domain {
				self.ensure_catalog(&default);
				let fallback_key = (self.locale.clone(), default);
				if let Some(Some(catalog)) = self.cache.get(&fallback_key) {
					return catalog.gettext(msgid).to_string();
				}
			}
		}
		msgid.to_string()
	}

	pub fn translate_plural(&mut self, domain: &str, singular: &str, plural: &str, n: u64) -> String {
		self.ensure_catalog(domain);
		let key = (self.locale.clone(), domain.to_string());
		if let Some(Some(catalog)) = self.cache.get(&key) {
			return catalog.ngettext(singular, plural, n).to_string();
		}
		if let Some(default) = self.default_domain.clone() {
			if default != domain {
				self.ensure_catalog(&default);
				let fallback_key = (self.locale.clone(), default);
				if let Some(Some(catalog)) = self.cache.get(&fallback_key) {
					return catalog.ngettext(singular, plural, n).to_string();
				}
			}
		}
		if n == 1 { singular.to_string() } else { plural.to_string() }
	}

	fn ensure_catalog(&mut self, domain: &str) {
		let locale = self.locale.clone();
		let key = (locale.clone(), domain.to_string());
		if !self.cache.contains_key(&key) {
			let catalog = self.load_catalog(domain, &locale);
			self.cache.insert(key, catalog);
		}
	}

	fn load_catalog(&self, domain: &str, locale: &str) -> Option<Catalog> {
		for loc in locale_chain(locale) {
			// Embedded domains (libraries using embed_domain!()) take priority.
			for ed in inventory::iter::<EmbeddedDomain> {
				if ed.name == domain {
					for (file_locale, bytes) in ed.files {
						if *file_locale == loc {
							if let Ok(catalog) = Catalog::parse(Cursor::new(*bytes)) {
								return Some(catalog);
							}
						}
					}
				}
			}
			// Path-based domains (the app's own strings via init()).
			if let Some(base) = self.paths.get(domain) {
				let path = base.join(&loc).join("LC_MESSAGES").join(format!("{domain}.mo"));
				if let Ok(file) = File::open(&path) {
					if let Ok(catalog) = Catalog::parse(file) {
						return Some(catalog);
					}
				}
			}
		}
		None
	}
}

/// Normalise locale strings: "en-US" and "en_us" both become "en_US".
fn normalize_locale(locale: &str) -> String {
	let s = locale.replace('-', "_");
	match s.split_once('_') {
		Some((lang, region)) => format!("{}_{}", lang.to_lowercase(), region.to_uppercase()),
		None => s.to_lowercase(),
	}
}

fn locale_chain(locale: &str) -> Vec<String> {
	let mut chain = vec![locale.to_string()];
	if let Some((lang, _)) = locale.split_once('_') {
		chain.push(lang.to_string());
	}
	chain
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn normalize_various_forms() {
		assert_eq!(normalize_locale("en-US"), "en_US");
		assert_eq!(normalize_locale("DE_de"), "de_DE");
		assert_eq!(normalize_locale("fr"), "fr");
	}

	#[test]
	fn locale_chain_expands_region() {
		assert_eq!(locale_chain("de_DE"), vec!["de_DE", "de"]);
		assert_eq!(locale_chain("fr"), vec!["fr"]);
	}
}
