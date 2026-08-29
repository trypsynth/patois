//! Scanning non-Rust sources (Swift, Kotlin) for `t()`/`nt()` calls and appending what they
//! turn up to an existing `.pot`. A small hand-written scanner rather than another `xgettext`
//! invocation, since those languages need no sanitizing and the call shapes are simple.

use std::{
	collections::HashSet,
	error, fs,
	path::{Path, PathBuf},
};

use crate::entries::{PotEntry, collect_pot_msgids, pot_entry_block};

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
	let mut new_entries: Vec<PotEntry> = Vec::new();
	let mut seen_in_scan: HashSet<String> = HashSet::new();
	for file in &files {
		let content = fs::read_to_string(file)?;
		for s in extract_t_strings(&content) {
			if seen_in_scan.insert(s.clone()) {
				new_entries.push(PotEntry { msgid: s, msgid_plural: None });
			}
		}
		for (singular, plural) in extract_nt_strings(&content) {
			if seen_in_scan.insert(singular.clone()) {
				new_entries.push(PotEntry { msgid: singular, msgid_plural: Some(plural) });
			}
		}
	}
	if new_entries.is_empty() {
		return Ok(());
	}

	// Read the existing pot and collect msgids already present.
	let existing = fs::read_to_string(pot_file)?;
	let existing_ids = collect_pot_msgids(&existing);

	// Append only truly new entries.
	let mut additions = String::new();
	for entry in &new_entries {
		if !existing_ids.contains(&entry.msgid) {
			additions.push_str(&pot_entry_block(entry));
		}
	}
	if !additions.is_empty() {
		let content = format!("{existing}{additions}");
		fs::write(pot_file, content)?;
	}
	Ok(())
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

/// Extract every `t("literal")` value from `content`.
///
/// Ignores `t(` when preceded by an alphanumeric character or underscore (e.g.
/// `stateDescription`).
fn extract_t_strings(content: &str) -> Vec<String> {
	let chars: Vec<char> = content.chars().collect();
	let n = chars.len();
	let mut out: Vec<String> = Vec::new();
	let mut i = 0;
	while i < n {
		if chars[i] == 't' && i + 1 < n && chars[i + 1] == '(' {
			let preceded_by_ident = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
			if !preceded_by_ident && let Some((s, next)) = read_string_literal(&chars, i + 2) {
				if !s.is_empty() {
					out.push(s);
				}
				i = next;
				continue;
			}
		}
		i += 1;
	}
	out
}

/// Extract every `nt("singular", "plural", ...)` call's singular/plural literal pair from
/// `content`. Ignores the third (count) argument entirely; only the first two string literals
/// matter. Same identifier-boundary rule as [`extract_t_strings`].
fn extract_nt_strings(content: &str) -> Vec<(String, String)> {
	let chars: Vec<char> = content.chars().collect();
	let n = chars.len();
	let mut out: Vec<(String, String)> = Vec::new();
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
						out.push((singular, plural));
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

	#[test]
	fn extract_simple() {
		assert_eq!(extract_t_strings(r#"Button(t("Cancel")) { dismiss() }"#), vec!["Cancel"]);
	}

	#[test]
	fn extract_multiple() {
		assert_eq!(extract_t_strings(r#"Text(t("Find")) Text(t("Cancel"))"#), vec!["Find", "Cancel"]);
	}

	#[test]
	fn extract_escaped_quote() {
		assert_eq!(extract_t_strings(r#"t("say \"hi\"")"#), vec!["say \"hi\""]);
	}

	#[test]
	fn extract_backslash_escape() {
		// Swift source on disk: t("Regular expression (\\1 = first capture group)")
		// Two actual backslash chars in the file → decoded to one backslash in the msgid.
		let src = "t(\"Regular expression (\\\\1 = first capture group)\")";
		assert_eq!(extract_t_strings(src), vec!["Regular expression (\\1 = first capture group)"]);
	}

	#[test]
	fn skip_ident_suffix_t() {
		// 't' preceded by 'x' in putText → not a t() call
		let src = r#"putText("bad") t("good")"#;
		let got = extract_t_strings(src);
		assert!(got.contains(&"good".to_string()));
		assert!(!got.contains(&"bad".to_string()));
	}

	#[test]
	fn unicode_passthrough() {
		let src = "t(\"Search\u{2026}\")";
		assert_eq!(extract_t_strings(src), vec!["Search\u{2026}"]);
	}

	#[test]
	fn extract_nt_finds_singular_and_plural() {
		let src = r#"Text(nt("{} result", "{} results", count))"#;
		assert_eq!(extract_nt_strings(src), vec![("{} result".to_string(), "{} results".to_string())]);
	}

	#[test]
	fn extract_nt_ignores_nt_suffix() {
		// 'nt' preceded by 'x' in xnt(...) → not an nt() call.
		let src = r#"xnt("bad", "worse", n) nt("good", "goods", n)"#;
		assert_eq!(extract_nt_strings(src), vec![("good".to_string(), "goods".to_string())]);
	}
}
