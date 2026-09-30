//! Checks that catch translation mistakes nothing else reports.
//!
//! - [`concatenated_translation_calls`] and [`check_sources`] find `t("a " + "b")`. The
//!   extractor only takes the first literal, so the `.pot` gets a message that stops mid-sentence
//!   while the code looks up the whole one. They never match, and the string stays in English in
//!   every language.
//! - [`stale_shortcuts`] and [`check_catalogs`] find translations that end in a keyboard
//!   shortcut their source doesn't have. `msgmerge` copies these in when a menu label loses its
//!   shortcut, and an app that compiles fuzzy entries shows them.
//!
//! Run both before generating the `.pot`, and as tests, so CI catches new mistakes.

use std::{
	fmt::Write as _,
	fs,
	path::{Path, PathBuf},
};

/// The 1-based line of each `t(...)` or `nt(...)` call in `source` that joins string literals
/// with `+`.
#[must_use]
pub fn concatenated_translation_calls(source: &str) -> Vec<usize> {
	let chars: Vec<char> = source.chars().collect();
	let mut hits = Vec::new();
	let mut i = 0;
	while i < chars.len() {
		let Some(open) = call_argument_start(&chars, i) else {
			i += 1;
			continue;
		};
		if let Some(end) = joins_literals(&chars, open) {
			hits.push(line_of(&chars, i) + 1);
			i = end;
			continue;
		}
		i += 1;
	}
	hits.sort_unstable();
	hits.dedup();
	hits
}

/// The index just past the `(` when `t(` or `nt(` starts at `i` as a call, not as the end of a
/// longer name such as `format(`.
fn call_argument_start(chars: &[char], i: usize) -> Option<usize> {
	let name_len = if chars.get(i) == Some(&'n') && chars.get(i + 1) == Some(&'t') {
		2
	} else if chars.get(i) == Some(&'t') {
		1
	} else {
		return None;
	};
	if chars.get(i + name_len) != Some(&'(') {
		return None;
	}
	if i > 0 {
		let before = chars[i - 1];
		if before.is_alphanumeric() || before == '_' || before == '.' {
			return None;
		}
	}
	Some(i + name_len + 1)
}

/// Scans one call's arguments from `start`, returning the index past its closing paren when a
/// string literal is followed by `+` and another string literal.
fn joins_literals(chars: &[char], start: usize) -> Option<usize> {
	let mut i = start;
	let mut depth = 1usize;
	let mut just_closed_a_literal = false;
	while i < chars.len() {
		match chars[i] {
			'"' => {
				i = skip_literal(chars, i)?;
				just_closed_a_literal = true;
				continue;
			}
			'(' => {
				depth += 1;
				just_closed_a_literal = false;
			}
			')' => {
				depth -= 1;
				if depth == 0 {
					return None;
				}
				just_closed_a_literal = false;
			}
			'+' if just_closed_a_literal => {
				// Only literal + literal breaks extraction. A literal joined to a variable is
				// already unextractable, which is a different mistake.
				let mut j = i + 1;
				while j < chars.len() && chars[j].is_whitespace() {
					j += 1;
				}
				if chars.get(j) == Some(&'"') {
					return Some(skip_to_call_end(chars, i, depth));
				}
				just_closed_a_literal = false;
			}
			c if c.is_whitespace() => {}
			_ => just_closed_a_literal = false,
		}
		i += 1;
	}
	None
}

/// The index just past the call's closing paren, so the scan resumes after it.
const fn skip_to_call_end(chars: &[char], i: usize, mut depth: usize) -> usize {
	let mut j = i;
	while j < chars.len() && depth > 0 {
		match chars[j] {
			'"' => {
				let Some(next) = skip_literal(chars, j) else {
					return chars.len();
				};
				j = next;
				continue;
			}
			'(' => depth += 1,
			')' => depth -= 1,
			_ => {}
		}
		j += 1;
	}
	j
}

/// The index just past the closing quote of the literal that opens at `i`, with escapes.
const fn skip_literal(chars: &[char], i: usize) -> Option<usize> {
	let mut j = i + 1;
	while j < chars.len() {
		match chars[j] {
			'\\' => j += 2,
			'"' => return Some(j + 1),
			_ => j += 1,
		}
	}
	None
}

fn line_of(chars: &[char], index: usize) -> usize {
	chars[..index].iter().filter(|&&c| c == '\n').count()
}

/// The Rust, Kotlin, and Swift files under `dir`, skipping build output.
fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
	let Ok(entries) = fs::read_dir(dir) else {
		return;
	};
	for entry in entries.flatten() {
		let path = entry.path();
		let name = entry.file_name();
		if path.is_dir() {
			if name != "target" && name != "build" && name != "generated" {
				source_files(&path, out);
			}
		} else if path
			.extension()
			.and_then(|e| e.to_str())
			.is_some_and(|e| ["rs", "kt", "swift"].iter().any(|kind| e.eq_ignore_ascii_case(kind)))
		{
			out.push(path);
		}
	}
}

/// Checks every Rust, Kotlin, and Swift file under `dirs` for translation calls that join
/// string literals.
///
/// # Errors
///
/// Returns the `file:line` of each such call. Also returns an error when `dirs` hold no source
/// files at all, since a check that finds nothing to check is usually looking in the wrong
/// place.
pub fn check_sources(dirs: &[&Path]) -> Result<(), String> {
	let mut files = Vec::new();
	for dir in dirs {
		source_files(dir, &mut files);
	}
	if files.is_empty() {
		return Err("found no source files to check".to_string());
	}
	let mut offenders = Vec::new();
	for file in files {
		let Ok(content) = fs::read_to_string(&file) else {
			continue;
		};
		for line in concatenated_translation_calls(&content) {
			offenders.push(format!("{}:{line}", file.display()));
		}
	}
	if offenders.is_empty() {
		return Ok(());
	}
	Err(format!(
		"these translation calls join string literals, so only the first one reaches the .pot and the message can't \
		 be translated. Write each message as one literal:\n  {}",
		offenders.join("\n  ")
	))
}

/// A translation that ends in a keyboard shortcut its source doesn't have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleShortcut {
	pub msgid: String,
	pub msgstr: String,
}

/// The text after the separator between a label and its shortcut: a tab, or the run of spaces a
/// machine translator made of one.
fn shortcut_tail(text: &str) -> Option<&str> {
	let tab = text.find("\\t").map(|at| &text[at + 2..]);
	let padded = text.split("   ").nth(1);
	tab.or(padded).map(str::trim)
}

/// Each entry in `catalog` whose translation ends in a shortcut its msgid doesn't have.
///
/// A msgid that has a shortcut of its own, such as an old-style menu label, can have one in its
/// translation too.
#[must_use]
pub fn stale_shortcuts(catalog: &str) -> Vec<StaleShortcut> {
	let mut found = Vec::new();
	for entry in catalog.split("\n\n") {
		// Obsolete entries are commented out with `#~` and never compiled.
		if entry.starts_with("#~") {
			continue;
		}
		let (Some(msgid), Some(msgstr)) = (field(entry, "msgid"), field(entry, "msgstr")) else {
			continue;
		};
		if !msgid.is_empty()
			&& !msgstr.is_empty()
			&& shortcut_tail(&msgstr).is_some()
			&& shortcut_tail(&msgid).is_none()
		{
			found.push(StaleShortcut { msgid, msgstr });
		}
	}
	found
}

/// One `msgid` or `msgstr` of an entry, with its continuation lines joined.
fn field(entry: &str, name: &str) -> Option<String> {
	let mut lines = entry.lines().skip_while(|line| !line.starts_with(&format!("{name} \"")));
	let mut out = quoted(lines.next()?)?.to_string();
	for line in lines {
		let Some(more) = line.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) else {
			break;
		};
		out.push_str(more);
	}
	Some(out)
}

fn quoted(line: &str) -> Option<&str> {
	let start = line.find('"')? + 1;
	line.get(start..line.len().checked_sub(1)?)
}

/// Checks every `.po` file in `po_dir` for stale shortcuts, and reports them all at once.
///
/// # Errors
///
/// Returns each stale translation, with its file.
pub fn check_catalogs(po_dir: &Path) -> Result<(), String> {
	let mut report = String::new();
	let Ok(entries) = fs::read_dir(po_dir) else {
		return Ok(());
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if path.extension().and_then(|e| e.to_str()) != Some("po") {
			continue;
		}
		let Ok(text) = fs::read_to_string(&path) else {
			continue;
		};
		for stale in stale_shortcuts(&text) {
			let _ = writeln!(report, "{}: {:?} -> {:?}", path.display(), stale.msgid, stale.msgstr);
		}
	}
	if report.is_empty() {
		Ok(())
	} else {
		Err(format!("translations with a shortcut their source doesn't have:\n{report}"))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_plain_call_is_fine() {
		assert!(concatenated_translation_calls(r#"t("Open Document")"#).is_empty());
	}

	#[test]
	fn a_call_joining_two_literals_is_reported() {
		assert_eq!(concatenated_translation_calls(r#"t("first " + "second")"#), vec![1]);
	}

	#[test]
	fn the_join_is_found_across_lines() {
		let src = "Text(\n\ttext = t(\n\t\t\"first \" +\n\t\t\t\"second\"\n\t),\n)";
		assert_eq!(concatenated_translation_calls(src), vec![2]);
	}

	#[test]
	fn plural_calls_are_checked_too() {
		assert_eq!(concatenated_translation_calls(r#"nt("a " + "b", "c", n)"#), vec![1]);
	}

	#[test]
	fn concatenation_outside_the_call_is_ignored() {
		assert!(concatenated_translation_calls(r#"val s = t("Open") + " " + name"#).is_empty());
	}

	#[test]
	fn a_longer_name_ending_in_t_is_not_a_translation_call() {
		assert!(concatenated_translation_calls(r#"format("a " + "b")"#).is_empty());
		assert!(concatenated_translation_calls(r#"obj.t("a " + "b")"#).is_empty());
	}

	#[test]
	fn a_plus_inside_a_literal_is_not_a_join() {
		assert!(concatenated_translation_calls("t(\"one \\\" + \\\" two\")").is_empty());
	}

	#[test]
	fn a_copied_menu_shortcut_is_reported() {
		let po = "msgid \"Reopen Last Closed\"\nmsgstr \"Rouvrir & Dernière fermeture    Ctrl+Maj+T\"\n";
		assert_eq!(
			stale_shortcuts(po),
			vec![StaleShortcut {
				msgid: "Reopen Last Closed".to_string(),
				msgstr: "Rouvrir & Dernière fermeture    Ctrl+Maj+T".to_string(),
			}]
		);
	}

	#[test]
	fn a_plain_translation_is_fine() {
		assert!(stale_shortcuts("msgid \"Close All\"\nmsgstr \"Tout fermer\"\n").is_empty());
	}

	#[test]
	fn a_shortcut_on_both_sides_is_fine() {
		let po = "msgid \"Close &All\\tCtrl+Shift+F4\"\nmsgstr \"Tout &fermer\\tCtrl+Shift+F4\"\n";
		assert!(stale_shortcuts(po).is_empty());
	}

	#[test]
	fn an_obsolete_entry_is_left_alone() {
		let po = "#~ msgid \"Go &Back\\tCtrl+[\"\n#~ msgstr \"Aller & Retour    Ctrl+[\"\n";
		assert!(stale_shortcuts(po).is_empty());
	}
}
