//! The interface's own wording for the terms a readme section names, taken from the language's catalog, so the readme calls a menu what the app calls it.

use patois_build::po::PoDocument;

/// Menus, buttons and options only. A longer string is a sentence, which a readme paraphrases rather than names.
const MAX_TERM_WORDS: usize = 4;

/// The most terms sent with one section. A long section names dozens of common words that are also labels, and past this the list costs more than it steers.
const MAX_TERMS_PER_SECTION: usize = 120;

/// English label and its translations, for every short label the catalog translates.
pub struct Terms(Vec<(String, Vec<String>)>);

impl Terms {
	/// Fuzzy entries count too: they are compiled with `--use-fuzzy`, so they are what the app shows.
	pub fn from_catalog(content: &str) -> Self {
		let doc = PoDocument::parse(content);
		let mut entries: Vec<(bool, String, String)> = doc
			.entries
			.iter()
			.filter(|entry| entry.msgid_plural.is_none() && !entry.msgstr.is_empty())
			.filter_map(|entry| Some((!entry.msgid.contains('&'), label(&entry.msgid)?, label(&entry.msgstr)?)))
			.collect();
		// Labels with an accelerator first: those are the menus and buttons a readme names, while the same English without one is often a different control, such as Russian's Go menu «Переход» beside a Go button «Вперед».
		entries.sort_by_key(|(no_accelerator, _, _)| *no_accelerator);
		let mut terms: Vec<(String, Vec<String>)> = Vec::new();
		for (_, english, translated) in entries {
			match terms.iter_mut().find(|(known, _)| *known == english) {
				Some((_, translations)) if !translations.contains(&translated) => translations.push(translated),
				Some(_) => {}
				None => terms.push((english, vec![translated])),
			}
		}
		// Longest first, so a phrase outranks the words inside it when the list is cut short.
		terms.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
		Self(terms)
	}

	/// The terms `section` names, one `English → translation` per line, with every translation the interface uses for it; empty when it names none.
	pub fn for_section(&self, section: &str) -> String {
		let named = self.0.iter().filter(|(english, _)| names(section, english)).take(MAX_TERMS_PER_SECTION);
		named
			.map(|(english, translations)| format!("{english} → {}", translations.join(" / ")))
			.collect::<Vec<_>>()
			.join("\n")
	}
}

/// A catalog string as it reads on screen: its accelerator marks, trailing ellipsis and colon, and any shortcut after a tab taken off. `None` for anything that is not a short label.
fn label(text: &str) -> Option<String> {
	let text = text.split("\\t").next().unwrap_or(text);
	let mut label = String::new();
	let mut chars = text.chars().peekable();
	while let Some(c) = chars.next() {
		match c {
			// A CJK-style "(&F)" accelerator is dropped whole, a plain "&" on its own.
			'(' if chars.peek() == Some(&'&') => {
				let rest: String = chars.clone().take(3).collect();
				if rest.len() == 3 && rest.ends_with(')') {
					chars.nth(2);
				} else {
					label.push(c);
				}
			}
			'&' => {}
			_ => label.push(c),
		}
	}
	let label = label.trim().trim_end_matches("...").trim_end_matches('…').trim_end_matches([':', '：']).trim();
	let plain = !label.is_empty() && !label.contains(['{', '%', '\\', '\n']) && label.chars().any(char::is_alphabetic);
	(plain && label.split_whitespace().count() <= MAX_TERM_WORDS).then(|| label.to_string())
}

/// Whether `text` contains `term` as whole words, so "Go" is found in "the Go menu" but not in "Google".
fn names(text: &str, term: &str) -> bool {
	text.match_indices(term).any(|(at, _)| {
		let before = text[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
		let after = text[at + term.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
		before && after
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	fn catalog(entries: &[(&str, &str)]) -> String {
		entries.iter().map(|(id, str)| format!("msgid \"{id}\"\nmsgstr \"{str}\"\n")).collect::<Vec<_>>().join("\n")
	}

	#[test]
	fn labels_lose_their_accelerators_ellipses_colons_and_shortcuts() {
		assert_eq!(label("&Go"), Some("Go".to_string()));
		assert_eq!(label("Go To &Line...\\tCtrl+G"), Some("Go To Line".to_string()));
		assert_eq!(label("移動(&G)"), Some("移動".to_string()));
		assert_eq!(label("&Password:"), Some("Password".to_string()));
	}

	#[test]
	fn sentences_and_placeholders_are_not_terms() {
		assert_eq!(label("This document contains no pages."), None);
		assert_eq!(label("Page {} of {}"), None);
		assert_eq!(label("%d%%"), None);
	}

	/// The Russian readme renamed the Go menu «Меню навигации» when its section was re-translated, though the interface calls it «Переход».
	#[test]
	fn a_section_gets_the_interface_names_it_mentions() {
		let terms = Terms::from_catalog(&catalog(&[
			("&Go", "&Переход"),
			("&Tools", "&Инструменты"),
			("Google Drive", "Google Диск"),
		]));
		let found = terms.for_section("### Go menu\n\n* `Ctrl+F`: Show the Find dialog.");
		assert_eq!(found, "Go → Переход");
	}

	/// Russian has "&Go" as the Go menu «Переход» and a plain "Go" button as «Вперед». Keeping only one taught the model the button's name for the menu.
	#[test]
	fn every_translation_of_a_label_is_kept_with_the_menu_one_first() {
		let terms = Terms::from_catalog(&catalog(&[("Go", "Вперед"), ("&Go", "&Переход")]));
		assert_eq!(terms.for_section("### Go menu"), "Go → Переход / Вперед");
	}

	#[test]
	fn a_term_inside_a_longer_word_is_not_a_mention() {
		assert!(names("the Go menu", "Go"));
		assert!(!names("Google", "Go"));
		assert!(!names("Ago", "Go"));
	}
}
