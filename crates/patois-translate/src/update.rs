//! Updating a translated readme section whose English changed, by keeping the lines that still translate it and translating only the rest.
//!
//! Asking the model to "keep what did not change" was not enough: one added line in Paperback's shortcuts section came back with ten untouched Dutch lines reworded, undoing what its translator had settled on. Here the model only says, line by line, which existing line still fits, and the section is assembled from those lines byte for byte, so an unchanged line cannot drift.

use crate::markdown::{restore_code_spans, structure_mismatch};

/// The lines of `markdown` that carry text, numbered from 1, as the model is shown them.
pub fn numbered(markdown: &str) -> String {
	text_lines(markdown).enumerate().map(|(i, line)| format!("{}: {line}", i + 1)).collect::<Vec<_>>().join("\n")
}

fn text_lines(markdown: &str) -> impl Iterator<Item = &str> {
	markdown.lines().filter(|line| !line.trim().is_empty())
}

/// The updated section: `english` with each text line replaced by its answer, `=N` taking line N of `existing` as it is. An error says what was wrong with `answer`, for the retry.
pub fn assemble(english: &str, existing: &str, answer: &str) -> Result<String, String> {
	let existing: Vec<&str> = text_lines(existing).collect();
	let answers: Vec<&str> = answer.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
	let wanted = text_lines(english).count();
	if answers.len() != wanted {
		return Err(format!("{wanted} English lines were numbered and {} answer lines came back", answers.len()));
	}
	let mut answers = answers.into_iter();
	let mut out: Vec<String> = Vec::new();
	for line in english.lines() {
		if line.trim().is_empty() {
			out.push(String::new());
			continue;
		}
		let Some(answer) = answers.next() else { unreachable!("the counts were checked") };
		match kept_line(answer) {
			Some(n) => match n.checked_sub(1).and_then(|i| existing.get(i)) {
				Some(kept) => out.push((*kept).to_string()),
				None => return Err(format!("={n} names a line the existing translation does not have")),
			},
			None => out.push(restore_code_spans(line, answer)),
		}
	}
	let section = out.join("\n");
	match structure_mismatch(english, &section) {
		Some(why) => Err(why),
		None => Ok(section),
	}
}

/// N for an answer of `=N`.
fn kept_line(answer: &str) -> Option<usize> {
	answer.strip_prefix('=').and_then(|n| n.trim().parse().ok())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Paperback's case: one line added to a list, every other line still translated by the existing text.
	const ENGLISH: &str = "### Extra keys\n\n* `Delete` on the tab control: Close the selected tab.\n* `Ctrl+1` through `Ctrl+9`: Go to the first nine open documents.\n* `Enter` in the document: Follow a link.";
	const DUTCH: &str = "### Aanvullende toetsen\n\n* `Delete` op het tabbladelement: Het geselecteerde tabblad sluiten.\n* `Enter` in het document: Een link volgen.";

	#[test]
	fn kept_lines_come_through_exactly_and_only_the_new_one_is_translated() {
		let answer = "=1\n=2\n* `Ctrl+1` t/m `Ctrl+9`: Naar de eerste negen geopende documenten gaan.\n=3";
		let updated = assemble(ENGLISH, DUTCH, answer).unwrap();
		assert_eq!(
			updated,
			"### Aanvullende toetsen\n\n* `Delete` op het tabbladelement: Het geselecteerde tabblad sluiten.\n* `Ctrl+1` t/m `Ctrl+9`: Naar de eerste negen geopende documenten gaan.\n* `Enter` in het document: Een link volgen."
		);
	}

	#[test]
	fn the_lines_are_numbered_from_one_without_the_blank_ones() {
		assert_eq!(
			numbered(DUTCH),
			"1: ### Aanvullende toetsen\n2: * `Delete` op het tabbladelement: Het geselecteerde tabblad sluiten.\n3: * `Enter` in het document: Een link volgen."
		);
	}

	#[test]
	fn a_new_line_gets_the_english_code_spans_back() {
		let answer = "=1\n=2\n* `Ctrl+1` t/m `Strg+9`: Naar de eerste negen geopende documenten gaan.\n=3";
		assert!(assemble(ENGLISH, DUTCH, answer).unwrap().contains("`Ctrl+1` t/m `Ctrl+9`"));
	}

	#[test]
	fn an_answer_with_a_line_missing_is_rejected() {
		assert!(assemble(ENGLISH, DUTCH, "=1\n=2\n=3").is_err());
	}

	#[test]
	fn a_line_the_translation_does_not_have_is_rejected() {
		assert!(assemble(ENGLISH, DUTCH, "=1\n=2\n=9\n=3").is_err());
		assert!(assemble(ENGLISH, DUTCH, "=0\n=2\n=2\n=3").is_err());
	}

	/// A kept line in the wrong place shows up as a list item where the heading belongs.
	#[test]
	fn an_answer_that_breaks_the_outline_is_rejected() {
		assert!(assemble(ENGLISH, DUTCH, "=2\n=2\n=3\n=3").is_err());
	}
}
