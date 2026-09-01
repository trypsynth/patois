//! Programmatic, position-preserving editing of `.po` files: find entries that still need a
//! translation, patch a translated string into just that entry's `msgstr`, and re-render the
//! file with everything else byte-for-byte untouched. Generic gettext-format logic with no
//! opinion on where a translation comes from (DeepL, a human, anything else).

use std::ops::Range;

use crate::entries::{po_unescape, pot_escape};

/// A translated result headed for one entry.
///
/// The two shapes are separate variants rather than a `Vec` that happens to have one element,
/// so a caller can't hand a plural entry a single string (collapsing `msgstr[0..n]` into one
/// `msgstr` and breaking the entry) without saying so explicitly.
pub enum Translation {
	Singular(String),
	/// One string per plural form, in index order: `msgstr[0]` first. How many there should be
	/// is [`PoDocument::nplurals`].
	Plural(Vec<String>),
}

/// A single `msgid`/`msgstr` entry, plus the exact line ranges in the source file needed
/// to patch it in place without disturbing anything else (header metadata, TRANSLATORS
/// comments, spacing, other entries).
pub struct PoEntryLoc {
	pub msgid: String,
	pub msgstr: String,
	/// The `msgid_plural` text, for a plural entry. `None` for an ordinary one, and the thing
	/// to branch on when you need to tell them apart.
	pub msgid_plural: Option<String>,
	/// The `msgstr[N]` values in index order, for a plural entry. Empty for an ordinary one,
	/// whose single translation lives in [`Self::msgstr`].
	///
	/// How many there are is a property of the target language, not of the string: the
	/// `nplurals` in the file's `Plural-Forms` header (see [`PoDocument::nplurals`]) says how
	/// many the language has, and Russian's three are as normal as French's two.
	pub msgstr_plural: Vec<String>,
	/// The `#.` note above the entry, as gettext writes it for a `TRANSLATORS:` comment in the
	/// source, with the lines of a run joined by a space and the marker itself dropped. `None`
	/// where the entry has no note.
	pub comment: Option<String>,
	pub is_fuzzy: bool,
	/// Whether this entry carries a `#|` "previous msgid" comment, `msgmerge` writes one
	/// exactly when *it* just fuzzy-matched this entry against a changed source string.
	/// An entry can also be fuzzy without one: that's how [`PoDocument::apply_all`] leaves an
	/// entry after machine-translating it (flagged for human review, but with nothing left
	/// to compare against). Distinguishing the two matters for
	/// [`PoDocument::needs_translation`], otherwise a machine-translated entry would look
	/// exactly like a freshly-changed one and get re-translated every single run forever.
	pub has_prev_msgid: bool,
	/// Range of `#,`/`#|` lines immediately preceding `msgid` (empty range at the msgid
	/// line itself when there's no existing flag/prev-msgid block to replace, inserting
	/// into an empty range is just an insert, not a removal).
	flag_block: Range<usize>,
	/// Range of the full `msgstr "..."` statement, including any continuation lines.
	msgstr_range: Range<usize>,
}

/// A parsed `.po` file, kept as its original lines so entries that aren't touched are
/// re-rendered byte-for-byte identical to the source.
///
/// Plural entries (`msgid_plural`/`msgstr[N]`) are parsed like any other, but kept on their own
/// track: [`Self::needs_translation`] and [`Self::apply_all`] deal only in single strings and
/// skip them, while [`Self::needs_plural_translation`] and [`Translation::Plural`] carry the
/// whole set of forms. Writing one string into a plural entry would collapse `msgstr[0..n]`
/// into a single `msgstr` and quietly destroy the entry, so the two are kept apart by type
/// rather than by remembering to check.
pub struct PoDocument {
	lines: Vec<String>,
	pub entries: Vec<PoEntryLoc>,
}

impl PoDocument {
	#[must_use]
	pub fn parse(content: &str) -> Self {
		let lines: Vec<String> = content.lines().map(str::to_string).collect();
		let mut entries = Vec::new();
		let mut flag_line: Option<usize> = None;
		let mut prev_msgid_range: Option<(usize, usize)> = None;
		let mut pending_comment: Vec<String> = Vec::new();
		let mut i = 0;
		while i < lines.len() {
			let trimmed = lines[i].trim_start();
			if trimmed.starts_with('#') {
				if let Some(note) = trimmed.strip_prefix("#.") {
					let note = note.trim();
					// `TRANSLATORS:` marks the note for whoever reads the catalog; what follows
					// it is the note itself.
					pending_comment.push(note.strip_prefix("TRANSLATORS:").unwrap_or(note).trim().to_string());
				} else if trimmed.starts_with("#,") {
					flag_line = Some(i);
				} else if trimmed.starts_with("#|") {
					prev_msgid_range = Some(prev_msgid_range.map_or((i, i + 1), |(start, _)| (start, i + 1)));
				}
				i += 1;
				continue;
			}
			if trimmed.is_empty() {
				flag_line = None;
				prev_msgid_range = None;
				pending_comment.clear();
				i += 1;
				continue;
			}
			let Some(rest) = trimmed.strip_prefix("msgid ") else {
				i += 1;
				continue;
			};
			let msgid_start = i;
			let mut msgid = po_unescape(rest);
			i += 1;
			while i < lines.len() && lines[i].trim_start().starts_with('"') {
				msgid.push_str(&po_unescape(lines[i].trim_start()));
				i += 1;
			}
			// A `msgid_plural` line, when there is one, sits between the msgid and the
			// translations and decides which of the two shapes follows: one `msgstr`, or a
			// run of `msgstr[N]`.
			let mut msgid_plural = None;
			if i < lines.len() && lines[i].trim_start().starts_with("msgid_plural ") {
				let rest = lines[i].trim_start().strip_prefix("msgid_plural ").unwrap().to_string();
				let mut plural = po_unescape(&rest);
				i += 1;
				while i < lines.len() && lines[i].trim_start().starts_with('"') {
					plural.push_str(&po_unescape(lines[i].trim_start()));
					i += 1;
				}
				msgid_plural = Some(plural);
			}
			let parsed = if msgid_plural.is_some() {
				read_plural_msgstrs(&lines, &mut i).map(|(forms, range)| (String::new(), forms, range))
			} else {
				read_msgstr(&lines, &mut i).map(|(msgstr, range)| (msgstr, Vec::new(), range))
			};
			if let Some((msgstr, msgstr_plural, msgstr_range)) = parsed {
				let is_fuzzy = flag_line.is_some_and(|fl| lines[fl].contains("fuzzy"));
				let flag_block_start = match (flag_line, prev_msgid_range) {
					(Some(f), Some((p, _))) => f.min(p),
					(Some(f), None) => f,
					(None, Some((p, _))) => p,
					(None, None) => msgid_start,
				};
				entries.push(PoEntryLoc {
					msgid,
					msgstr,
					msgid_plural,
					msgstr_plural,
					comment: (!pending_comment.is_empty()).then(|| pending_comment.join(" ")),
					is_fuzzy,
					has_prev_msgid: prev_msgid_range.is_some(),
					flag_block: flag_block_start..msgid_start,
					msgstr_range,
				});
			}
			flag_line = None;
			prev_msgid_range = None;
			pending_comment.clear();
		}
		Self { lines, entries }
	}

	/// Entries that still need a translation: blank `msgstr`, or fuzzy *because `msgmerge`
	/// just changed it* (has a `#|` previous-msgid comment, see
	/// [`PoEntryLoc::has_prev_msgid`]). A fuzzy entry with no `#|` is one already
	/// machine-translated on a prior run and left flagged for human review; treating that
	/// as a candidate too would re-translate the same unchanged text forever. The header
	/// entry (`msgid ""`) is never a candidate.
	pub fn needs_translation(&self) -> impl Iterator<Item = (usize, &str)> {
		self.entries
			.iter()
			.enumerate()
			.filter(|(_, e)| {
				e.msgid_plural.is_none()
					&& !e.msgid.is_empty()
					&& (e.msgstr.is_empty() || (e.is_fuzzy && e.has_prev_msgid))
			})
			.map(|(i, e)| (i, e.msgid.as_str()))
	}

	/// Plural entries that still need translating, as `(index, msgid, msgid_plural)`.
	///
	/// The same rule as [`Self::needs_translation`], with "blank" meaning any form is blank:
	/// a language with three forms and only two filled in is as untranslated as one with none,
	/// and gettext falls back to the msgid for the missing slot.
	pub fn needs_plural_translation(&self) -> impl Iterator<Item = (usize, &str, &str)> {
		self.entries.iter().enumerate().filter_map(|(i, e)| {
			let plural = e.msgid_plural.as_deref()?;
			let blank = e.msgstr_plural.is_empty() || e.msgstr_plural.iter().any(String::is_empty);
			(!e.msgid.is_empty() && (blank || (e.is_fuzzy && e.has_prev_msgid))).then_some((
				i,
				e.msgid.as_str(),
				plural,
			))
		})
	}

	/// The `nplurals=N` value from the file's `Plural-Forms` header, which is how many forms
	/// this language wants. `None` when the header is absent or unparsable, which is a reason
	/// to leave the file's plurals alone rather than guess at two.
	#[must_use]
	pub fn nplurals(&self) -> Option<usize> {
		let header = self.entries.iter().find(|e| e.msgid.is_empty())?;
		let start = header.msgstr.find("nplurals")?;
		let rest = header.msgstr[start..].split_once('=')?.1;
		let digits: String = rest.trim_start().chars().take_while(char::is_ascii_digit).collect();
		digits.parse().ok().filter(|n| *n > 0)
	}

	/// Applies a batch of `(entry index, translated text)` results: for each entry, the
	/// existing `#,`/`#|` block (if any) collapses to a plain `#, fuzzy` line and the
	/// `msgstr` is replaced. Everything else in the file is untouched. All ranges are
	/// computed against the original parse, so patches are applied bottom-up in one pass
	/// to keep earlier ranges valid.
	pub fn apply_all(&mut self, translations: &[(usize, String)]) {
		let owned: Vec<(usize, Translation)> =
			translations.iter().map(|(i, t)| (*i, Translation::Singular(t.clone()))).collect();
		self.apply(&owned);
	}

	/// Applies a batch of results, singular or plural.
	///
	/// One entry point for both shapes because the ranges are all computed against the original
	/// parse: applying singulars and plurals in two passes would leave the second pass patching
	/// line numbers the first pass had already shifted. Everything is collected, sorted from the
	/// bottom of the file up, and spliced in one go.
	///
	/// A [`Translation::Plural`] aimed at an entry that has no `msgid_plural` is skipped rather
	/// than written: it would replace the entry's single `msgstr` with a run of `msgstr[N]`
	/// lines that gettext would reject against that msgid. The reverse is skipped for the same
	/// reason.
	pub fn apply(&mut self, translations: &[(usize, Translation)]) {
		let mut ops: Vec<(Range<usize>, Vec<String>)> = Vec::new();
		for (idx, translated) in translations {
			let entry = &self.entries[*idx];
			let replacement = match (translated, entry.msgid_plural.is_some()) {
				(Translation::Singular(text), false) => vec![format!("msgstr \"{}\"", pot_escape(text))],
				(Translation::Plural(forms), true) => {
					forms.iter().enumerate().map(|(n, form)| format!("msgstr[{n}] \"{}\"", pot_escape(form))).collect()
				}
				_ => continue,
			};
			ops.push((entry.msgstr_range.clone(), replacement));
			ops.push((entry.flag_block.clone(), vec!["#, fuzzy".to_string()]));
		}
		ops.sort_by_key(|op| std::cmp::Reverse(op.0.start));
		for (range, replacement) in ops {
			self.lines.splice(range, replacement);
		}
	}

	#[must_use]
	pub fn render(&self) -> String {
		let mut out = self.lines.join("\n");
		out.push('\n');
		out
	}
}

/// Reads a single `msgstr "..."` statement and its continuation lines, advancing `i` past it.
/// `None` when the line isn't a `msgstr` at all, which means this wasn't a complete entry.
fn read_msgstr(lines: &[String], i: &mut usize) -> Option<(String, Range<usize>)> {
	if *i >= lines.len() || !lines[*i].trim_start().starts_with("msgstr ") {
		return None;
	}
	let start = *i;
	let rest = lines[*i].trim_start().strip_prefix("msgstr ").unwrap().to_string();
	let mut msgstr = po_unescape(&rest);
	*i += 1;
	while *i < lines.len() && lines[*i].trim_start().starts_with('"') {
		msgstr.push_str(&po_unescape(lines[*i].trim_start()));
		*i += 1;
	}
	Some((msgstr, start..*i))
}

/// Reads the run of `msgstr[N] "..."` statements after a `msgid_plural`, in file order,
/// advancing `i` past all of them.
///
/// The bracketed index is not used to place the value: gettext writes them in order from 0, and
/// trusting the order rather than the label means a file with a duplicated or missing index
/// still round-trips as the lines that are actually there, instead of silently dropping a form
/// into the wrong slot.
fn read_plural_msgstrs(lines: &[String], i: &mut usize) -> Option<(Vec<String>, Range<usize>)> {
	if *i >= lines.len() || !lines[*i].trim_start().starts_with("msgstr[") {
		return None;
	}
	let start = *i;
	let mut forms = Vec::new();
	while *i < lines.len() {
		let trimmed = lines[*i].trim_start();
		if !trimmed.starts_with("msgstr[") {
			break;
		}
		let Some((_, rest)) = trimmed.split_once(']') else {
			break;
		};
		let mut value = po_unescape(rest.trim_start());
		*i += 1;
		while *i < lines.len() && lines[*i].trim_start().starts_with('"') {
			value.push_str(&po_unescape(lines[*i].trim_start()));
			*i += 1;
		}
		forms.push(value);
	}
	Some((forms, start..*i))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn find<'a>(doc: &'a PoDocument, msgid: &str) -> &'a PoEntryLoc {
		doc.entries.iter().find(|e| e.msgid == msgid).unwrap_or_else(|| panic!("no entry for {msgid:?}"))
	}

	#[test]
	fn an_entry_carries_the_note_written_above_it() {
		let src = "#. TRANSLATORS: shown on hover
msgid \"Warning\"
msgstr \"\"
";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries[0].comment.as_deref(), Some("shown on hover"));
	}

	/// A note can run to several lines; gettext writes one `#.` per line.
	#[test]
	fn a_note_over_several_lines_is_joined() {
		let src = "#. TRANSLATORS: first half
#. second half
msgid \"Warning\"
msgstr \"\"
";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries[0].comment.as_deref(), Some("first half second half"));
	}

	#[test]
	fn an_entry_without_a_note_has_none() {
		let doc = PoDocument::parse(
			"msgid \"Warning\"
msgstr \"\"
",
		);
		assert_eq!(doc.entries[0].comment, None);
	}

	/// A blank line ends the entry a note belongs to, so it cannot drift onto the next one.
	#[test]
	fn a_note_does_not_reach_past_a_blank_line() {
		let src = "#. TRANSLATORS: about the first
msgid \"One\"
msgstr \"\"

msgid \"Two\"
msgstr \"\"
";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries[0].comment.as_deref(), Some("about the first"));
		assert_eq!(doc.entries[1].comment, None);
	}

	#[test]
	fn header_entry_is_never_a_translation_candidate() {
		let doc = PoDocument::parse("msgid \"\"\nmsgstr \"\"\n\"Language: de\\n\"\n");
		assert_eq!(doc.needs_translation().count(), 0);
	}

	#[test]
	fn already_translated_entry_is_left_untouched() {
		let src = "msgid \"Cancel\"\nmsgstr \"Abbrechen\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.needs_translation().count(), 0);
		assert_eq!(doc.render(), src);
	}

	#[test]
	fn blank_entry_with_no_flag_gets_a_fuzzy_line_inserted() {
		let src = "msgid \"Warning\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		let candidates: Vec<_> = doc.needs_translation().map(|(i, m)| (i, m.to_string())).collect();
		assert_eq!(candidates, vec![(0, "Warning".to_string())]);
		doc.apply_all(&[(0, "Warnung".to_string())]);
		assert_eq!(doc.render(), "#, fuzzy\nmsgid \"Warning\"\nmsgstr \"Warnung\"\n");
	}

	#[test]
	fn translators_comment_is_preserved_when_inserting_a_fuzzy_line() {
		let src = "#. TRANSLATORS: shown on hover\nmsgid \"Warning\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply_all(&[(0, "Warnung".to_string())]);
		assert_eq!(doc.render(), "#. TRANSLATORS: shown on hover\n#, fuzzy\nmsgid \"Warning\"\nmsgstr \"Warnung\"\n");
	}

	#[test]
	fn fuzzy_entry_with_prev_msgid_comment_is_retranslated_and_comment_dropped() {
		let src = "#, fuzzy\n#| msgid \"No pages.\"\nmsgid \"No images.\"\nmsgstr \"Sem pagina.\"\n";
		let mut doc = PoDocument::parse(src);
		let entry = find(&doc, "No images.");
		assert!(entry.is_fuzzy);
		assert!(entry.has_prev_msgid, "msgmerge-authored fuzzy match must carry a #| comment");
		assert_eq!(doc.needs_translation().count(), 1, "a genuinely stale fuzzy match is a candidate");
		let idx = doc.entries.iter().position(|e| e.msgid == "No images.").unwrap();
		doc.apply_all(&[(idx, "Sem imagens.".to_string())]);
		assert_eq!(doc.render(), "#, fuzzy\nmsgid \"No images.\"\nmsgstr \"Sem imagens.\"\n");
	}

	/// After `apply_all` translates an entry it's left `#, fuzzy` with no `#|` comment
	/// (see the test above). A later run must not treat that as a fresh candidate, that
	/// would re-translate the same unchanged text forever.
	#[test]
	fn fuzzy_entry_without_prev_msgid_comment_is_not_retranslated() {
		let src = "#, fuzzy\nmsgid \"No images.\"\nmsgstr \"Sem imagens.\"\n";
		let doc = PoDocument::parse(src);
		let entry = find(&doc, "No images.");
		assert!(entry.is_fuzzy);
		assert!(!entry.has_prev_msgid);
		assert_eq!(doc.needs_translation().count(), 0);
	}

	#[test]
	fn multiline_msgid_is_decoded_and_can_be_translated() {
		let src = "msgid \"\"\n\"Are you sure you want to remove the selected document? This will also remove \"\n\"its reading position and bookmarks.\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		let entry = find(
			&doc,
			"Are you sure you want to remove the selected document? This will also remove its reading position and bookmarks.",
		);
		assert!(entry.msgstr.is_empty());
		let idx = 0;
		doc.apply_all(&[(idx, "Translated.".to_string())]);
		assert!(doc.render().contains("msgstr \"Translated.\""));
	}

	#[test]
	fn multiple_entries_patch_independently_in_one_pass() {
		let src = "msgid \"A\"\nmsgstr \"\"\n\nmsgid \"B\"\nmsgstr \"already\"\n\nmsgid \"C\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		let candidates: Vec<_> = doc.needs_translation().map(|(i, m)| (i, m.to_string())).collect();
		assert_eq!(candidates, vec![(0, "A".to_string()), (2, "C".to_string())]);
		doc.apply_all(&[(0, "A-translated".to_string()), (2, "C-translated".to_string())]);
		let rendered = doc.render();
		assert!(rendered.contains("msgid \"A\"\nmsgstr \"A-translated\""));
		assert!(rendered.contains("msgid \"B\"\nmsgstr \"already\""));
		assert!(rendered.contains("msgid \"C\"\nmsgstr \"C-translated\""));
	}

	#[test]
	fn round_trip_escapes_quotes_and_newlines_in_translated_text() {
		let src = "msgid \"Quote\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply_all(&[(0, "She said \"hi\"\nline two".to_string())]);
		let rendered = doc.render();
		assert!(rendered.contains("msgstr \"She said \\\"hi\\\"\\nline two\""));
	}

	// A plural entry is parsed now, but stays out of the single-string API: handing one a bare
	// string would collapse msgstr[0..n] into one msgstr and break the entry.
	#[test]
	fn a_plural_entry_is_parsed_but_never_a_singular_candidate() {
		let src = "msgid \"{} result\"\nmsgid_plural \"{} results\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries.len(), 1);
		assert_eq!(doc.entries[0].msgid_plural.as_deref(), Some("{} results"));
		assert_eq!(doc.entries[0].msgstr_plural, vec![String::new(), String::new()]);
		assert_eq!(doc.needs_translation().count(), 0, "plurals must not reach the single-string path");
		assert_eq!(doc.render(), src, "an untouched plural entry round-trips byte for byte");
	}

	#[test]
	fn a_blank_plural_entry_is_a_plural_candidate() {
		let src = "msgid \"{} result\"\nmsgid_plural \"{} results\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let doc = PoDocument::parse(src);
		let got: Vec<_> = doc.needs_plural_translation().collect();
		assert_eq!(got, vec![(0, "{} result", "{} results")]);
	}

	// Russian has three forms. Two filled and one blank is as untranslated as none: gettext
	// falls back to the msgid for the empty slot.
	#[test]
	fn a_partly_filled_plural_entry_still_needs_translation() {
		let src =
			"msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"файл\"\nmsgstr[1] \"файла\"\nmsgstr[2] \"\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.needs_plural_translation().count(), 1);
	}

	#[test]
	fn a_fully_translated_plural_entry_is_left_alone() {
		let src = "msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"a\"\nmsgstr[1] \"b\"\nmsgstr[2] \"c\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.needs_plural_translation().count(), 0);
	}

	#[test]
	fn applying_a_plural_writes_every_form_and_flags_it_fuzzy() {
		let src = "msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\nmsgstr[2] \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[(
			0,
			Translation::Plural(vec!["%d файл".to_string(), "%d файла".to_string(), "%d файлов".to_string()]),
		)]);
		let rendered = doc.render();
		assert!(rendered.contains("msgstr[0] \"%d файл\""));
		assert!(rendered.contains("msgstr[1] \"%d файла\""));
		assert!(rendered.contains("msgstr[2] \"%d файлов\""));
		assert!(rendered.contains("#, fuzzy"));
		assert!(!rendered.contains("msgstr \""), "the plural shape must survive");
	}

	// The count of forms is a property of the language, so writing three where the file had two
	// is correct, not an overflow to guard against.
	#[test]
	fn applying_more_forms_than_were_there_replaces_the_whole_run() {
		let src = "msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[(0, Translation::Plural(vec!["a".to_string(), "b".to_string(), "c".to_string()]))]);
		let rendered = doc.render();
		assert!(rendered.contains("msgstr[2] \"c\""));
		assert_eq!(rendered.matches("msgstr[").count(), 3);
	}

	// Mixing the shapes would corrupt the entry, so a mismatch is dropped rather than written.
	#[test]
	fn a_shape_mismatch_is_skipped_rather_than_written() {
		let src = "msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[(0, Translation::Singular("nope".to_string()))]);
		assert_eq!(doc.render(), src);

		let src = "msgid \"Cancel\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[(0, Translation::Plural(vec!["a".to_string()]))]);
		assert_eq!(doc.render(), src);
	}

	#[test]
	fn singular_and_plural_entries_patch_together_in_one_pass() {
		let src = "msgid \"Cancel\"\nmsgstr \"\"\n\nmsgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[
			(0, Translation::Singular("Annuler".to_string())),
			(1, Translation::Plural(vec!["%d fichier".to_string(), "%d fichiers".to_string()])),
		]);
		let rendered = doc.render();
		assert!(rendered.contains("msgstr \"Annuler\""));
		assert!(rendered.contains("msgstr[0] \"%d fichier\""));
		assert!(rendered.contains("msgstr[1] \"%d fichiers\""));
	}

	#[test]
	fn nplurals_comes_from_the_header() {
		let src = "msgid \"\"\nmsgstr \"\"\n\"Plural-Forms: nplurals=3; plural=(n%10==1 ? 0 : 1);\\n\"\n";
		assert_eq!(PoDocument::parse(src).nplurals(), Some(3));
	}

	#[test]
	fn nplurals_is_none_without_a_usable_header() {
		assert_eq!(PoDocument::parse("msgid \"\"\nmsgstr \"\"\n").nplurals(), None);
		let broken = "msgid \"\"\nmsgstr \"\"\n\"Plural-Forms: nplurals=; plural=0;\\n\"\n";
		assert_eq!(PoDocument::parse(broken).nplurals(), None);
	}

	#[test]
	fn a_multiline_plural_form_is_joined() {
		let src = "msgid \"a\"\nmsgid_plural \"b\"\nmsgstr[0] \"\"\n\"joined\"\nmsgstr[1] \"x\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries[0].msgstr_plural, vec!["joined".to_string(), "x".to_string()]);
	}
}
