//! The text-level primitives shared by everything that reads or writes gettext files:
//! parsing `msgid`/`msgid_plural` entries out of a pot or po, rendering an entry back out,
//! and the escaping rules on both sides of that.

use std::collections::HashSet;

/// A parsed pot/po entry: a msgid, and its msgid_plural text if it's a plural entry.
pub(crate) struct PotEntry {
	pub(crate) msgid: String,
	pub(crate) msgid_plural: Option<String>,
}

/// Renders a `PotEntry` as the block of lines to append to a pot file (blank msgstr(s), ready
/// for `msgmerge` or a translator to fill in).
pub(crate) fn pot_entry_block(entry: &PotEntry) -> String {
	match &entry.msgid_plural {
		Some(plural) => format!(
			"\nmsgid \"{}\"\nmsgid_plural \"{}\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n",
			pot_escape(&entry.msgid),
			pot_escape(plural)
		),
		None => format!("\nmsgid \"{}\"\nmsgstr \"\"\n", pot_escape(&entry.msgid)),
	}
}

/// Parse msgid values already present in a pot/po file.
pub(crate) fn collect_pot_msgids(content: &str) -> HashSet<String> {
	collect_pot_entries_ordered(content).into_iter().map(|e| e.msgid).collect()
}

/// Parse msgid/msgid_plural entries already present in a pot/po file, preserving first-seen
/// order.
pub(crate) fn collect_pot_entries_ordered(content: &str) -> Vec<PotEntry> {
	let mut entries: Vec<PotEntry> = Vec::new();
	let mut current_id = String::new();
	let mut current_plural: Option<String> = None;
	let mut in_msgid = false;
	let mut in_msgid_plural = false;
	let flush = |current_id: &mut String, current_plural: &mut Option<String>, entries: &mut Vec<PotEntry>| {
		if !current_id.is_empty() || current_plural.is_some() {
			entries.push(PotEntry { msgid: std::mem::take(current_id), msgid_plural: current_plural.take() });
		}
	};
	for line in content.lines() {
		let line = line.trim();
		if let Some(rest) = line.strip_prefix("msgid_plural ") {
			current_plural = Some(po_unescape(rest));
			in_msgid = false;
			in_msgid_plural = true;
		} else if let Some(rest) = line.strip_prefix("msgid ") {
			flush(&mut current_id, &mut current_plural, &mut entries);
			current_id = po_unescape(rest);
			in_msgid = true;
			in_msgid_plural = false;
		} else if line.starts_with("msgstr") {
			// Covers both a plain entry's `msgstr "..."` and a plural entry's `msgstr[N] "..."`.
			flush(&mut current_id, &mut current_plural, &mut entries);
			in_msgid = false;
			in_msgid_plural = false;
		} else if line.starts_with('"') {
			if in_msgid_plural {
				if let Some(p) = current_plural.as_mut() {
					p.push_str(&po_unescape(line));
				}
			} else if in_msgid {
				current_id.push_str(&po_unescape(line));
			}
		}
	}
	flush(&mut current_id, &mut current_plural, &mut entries);
	entries
}

/// Escape a string for use as a pot msgid value (between the outer double-quotes).
pub(crate) fn pot_escape(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for c in s.chars() {
		match c {
			'"' => out.push_str("\\\""),
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			'\t' => out.push_str("\\t"),
			c => out.push(c),
		}
	}
	out
}

pub(crate) fn po_unescape(s: &str) -> String {
	let s = s.trim();
	if s.len() < 2 || !s.starts_with('"') || !s.ends_with('"') {
		return String::new();
	}
	let inner = &s[1..s.len() - 1];
	let mut out = String::with_capacity(inner.len());
	let mut chars = inner.chars();
	while let Some(c) = chars.next() {
		if c == '\\' {
			match chars.next() {
				Some('n') => out.push('\n'),
				Some('t') => out.push('\t'),
				Some('"') => out.push('"'),
				Some('\\') => out.push('\\'),
				Some(c) => {
					out.push('\\');
					out.push(c);
				}
				None => out.push('\\'),
			}
		} else {
			out.push(c);
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pot_escape_roundtrip() {
		let s = "say \"hi\" and \\bye\nnewline";
		let escaped = pot_escape(s);
		assert_eq!(escaped, r#"say \"hi\" and \\bye\nnewline"#);
		assert_eq!(po_unescape(&format!("\"{escaped}\"")), s);
	}

	#[test]
	fn collect_msgids_finds_existing() {
		let pot = "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n\nmsgid \"OK\"\nmsgstr \"OK\"\n";
		let ids = collect_pot_msgids(pot);
		assert!(ids.contains("Cancel"));
		assert!(ids.contains("OK"));
	}

	#[test]
	fn collect_pot_entries_captures_plural_shape() {
		let pot = "msgid \"\"\nmsgstr \"\"\n\nmsgid \"{} result\"\nmsgid_plural \"{} results\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n";
		let entries = collect_pot_entries_ordered(pot);
		let plural_entry = entries.iter().find(|e| e.msgid == "{} result").unwrap();
		assert_eq!(plural_entry.msgid_plural.as_deref(), Some("{} results"));
		let singular_entry = entries.iter().find(|e| e.msgid == "Cancel").unwrap();
		assert_eq!(singular_entry.msgid_plural, None);
	}
}
