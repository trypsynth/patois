//! Translator credits from the `Last-Translator` headers of the `.po` files, so an app credits a
//! new translator in the same commit that adds their language.

use std::{collections::BTreeSet, fmt::Write as _, fs, io, path::Path};

/// Placeholders gettext writes in a catalog that no one has claimed.
const PLACEHOLDERS: [&str; 2] = ["FULL NAME", "EMAIL@ADDRESS"];

/// The name in a catalog's `Last-Translator` header, without the email address.
///
/// The address is left out on purpose: a credit in an About dialog is a thank-you, and
/// translators gave their address to the maintainers, not to everyone who installs the app.
#[must_use]
pub fn last_translator(catalog: &str) -> Option<String> {
	let line = catalog.lines().find(|line| line.contains("Last-Translator:"))?;
	let after = line.split("Last-Translator:").nth(1)?;
	// The header is a quoted gettext string, so it ends in a literal \n and a quote.
	let name = after.split('<').next()?.replace("\\n", "").replace('"', "");
	let name = name.trim();
	if !name.chars().any(char::is_alphabetic) || PLACEHOLDERS.iter().any(|placeholder| name.contains(placeholder)) {
		return None;
	}
	Some(name.to_string())
}

/// The translators of the `.po` files in `po_dir`, sorted and without duplicates.
#[must_use]
pub fn translators(po_dir: &Path) -> Vec<String> {
	let Ok(entries) = fs::read_dir(po_dir) else {
		return Vec::new();
	};
	let names: BTreeSet<String> = entries
		.flatten()
		.map(|entry| entry.path())
		.filter(|path| path.extension().is_some_and(|ext| ext == "po"))
		.filter_map(|path| fs::read_to_string(path).ok())
		.filter_map(|catalog| last_translator(&catalog))
		.collect();
	names.into_iter().collect()
}

/// Writes the [`translators`] of `po_dir` to `out` as `pub static TRANSLATORS: &[&str]`.
///
/// Call it from a build script and `include!` the result. It also tells Cargo to run the build
/// script again when the catalogs change.
///
/// # Errors
///
/// Returns an error if `out` can't be written.
pub fn write_translators_module(po_dir: &Path, out: &Path) -> io::Result<()> {
	println!("cargo:rerun-if-changed={}", po_dir.display());
	let mut code = String::from("pub static TRANSLATORS: &[&str] = &[\n");
	for name in translators(po_dir) {
		let _ = writeln!(code, "\t{name:?},");
	}
	code.push_str("];\n");
	fs::write(out, code)
}

#[cfg(test)]
mod tests {
	use super::last_translator;

	#[test]
	fn the_name_is_taken_without_the_address() {
		let header = r#""Last-Translator: Michał Dziwisz <michal@dziwisz.net>\n""#;
		assert_eq!(Some("Michał Dziwisz".to_string()), last_translator(header));
	}

	#[test]
	fn an_unclaimed_catalog_credits_nobody() {
		assert_eq!(None, last_translator(r#""Last-Translator: FULL NAME <EMAIL@ADDRESS>\n""#));
	}

	#[test]
	fn a_header_with_no_name_credits_nobody() {
		assert_eq!(None, last_translator(r#""Last-Translator: <someone@example.com>\n""#));
	}

	#[test]
	fn an_empty_header_credits_nobody() {
		assert_eq!(None, last_translator(r#""Last-Translator: \n""#));
		assert_eq!(None, last_translator(r#""Last-Translator: ""#));
	}

	#[test]
	fn a_catalog_without_the_header_credits_nobody() {
		assert_eq!(None, last_translator("msgid \"\"\nmsgstr \"\"\n"));
	}
}
