use crate::{available_locales, get_locale, set_locale};

/// A language available for the app, with its code and native display name.
#[derive(Debug, Clone)]
pub struct LanguageInfo {
	/// BCP 47 / gettext locale code as stored in the `locale/` directory (e.g. `"de"`, `"pt_br"`).
	pub code: String,
	/// Native name suitable for display in a language-selector UI (e.g. `"Deutsch"`).
	pub name: String,
}

/// Manages runtime language selection for a patois domain.
///
/// # Example
///
/// ```rust,no_run
/// patois::embed_domain!();
///
/// fn main() {
///     patois::init_auto(env!("CARGO_PKG_NAME"));
///     let mgr = patois::LanguageManager::new(env!("CARGO_PKG_NAME"));
///     for lang in mgr.available() {
///         println!("{}: {}", lang.code, lang.name);
///     }
///     mgr.set("de");
/// }
/// ```
pub struct LanguageManager {
	domain: String,
}

impl LanguageManager {
	/// Create a manager for the given patois domain.
	pub fn new(domain: &str) -> Self {
		Self { domain: domain.to_string() }
	}

	/// All available languages: English is always first, the rest are sorted by native name.
	pub fn available(&self) -> Vec<LanguageInfo> {
		let mut langs = vec![LanguageInfo { code: "en".to_string(), name: "English".to_string() }];
		let mut others: Vec<LanguageInfo> = available_locales(&self.domain)
			.into_iter()
			.filter(|&code| code != "en")
			.map(|code| LanguageInfo { code: code.to_string(), name: language_name(code).to_string() })
			.collect();
		others.sort_by(|a, b| a.name.cmp(&b.name));
		langs.extend(others);
		langs
	}

	/// Returns the currently active locale code.
	pub fn current(&self) -> String {
		get_locale()
	}

	/// Switch to `code`. Returns `false` if the code is not in the available list (English always accepted).
	pub fn set(&self, code: &str) -> bool {
		if code == "en" || available_locales(&self.domain).contains(&code) {
			set_locale(code);
			true
		} else {
			false
		}
	}

	/// Detect the operating system's preferred locale, falling back to `"en"`.
	pub fn system_language() -> String {
		crate::system_locale()
	}
}

/// Returns the native display name for a locale code, or the code itself if not recognised.
///
/// Matching is done on the raw code as stored in your `locale/` directory.
pub fn language_name(code: &str) -> &str {
	match code.to_lowercase().as_str() {
		"af" => "Afrikaans",
		"ar" => "العربية",
		"bg" => "Български",
		"bs" => "Bosanski",
		"ca" => "Català",
		"cs" => "Čeština",
		"cy" => "Cymraeg",
		"da" => "Dansk",
		"de" | "de_at" | "de_ch" => "Deutsch",
		"el" => "Ελληνικά",
		"en" | "en_us" | "en_gb" => "English",
		"eo" => "Esperanto",
		"es" | "es_419" => "Español",
		"et" => "Eesti",
		"eu" => "Euskara",
		"fa" => "فارسی",
		"fi" => "Suomi",
		"fr" | "fr_be" | "fr_ca" | "fr_ch" => "Français",
		"ga" => "Gaeilge",
		"gl" => "Galego",
		"he" => "עברית",
		"hi" => "हिन्दी",
		"hr" => "Hrvatski",
		"hu" => "Magyar",
		"hy" => "Հայերեն",
		"id" => "Indonesia",
		"is" => "Íslenska",
		"it" => "Italiano",
		"ja" => "日本語",
		"ka" => "ქართული",
		"ko" => "한국어",
		"lt" => "Lietuvių",
		"lv" => "Latviešu",
		"mk" => "Македонски",
		"ms" => "Bahasa Melayu",
		"nb" => "Norsk bokmål",
		"nl" => "Nederlands",
		"nn" => "Norsk nynorsk",
		"pl" => "Polski",
		"pt" => "Português",
		"pt_br" => "Português (Brasil)",
		"ro" => "Română",
		"ru" => "Русский",
		"sk" => "Slovenčina",
		"sl" => "Slovenščina",
		"sq" => "Shqip",
		"sr" => "Српски",
		"sv" => "Svenska",
		"th" => "ภาษาไทย",
		"tr" => "Türkçe",
		"uk" => "Українська",
		"vi" => "Tiếng Việt",
		"zh" => "中文",
		"zh_cn" => "中文（简体）",
		"zh_tw" => "中文（繁體）",
		_ => code,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn language_name_known() {
		assert_eq!(language_name("de"), "Deutsch");
		assert_eq!(language_name("pt_br"), "Português (Brasil)");
		assert_eq!(language_name("pt_BR"), "Português (Brasil)");
		assert_eq!(language_name("zh_CN"), "中文（简体）");
	}

	#[test]
	fn language_name_unknown_returns_code() {
		assert_eq!(language_name("xx"), "xx");
		assert_eq!(language_name("tok"), "tok");
	}
}
