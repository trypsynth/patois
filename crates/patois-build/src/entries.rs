//! The text-level primitives shared by everything that reads or writes gettext files:
//! parsing `msgid`/`msgid_plural` entries out of a pot or po, rendering an entry back out,
//! and the escaping rules on both sides of that.

use std::collections::HashSet;

/// A parsed pot/po entry: a msgid, its msgid_plural text if it's a plural entry, the `#:`
/// reference naming what produced it, and the `#.` note left for translators, for entries
/// that carry them.
pub(crate) struct PotEntry {
	pub(crate) msgid: String,
	pub(crate) msgid_plural: Option<String>,
	/// The entry's `#:` line, naming the scan the string came from (e.g. `swift`).
	///
	/// `xgettext` runs with `--no-location`, so nothing extracted from Rust ever carries one.
	/// That is what makes a reference usable as the marker for "this came from a non-Rust
	/// scan", and what lets a regeneration carry those entries across without also
	/// resurrecting Rust msgids that were renamed or deleted in the same edit.
	pub(crate) reference: Option<String>,
	/// The entry's `#.` extracted-comment text: the note the source left for translators.
	///
	/// Stored without the `#. ` prefix, and newline-separated when the note ran to several
	/// lines. `xgettext --add-comments=TRANSLATORS` produces these for Rust; the hand-written
	/// scanner in [`crate::extract`] produces them for the other languages.
	pub(crate) comment: Option<String>,
}

/// Renders a `PotEntry` as the block of lines to append to a pot file (blank msgstr(s), ready
/// for `msgmerge` or a translator to fill in).
///
/// The `#:` line is re-emitted when the entry has one. Dropping it would un-mark a foreign
/// entry the moment it was preserved, so the next regeneration would discard it, the scan that
/// owns it would add it back, and the pot would be rewritten on every other run.
pub(crate) fn pot_entry_block(entry: &PotEntry) -> String {
	// `#.` sits above `#:`, the order xgettext itself writes the two in.
	let comment = entry
		.comment
		.as_ref()
		.map_or_else(String::new, |c| c.lines().map(|line| format!("#. {line}\n")).collect::<String>());
	let reference = entry.reference.as_ref().map_or_else(String::new, |r| format!("#: {r}\n"));
	match &entry.msgid_plural {
		Some(plural) => format!(
			"\n{comment}{reference}msgid \"{}\"\nmsgid_plural \"{}\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n",
			pot_escape(&entry.msgid),
			pot_escape(plural)
		),
		None => format!("\n{comment}{reference}msgid \"{}\"\nmsgstr \"\"\n", pot_escape(&entry.msgid)),
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
	let mut current_reference: Option<String> = None;
	let mut current_comment: Option<String> = None;
	// A `#:` line comes *before* the msgid it belongs to, so it is held here until that msgid
	// line arrives and takes it.
	let mut pending_reference: Option<String> = None;
	let mut pending_comment: Option<String> = None;
	let mut in_msgid = false;
	let mut in_msgid_plural = false;
	let flush = |current_id: &mut String,
	             current_plural: &mut Option<String>,
	             current_reference: &mut Option<String>,
	             current_comment: &mut Option<String>,
	             entries: &mut Vec<PotEntry>| {
		if !current_id.is_empty() || current_plural.is_some() {
			entries.push(PotEntry {
				msgid: std::mem::take(current_id),
				msgid_plural: current_plural.take(),
				reference: current_reference.take(),
				comment: current_comment.take(),
			});
		} else {
			// The header entry (empty msgid) isn't emitted, so anything picked up ahead of it
			// would otherwise leak onto the first real entry.
			*current_reference = None;
			*current_comment = None;
		}
	};
	for line in content.lines() {
		let line = line.trim();
		if let Some(rest) = line.strip_prefix("#.") {
			// A note can run to several `#.` lines; keep them as one newline-joined string.
			let text = rest.trim();
			pending_comment = Some(match pending_comment.take() {
				Some(existing) => format!("{existing}\n{text}"),
				None => text.to_string(),
			});
		} else if let Some(rest) = line.strip_prefix("#:") {
			pending_reference = Some(rest.trim().to_string());
		} else if let Some(rest) = line.strip_prefix("msgid_plural ") {
			current_plural = Some(po_unescape(rest));
			in_msgid = false;
			in_msgid_plural = true;
		} else if let Some(rest) = line.strip_prefix("msgid ") {
			flush(&mut current_id, &mut current_plural, &mut current_reference, &mut current_comment, &mut entries);
			current_id = po_unescape(rest);
			current_reference = pending_reference.take();
			current_comment = pending_comment.take();
			in_msgid = true;
			in_msgid_plural = false;
		} else if line.starts_with("msgstr") {
			// Covers both a plain entry's `msgstr "..."` and a plural entry's `msgstr[N] "..."`.
			flush(&mut current_id, &mut current_plural, &mut current_reference, &mut current_comment, &mut entries);
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
	flush(&mut current_id, &mut current_plural, &mut current_reference, &mut current_comment, &mut entries);
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

	// The `#:` line sits above its msgid, so it has to be held and attached to the entry that
	// follows rather than the one just finished.
	#[test]
	fn collect_pot_entries_attaches_a_reference_to_the_entry_below_it() {
		let pot = "msgid \"\"\nmsgstr \"\"\n\nmsgid \"From Rust\"\nmsgstr \"\"\n\n#: swift\nmsgid \"From Swift\"\nmsgstr \"\"\n";
		let entries = collect_pot_entries_ordered(pot);
		let rust = entries.iter().find(|e| e.msgid == "From Rust").unwrap();
		assert_eq!(rust.reference, None, "the reference belongs to the entry after it, not before");
		let swift = entries.iter().find(|e| e.msgid == "From Swift").unwrap();
		assert_eq!(swift.reference.as_deref(), Some("swift"));
	}

	// The header's empty msgid is parsed but never emitted as an entry, so a reference read
	// before it has nowhere to go and must not drift onto the first real string.
	#[test]
	fn a_reference_above_the_header_does_not_leak_onto_the_first_entry() {
		let pot = "#: swift\nmsgid \"\"\nmsgstr \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n";
		let entries = collect_pot_entries_ordered(pot);
		let cancel = entries.iter().find(|e| e.msgid == "Cancel").unwrap();
		assert_eq!(cancel.reference, None);
	}

	#[test]
	fn pot_entry_block_round_trips_a_reference() {
		let entry = PotEntry {
			msgid: "Continue".to_string(),
			msgid_plural: None,
			reference: Some("kt".to_string()),
			comment: None,
		};
		let rendered = pot_entry_block(&entry);
		assert!(rendered.contains("#: kt\nmsgid \"Continue\""));
		let parsed = collect_pot_entries_ordered(&rendered);
		assert_eq!(parsed[0].reference.as_deref(), Some("kt"));
	}
	// A note has to survive a preserve-and-rewrite cycle: `preserve_foreign_entries` parses the
	// old pot and re-renders what it finds, so anything the parser drops is gone for good.
	#[test]
	fn pot_entry_block_round_trips_a_comment() {
		let entry = PotEntry {
			msgid: "Continue".to_string(),
			msgid_plural: None,
			reference: Some("kt".to_string()),
			comment: Some("TRANSLATORS: on the last onboarding page".to_string()),
		};
		let rendered = pot_entry_block(&entry);
		assert!(rendered.contains("#. TRANSLATORS: on the last onboarding page\n#: kt\nmsgid"));
		let parsed = collect_pot_entries_ordered(&rendered);
		assert_eq!(parsed[0].comment.as_deref(), Some("TRANSLATORS: on the last onboarding page"));
	}

	#[test]
	fn a_comment_spanning_several_lines_round_trips() {
		let entry = PotEntry {
			msgid: "Continue".to_string(),
			msgid_plural: None,
			reference: None,
			comment: Some("TRANSLATORS: first half\nsecond half".to_string()),
		};
		let rendered = pot_entry_block(&entry);
		assert!(rendered.contains("#. TRANSLATORS: first half\n#. second half\n"));
		let parsed = collect_pot_entries_ordered(&rendered);
		assert_eq!(parsed[0].comment.as_deref(), Some("TRANSLATORS: first half\nsecond half"));
	}

	// Same trap as the reference: a `#.` read before the header entry has nowhere to go.
	#[test]
	fn a_comment_above_the_header_does_not_leak_onto_the_first_entry() {
		let pot = "#. TRANSLATORS: stray\nmsgid \"\"\nmsgstr \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n";
		let entries = collect_pot_entries_ordered(pot);
		let cancel = entries.iter().find(|e| e.msgid == "Cancel").unwrap();
		assert_eq!(cancel.comment, None);
	}
}
