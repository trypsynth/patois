//! Whether a translation kept what isn't prose: the placeholders, the accelerator, and the
//! shortcut suffix of its source, and all the forms a plural needs.
//!
//! Use [`check`] and [`check_plural`] on new translations, such as machine translations, before
//! writing them to a catalog. Use [`is_damaged`] to find bad translations already in a catalog,
//! such as the ones `msgmerge` copies in from similar strings.

/// Returns `translated` if it kept the placeholders, accelerator, and shortcut suffix of
/// `source`, or `None` if it didn't. An empty translation is also rejected.
#[must_use]
pub fn check(source: &str, translated: &str) -> Option<String> {
	if translated.trim().is_empty()
		|| placeholder_counts(source) != placeholder_counts(translated)
		|| accelerator_count(source) != accelerator_count(translated)
		|| shortcut_suffix(source) != shortcut_suffix(translated)
	{
		return None;
	}
	Some(translated.to_string())
}

/// The plural version of [`check`]: returns `forms` if there are exactly `nplurals` of them,
/// none empty, and each has the placeholders of `plural`, the English plural.
///
/// It's all or nothing, because a partly filled plural entry looks translated to every tool,
/// while gettext silently uses the English for the missing forms.
#[must_use]
pub fn check_plural(plural: &str, forms: &[String], nplurals: usize) -> Option<Vec<String>> {
	if forms.len() != nplurals || forms.iter().any(|f| f.trim().is_empty()) {
		return None;
	}
	// The English plural, not the singular: "1 document." can spell the number out, while every
	// translated form needs the placeholder the count goes into.
	let expected = placeholder_counts(plural);
	forms.iter().all(|f| placeholder_counts(f) == expected).then(|| forms.to_vec())
}

/// Whether an existing translation dropped or added a placeholder, an accelerator, or a shortcut
/// suffix. An empty translation isn't damaged, only untranslated.
#[must_use]
pub fn is_damaged(source: &str, translated: &str) -> bool {
	!translated.is_empty() && check(source, translated).is_none()
}

/// How many `%s`, `%d`, and `{}` placeholders a string has.
fn placeholder_counts(s: &str) -> (usize, usize, usize) {
	(s.matches("%s").count(), s.matches("%d").count(), s.matches("{}").count())
}

/// How many accelerator markers a string has: an `&` right before a letter or digit. `&&` is an
/// escaped ampersand, not a marker.
fn accelerator_count(s: &str) -> usize {
	let chars: Vec<char> = s.chars().collect();
	let mut count = 0;
	let mut i = 0;
	while i < chars.len() {
		if chars[i] == '&' {
			match chars.get(i + 1) {
				Some('&') => i += 2,
				Some(c) if c.is_alphanumeric() => {
					count += 1;
					i += 2;
				}
				_ => i += 1,
			}
			continue;
		}
		i += 1;
	}
	count
}

/// The shortcut after the tab, as in `&Copy\tCtrl+C`, which a translation must keep exactly.
fn shortcut_suffix(s: &str) -> Option<&str> {
	s.split_once('\t').map(|(_, suffix)| suffix)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn placeholder_counts_catch_a_dropped_token() {
		assert_eq!(placeholder_counts("%s Heading level %d"), placeholder_counts("Ebene %d von %s"));
		assert_ne!(
			placeholder_counts("Remove the {} selected documents?"),
			placeholder_counts("Sollen die gewahlten Dokumente entfernt werden?")
		);
	}

	#[test]
	fn a_dropped_accelerator_is_rejected() {
		assert_eq!(check("E&xit", "Выход"), None);
		assert_eq!(check("E&xit", "В&ыход"), Some("В&ыход".to_string()));
	}

	#[test]
	fn an_invented_accelerator_is_rejected() {
		assert_eq!(check("Ready", "&Listo"), None);
		assert_eq!(check("Ready", "Listo"), Some("Listo".to_string()));
	}

	#[test]
	fn an_escaped_ampersand_is_not_an_accelerator() {
		assert_eq!(accelerator_count("Search && Replace"), 0);
		assert_eq!(accelerator_count("&Search && Replace"), 1);
		assert_eq!(check("Search && Replace", "Buscar && Reemplazar"), Some("Buscar && Reemplazar".to_string()));
	}

	#[test]
	fn the_accelerator_may_move_to_a_different_letter() {
		assert_eq!(check("&Open", "&Abrir"), Some("&Abrir".to_string()));
		assert_eq!(check("&Open", "A&brir"), Some("A&brir".to_string()));
	}

	#[test]
	fn a_shortcut_suffix_must_come_back_untouched() {
		assert_eq!(check("&Copy\tCtrl+C", "&Copiar\tCtrl+C"), Some("&Copiar\tCtrl+C".to_string()));
		assert_eq!(check("&Copy\tCtrl+C", "&Copiar\tCtrl+D"), None, "a rewritten key combination");
		assert_eq!(check("&Copy\tCtrl+C", "&Copiar"), None, "a dropped shortcut");
	}

	#[test]
	fn an_empty_translation_is_rejected() {
		assert_eq!(check("Ready", "   "), None);
	}

	#[test]
	fn damage_already_in_a_catalog_is_recognised() {
		assert!(is_damaged("&Status:", "Estado"), "dropped accelerator");
		assert!(is_damaged("{} minutes", "minutos"), "dropped placeholder");
		assert!(is_damaged("Page %d", "Página %d: %s"), "invented placeholder");
		assert!(is_damaged("&Copy\tCtrl+C", "&Copiar\tCtrl+D"), "rewritten shortcut");
	}

	#[test]
	fn a_sound_translation_is_not_damaged() {
		assert!(!is_damaged("E&xit", "В&ыход"));
		assert!(!is_damaged("Page %d", "Página %d"));
		assert!(!is_damaged("Ready", ""), "untranslated, not damaged");
	}

	#[test]
	fn a_complete_plural_set_is_accepted() {
		let forms = ["%d документ.".to_string(), "%d документа.".to_string(), "%d документов.".to_string()];
		assert_eq!(check_plural("%d documents.", &forms, 3), Some(forms.to_vec()));
	}

	#[test]
	fn a_short_plural_set_is_rejected() {
		let forms = ["%d документ.".to_string(), "%d документа.".to_string()];
		assert_eq!(check_plural("%d documents.", &forms, 3), None);
	}

	#[test]
	fn a_plural_set_with_a_blank_form_is_rejected() {
		assert_eq!(check_plural("%d documents.", &["a".to_string(), "  ".to_string()], 2), None);
	}

	#[test]
	fn a_plural_form_that_dropped_the_placeholder_is_rejected() {
		let forms = ["%d документ.".to_string(), "документа.".to_string()];
		assert_eq!(check_plural("%d documents.", &forms, 2), None);
	}
}
