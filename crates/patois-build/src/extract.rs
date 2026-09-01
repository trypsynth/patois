//! Scanning non-Rust sources (Swift, Kotlin) for `t()`/`nt()` calls and appending what they
//! turn up to an existing `.pot`. A small hand-written scanner rather than another `xgettext`
//! invocation, since those languages need no sanitizing and the call shapes are simple.

use std::{
	collections::{HashMap, HashSet},
	error, fs,
	path::{Path, PathBuf},
};

use crate::entries::{PotEntry, collect_pot_msgids, po_unescape, pot_entry_block};

/// Collect source files with the given extension from a directory tree.
fn collect_source_files(dir: &Path, extension: &str, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn error::Error>> {
	if !dir.is_dir() {
		return Ok(());
	}
	for entry in fs::read_dir(dir)? {
		let path = entry?.path();
		if path.is_dir() {
			collect_source_files(&path, extension, files)?;
		} else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
			files.push(path);
		}
	}
	Ok(())
}

/// Extend an existing `.pot` file with strings from source files in the given directories.
///
/// Scans files matching `extension` (e.g. `"swift"` or `"kt"`) for `t("...")` and
/// `nt("...", "...", ...)` calls using a native Rust parser, no xgettext required. Handles
/// standard C-style escape sequences in string literals and skips strings that are already
/// present in the pot file.
pub fn extend_pot_from_source_dirs(
	dirs: &[impl AsRef<Path>],
	extension: &str,
	pot_file: impl AsRef<Path>,
) -> Result<(), Box<dyn error::Error>> {
	let pot_file = pot_file.as_ref();
	if !pot_file.exists() {
		return Ok(());
	}
	let mut files: Vec<PathBuf> = Vec::new();
	for dir in dirs {
		collect_source_files(dir.as_ref(), extension, &mut files)?;
	}
	if files.is_empty() {
		return Ok(());
	}

	// Collect t("...")/nt("...", "...") calls from every source file, preserving first-seen
	// order. Both share one "seen" set keyed on the singular text so the same string can't be
	// added twice even if it turns up as both a t() and an nt() call somewhere.
	//
	// Every entry is stamped with `extension` as its `#:` reference. That is what marks it as
	// belonging to this scan rather than to the Rust one that wrote the pot, so a later
	// regeneration can carry it across (see `pot::preserve_foreign_entries`) without having to
	// guess, and without that guess also resurrecting deleted Rust msgids.
	let mut new_entries: Vec<PotEntry> = Vec::new();
	let mut seen_in_scan: HashSet<String> = HashSet::new();
	for file in &files {
		let content = fs::read_to_string(file)?;
		for (s, comment) in extract_t_strings(&content) {
			if seen_in_scan.insert(s.clone()) {
				new_entries.push(PotEntry {
					msgid: s,
					msgid_plural: None,
					reference: Some(extension.to_string()),
					comment,
				});
			}
		}
		for (singular, plural, comment) in extract_nt_strings(&content) {
			if seen_in_scan.insert(singular.clone()) {
				new_entries.push(PotEntry {
					msgid: singular,
					msgid_plural: Some(plural),
					reference: Some(extension.to_string()),
					comment,
				});
			}
		}
	}
	if new_entries.is_empty() {
		return Ok(());
	}

	// Read the existing pot and collect msgids already present.
	let existing = fs::read_to_string(pot_file)?;
	let existing_ids = collect_pot_msgids(&existing);

	// Bring the notes on entries already in the pot up to date before appending the rest.
	let notes: HashMap<&str, Option<&str>> =
		new_entries.iter().map(|e| (e.msgid.as_str(), e.comment.as_deref())).collect();
	let refreshed = refresh_foreign_comments(&existing, &notes, extension);

	// Append only truly new entries.
	let mut additions = String::new();
	for entry in &new_entries {
		if !existing_ids.contains(&entry.msgid) {
			additions.push_str(&pot_entry_block(entry));
		}
	}
	if !additions.is_empty() || refreshed != existing {
		fs::write(pot_file, format!("{refreshed}{additions}"))?;
	}
	Ok(())
}

/// Bring the `#.` notes on the entries this scan owns into line with what the sources now say.
///
/// [`extend_pot_from_source_dirs`] only ever *appends* msgids the pot does not have yet, and a
/// regeneration copies the ones it does have across verbatim (see
/// `pot::preserve_foreign_entries`), so without this step a note added, reworded or deleted
/// after its msgid first landed would never reach the catalog. Entries carrying a different
/// `#:` reference, and the xgettext-owned ones carrying none, are left untouched.
fn refresh_foreign_comments(content: &str, notes: &HashMap<&str, Option<&str>>, extension: &str) -> String {
	let reference = format!("#: {extension}");
	// Entries are blank-line separated, and a newline inside a string is escaped rather than
	// literal, so an empty line is always an entry boundary and never part of one.
	content
		.split("\n\n")
		.map(|block| {
			// `lines()` drops a trailing newline, and on the final block that newline is the
			// file's own, so put it back rather than leaving the pot without one.
			let trailing = if block.ends_with('\n') { "\n" } else { "" };
			let lines: Vec<&str> = block.lines().collect();
			if !lines.iter().any(|l| l.trim() == reference) {
				return block.to_string();
			}
			let Some(msgid) = block_msgid(&lines) else {
				return block.to_string();
			};
			let Some(note) = notes.get(msgid.as_str()) else {
				return block.to_string();
			};
			let mut rebuilt: Vec<String> = Vec::with_capacity(lines.len() + 1);
			// A leading empty line is the separator this block was split on; keep it in place.
			let mut rest = lines.as_slice();
			while let Some((first, tail)) = rest.split_first() {
				if first.trim().is_empty() {
					rebuilt.push((*first).to_string());
					rest = tail;
				} else {
					break;
				}
			}
			if let Some(text) = note {
				rebuilt.extend(text.lines().map(|line| format!("#. {line}")));
			}
			rebuilt.extend(rest.iter().filter(|l| !l.starts_with("#.")).map(|l| (*l).to_string()));
			format!("{}{trailing}", rebuilt.join("\n"))
		})
		.collect::<Vec<_>>()
		.join("\n\n")
}

/// The decoded `msgid` of one pot entry's lines, following its continuation lines.
fn block_msgid(lines: &[&str]) -> Option<String> {
	let start = lines.iter().position(|l| l.starts_with("msgid "))?;
	let mut msgid = po_unescape(lines[start].trim_start_matches("msgid "));
	for line in &lines[start + 1..] {
		if line.starts_with('"') {
			msgid.push_str(&po_unescape(line));
		} else {
			break;
		}
	}
	Some(msgid)
}

/// Reads a `"..."` string literal starting at `pos` (may point at whitespace before the
/// opening quote). Returns the decoded string and the index just past the closing quote, or
/// `None` if there's no quote there, or the literal is never terminated (hits a raw newline or
/// end of input first). Handles standard C/Swift/Kotlin escapes (`\\`, `\"`, `\n`, `\t`).
fn read_string_literal(chars: &[char], mut pos: usize) -> Option<(String, usize)> {
	let n = chars.len();
	while pos < n && chars[pos].is_ascii_whitespace() {
		pos += 1;
	}
	if pos >= n || chars[pos] != '"' {
		return None;
	}
	pos += 1;
	let mut s = String::new();
	loop {
		if pos >= n {
			return None;
		}
		match chars[pos] {
			'"' => return Some((s, pos + 1)),
			'\\' if pos + 1 < n => {
				pos += 1;
				match chars[pos] {
					'n' => s.push('\n'),
					't' => s.push('\t'),
					'"' => s.push('"'),
					'\\' => s.push('\\'),
					c => {
						s.push('\\');
						s.push(c);
					}
				}
				pos += 1;
			}
			'\n' | '\r' => return None,
			c => {
				s.push(c);
				pos += 1;
			}
		}
	}
}

/// The tag a note must open with to be picked up, matching the `--add-comments=TRANSLATORS`
/// the Rust scan passes to xgettext.
const COMMENT_TAG: &str = "TRANSLATORS";

/// How many string-free lines may sit between a note and the call it describes.
///
/// A note is normally written directly above its `t(...)`, but a call split over several lines
/// puts the opening line of the construct in between:
///
/// ```text
/// // TRANSLATORS: Section heading
/// Text(
///     t("General"),
/// ```
///
/// Only stepping over lines that hold no string literal is what keeps that safe: such a line
/// can hold no other extractable message, so there is nothing between the note and this call
/// that the note might have belonged to instead.
const MAX_SKIPPED_WRAPPER_LINES: usize = 3;

/// Maps every char index in `chars` to the zero-based line it falls on.
fn line_of_each_char(chars: &[char]) -> Vec<usize> {
	let mut out = Vec::with_capacity(chars.len() + 1);
	let mut line = 0;
	for &c in chars {
		out.push(line);
		if c == '\n' {
			line += 1;
		}
	}
	out.push(line);
	out
}

/// The `TRANSLATORS:` note for the call starting on `line_index`, if it has one.
///
/// Reads the run of `//` lines immediately above (after stepping over at most
/// [`MAX_SKIPPED_WRAPPER_LINES`] wrapper lines) and returns it, tag and all, when that run
/// opens with [`COMMENT_TAG`]. A blank line ends the search, so an unrelated note further up
/// cannot drift down onto a call it was never written for.
fn translators_comment_before(lines: &[&str], line_index: usize) -> Option<String> {
	let mut cursor = line_index;
	let mut skipped = 0;
	while cursor > 0 {
		let above = lines[cursor - 1].trim();
		if above.starts_with("//") {
			break;
		}
		if above.is_empty() || above.contains('"') || skipped == MAX_SKIPPED_WRAPPER_LINES {
			return None;
		}
		skipped += 1;
		cursor -= 1;
	}
	// Walking up collects bottom-up, so reverse to put the note back in reading order.
	let mut block: Vec<&str> = Vec::new();
	while cursor > 0 {
		let Some(text) = lines[cursor - 1].trim().strip_prefix("//") else {
			break;
		};
		block.push(text.trim());
		cursor -= 1;
	}
	block.reverse();
	if block.first()?.starts_with(COMMENT_TAG) { Some(block.join("\n")) } else { None }
}

/// Extract every `t("literal")` value from `content`, each with the `TRANSLATORS:` note
/// written above it, if any.
///
/// Ignores `t(` when preceded by an alphanumeric character or underscore (e.g.
/// `stateDescription`).
fn extract_t_strings(content: &str) -> Vec<(String, Option<String>)> {
	let chars: Vec<char> = content.chars().collect();
	let lines: Vec<&str> = content.lines().collect();
	let line_of = line_of_each_char(&chars);
	let n = chars.len();
	let mut out: Vec<(String, Option<String>)> = Vec::new();
	let mut i = 0;
	while i < n {
		if chars[i] == 't' && i + 1 < n && chars[i + 1] == '(' {
			let preceded_by_ident = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
			if !preceded_by_ident && let Some((s, next)) = read_string_literal(&chars, i + 2) {
				if !s.is_empty() {
					out.push((s, translators_comment_before(&lines, line_of[i])));
				}
				i = next;
				continue;
			}
		}
		i += 1;
	}
	out
}

/// Extract every `nt("singular", "plural", ...)` call's literal pair from `content`,
/// each with the `TRANSLATORS:` note written above it, if any. Ignores the third (count)
/// argument entirely; only the first two string literals matter. Same identifier-boundary rule
/// as [`extract_t_strings`].
fn extract_nt_strings(content: &str) -> Vec<(String, String, Option<String>)> {
	let chars: Vec<char> = content.chars().collect();
	let lines: Vec<&str> = content.lines().collect();
	let line_of = line_of_each_char(&chars);
	let n = chars.len();
	let mut out: Vec<(String, String, Option<String>)> = Vec::new();
	let mut i = 0;
	while i < n {
		if chars[i] == 'n' && i + 1 < n && chars[i + 1] == 't' && i + 2 < n && chars[i + 2] == '(' {
			let preceded_by_ident = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
			if !preceded_by_ident && let Some((singular, after_singular)) = read_string_literal(&chars, i + 3) {
				let mut j = after_singular;
				while j < n && (chars[j].is_ascii_whitespace() || chars[j] == ',') {
					j += 1;
				}
				if let Some((plural, after_plural)) = read_string_literal(&chars, j) {
					if !singular.is_empty() && !plural.is_empty() {
						out.push((singular, plural, translators_comment_before(&lines, line_of[i])));
					}
					i = after_plural;
					continue;
				}
			}
		}
		i += 1;
	}
	out
}
#[cfg(test)]
mod tests {
	use super::*;

	/// Just the extracted texts, for the cases that are not about the notes.
	fn t_texts(src: &str) -> Vec<String> {
		extract_t_strings(src).into_iter().map(|(s, _)| s).collect()
	}

	/// Just the singular/plural pairs, for the cases that are not about the notes.
	fn nt_pairs(src: &str) -> Vec<(String, String)> {
		extract_nt_strings(src).into_iter().map(|(a, b, _)| (a, b)).collect()
	}

	#[test]
	fn extract_simple() {
		assert_eq!(t_texts(r#"Button(t("Cancel")) { dismiss() }"#), vec!["Cancel"]);
	}

	#[test]
	fn extract_multiple() {
		assert_eq!(t_texts(r#"Text(t("Find")) Text(t("Cancel"))"#), vec!["Find", "Cancel"]);
	}

	#[test]
	fn extract_escaped_quote() {
		assert_eq!(t_texts(r#"t("say \"hi\"")"#), vec!["say \"hi\""]);
	}

	#[test]
	fn extract_backslash_escape() {
		// Swift source on disk: t("Regular expression (\\1 = first capture group)")
		// Two actual backslash chars in the file → decoded to one backslash in the msgid.
		let src = "t(\"Regular expression (\\\\1 = first capture group)\")";
		assert_eq!(t_texts(src), vec!["Regular expression (\\1 = first capture group)"]);
	}

	#[test]
	fn skip_ident_suffix_t() {
		// 't' preceded by 'x' in putText → not a t() call
		let src = r#"putText("bad") t("good")"#;
		let got = t_texts(src);
		assert!(got.contains(&"good".to_string()));
		assert!(!got.contains(&"bad".to_string()));
	}

	#[test]
	fn unicode_passthrough() {
		let src = "t(\"Search\u{2026}\")";
		assert_eq!(t_texts(src), vec!["Search\u{2026}"]);
	}

	#[test]
	fn extract_nt_finds_singular_and_plural() {
		let src = r#"Text(nt("{} result", "{} results", count))"#;
		assert_eq!(nt_pairs(src), vec![("{} result".to_string(), "{} results".to_string())]);
	}

	#[test]
	fn extract_nt_ignores_nt_suffix() {
		// 'nt' preceded by 'x' in xnt(...) → not an nt() call.
		let src = r#"xnt("bad", "worse", n) nt("good", "goods", n)"#;
		assert_eq!(nt_pairs(src), vec![("good".to_string(), "goods".to_string())]);
	}
	#[test]
	fn note_directly_above_a_call_is_attached() {
		let src = "// TRANSLATORS: Button that closes the sheet\nText(t(\"Done\"))";
		let got = extract_t_strings(src);
		assert_eq!(got[0].1.as_deref(), Some("TRANSLATORS: Button that closes the sheet"));
	}

	// The note is written above the whole widget, so the line opening it sits in between.
	#[test]
	fn note_carries_over_a_wrapper_line() {
		let src = "// TRANSLATORS: Section heading\nText(\n\tt(\"General\"),\n)";
		let got = extract_t_strings(src);
		assert_eq!(got[0].1.as_deref(), Some("TRANSLATORS: Section heading"));
	}

	#[test]
	fn an_ordinary_comment_is_not_a_note() {
		let src = "// this explains the code, not the string\nt(\"Done\")";
		assert_eq!(extract_t_strings(src)[0].1, None);
	}

	// A blank line is what separates a note from code it was not written for.
	#[test]
	fn a_blank_line_detaches_the_note() {
		let src = "// TRANSLATORS: for something else\n\nt(\"Done\")";
		assert_eq!(extract_t_strings(src)[0].1, None);
	}

	// The note above the first call must not also land on the second.
	#[test]
	fn a_note_does_not_leak_onto_the_next_call() {
		let src = "// TRANSLATORS: about A\nt(\"A\")\nt(\"B\")";
		let got = extract_t_strings(src);
		assert_eq!(got[0].1.as_deref(), Some("TRANSLATORS: about A"));
		assert_eq!(got[1].1, None);
	}

	#[test]
	fn a_note_spanning_several_lines_is_joined() {
		let src = "// TRANSLATORS: first half\n// second half\nt(\"Done\")";
		let got = extract_t_strings(src);
		assert_eq!(got[0].1.as_deref(), Some("TRANSLATORS: first half\nsecond half"));
	}

	#[test]
	fn nt_calls_pick_up_their_note_too() {
		let src = "// TRANSLATORS: {} is the match count\nnt(\"{} result\", \"{} results\", n)";
		let got = extract_nt_strings(src);
		assert_eq!(got[0].2.as_deref(), Some("TRANSLATORS: {} is the match count"));
	}
	/// A pot holding one Rust-owned entry and one `kt` entry, both without notes.
	fn pot_with_a_kt_entry() -> String {
		String::from("msgid \"\"\nmsgstr \"\"\n\nmsgid \"Rust\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Cancel\"\nmsgstr \"\"\n")
	}

	// The msgid landed in the pot before anyone wrote a note for it, and only this refresh can
	// carry the note across afterwards: the appending path skips msgids the pot already has.
	#[test]
	fn a_note_is_backfilled_onto_an_entry_already_in_the_pot() {
		let notes = HashMap::from([("Cancel", Some("TRANSLATORS: dismisses the sheet"))]);
		let got = refresh_foreign_comments(&pot_with_a_kt_entry(), &notes, "kt");
		assert!(got.contains("#. TRANSLATORS: dismisses the sheet\n#: kt\nmsgid \"Cancel\""));
	}

	#[test]
	fn a_reworded_note_replaces_the_old_one() {
		let before = pot_with_a_kt_entry().replace("#: kt", "#. TRANSLATORS: old wording\n#: kt");
		let notes = HashMap::from([("Cancel", Some("TRANSLATORS: new wording"))]);
		let got = refresh_foreign_comments(&before, &notes, "kt");
		assert!(got.contains("TRANSLATORS: new wording"));
		assert!(!got.contains("TRANSLATORS: old wording"));
	}

	#[test]
	fn a_deleted_note_is_removed_from_the_entry() {
		let before = pot_with_a_kt_entry().replace("#: kt", "#. TRANSLATORS: gone now\n#: kt");
		let notes = HashMap::from([("Cancel", None)]);
		let got = refresh_foreign_comments(&before, &notes, "kt");
		assert!(!got.contains("#."));
		assert!(got.contains("#: kt\nmsgid \"Cancel\""));
	}

	// Entries xgettext owns carry no `#:` at all, and another scan's carry a different one.
	#[test]
	fn entries_this_scan_does_not_own_are_left_alone() {
		let before = pot_with_a_kt_entry();
		let notes = HashMap::from([("Rust", Some("TRANSLATORS: not yours"))]);
		assert_eq!(refresh_foreign_comments(&before, &notes, "kt"), before);
		let swift_notes = HashMap::from([("Cancel", Some("TRANSLATORS: not yours"))]);
		assert_eq!(refresh_foreign_comments(&before, &swift_notes, "swift"), before);
	}

	// `lines()` drops it, and on the last block that is the file's own trailing newline.
	#[test]
	fn the_files_trailing_newline_survives_a_refresh() {
		let notes = HashMap::from([("Cancel", Some("TRANSLATORS: dismisses the sheet"))]);
		let got = refresh_foreign_comments(&pot_with_a_kt_entry(), &notes, "kt");
		assert!(got.ends_with('\n'));
	}

	// Rewriting a block that needs no change would churn the pot and bump POT-Creation-Date.
	#[test]
	fn a_pot_that_is_already_current_is_returned_untouched() {
		let before = pot_with_a_kt_entry().replace("#: kt", "#. TRANSLATORS: same as ever\n#: kt");
		let notes = HashMap::from([("Cancel", Some("TRANSLATORS: same as ever"))]);
		assert_eq!(refresh_foreign_comments(&before, &notes, "kt"), before);
	}
}
