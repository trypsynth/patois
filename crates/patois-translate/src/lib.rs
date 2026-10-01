//! Machine translation of gettext catalogs and Markdown readmes through the Claude API, for apps
//! that use [patois](https://docs.rs/patois).
//!
//! [`translate`] syncs each `.po` file with the `.pot` through `msgmerge`, then asks Claude to
//! fill the empty and fuzzy entries. Each request carries the `TRANSLATORS:` comment of each
//! string, and each answer is checked with [`patois_build::check`], so a translation that drops
//! a placeholder, an accelerator, or a shortcut is never written. It can also translate a readme
//! into `readme-<lang>.md` files, one section at a time.
//!
//! Call it from an xtask after regenerating the `.pot`. It needs `msgmerge` from gettext, and an
//! `ANTHROPIC_API_KEY` unless it's a dry run. Set `PATOIS_TRANSLATE_MODEL` to use a model other
//! than the default.
//!
//! Two optional files in the `.po` folder change what it does:
//!
//! - `human-maintained-locales.txt`: locales that people translate, one per line, which it
//!   skips.
//! - `style/<lang>.md`: conventions for one language, added to its instructions.

use std::{
	collections::{HashMap, HashSet},
	env,
	error::Error,
	fs,
	path::{Path, PathBuf},
	process::{self, Command},
};

use patois_build::{
	check::is_damaged,
	po::{PoDocument, Translation},
};

mod claude;
mod markdown;
mod prompts;
mod readme;
mod terms;

/// What the model is told about the app.
#[derive(Debug, Clone, Default)]
pub struct App {
	/// The app's name, such as `Paperback`.
	pub name: String,
	/// What the app is, to complete "the user interface of Paperback, …", such as `a desktop
	/// ebook and document reader used heavily with screen readers`.
	pub description: String,
	/// Names to leave untranslated besides the app's own, such as `EPUB` and `PDF`. File
	/// extensions and URLs are always left alone.
	pub proper_nouns: Vec<String>,
}

/// Where a project keeps its translations. The paths other than `root` are relative to it.
#[derive(Debug, Clone)]
pub struct Project<'a> {
	pub root: &'a Path,
	/// The folder with the `.po` files, such as `po`.
	pub po_dir: &'a Path,
	/// The template, such as `po/myapp.pot`.
	pub pot: &'a Path,
	/// The English readme to translate into `<stem>-<lang>.md` beside it, such as
	/// `doc/readme.md`.
	pub readme: Option<&'a Path>,
	pub app: App,
}

/// What a run does.
#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools, reason = "these are independent switches")]
pub struct Options {
	/// Reports what would be translated, without calling the API or changing a file.
	pub dry_run: bool,
	/// Also translates again the entries whose translation dropped or added a placeholder, an
	/// accelerator, or a shortcut.
	pub repair: bool,
	/// Also translates again the machine translations that are shared with a string that says
	/// something different, which `msgmerge` copies in. Meant to run once, not on every run.
	pub repair_copies: bool,
}

/// Translates every `.po` file of `project` that isn't human-maintained, then its readme.
///
/// A file is written only when its content changed, not when `msgmerge` only updated the
/// timestamps in its header.
///
/// # Errors
///
/// Returns an error when `ANTHROPIC_API_KEY` isn't set outside a dry run, when a file can't be
/// read or written, when an API request fails, or when a readme section comes back incomplete.
pub fn translate(project: &Project, options: Options) -> Result<(), Box<dyn Error>> {
	let client = if options.dry_run {
		None
	} else {
		let api_key = env::var("ANTHROPIC_API_KEY").map_err(|_| "ANTHROPIC_API_KEY environment variable is not set")?;
		// A secret that exists but is empty passes `env::var`, then fails at the API with an
		// error that reads like a wrong key rather than a missing one.
		if api_key.trim().is_empty() {
			return Err("ANTHROPIC_API_KEY is set but empty".into());
		}
		let client = claude::ClaudeClient::new(api_key, project.app.clone());
		println!("translating with {}", client.model());
		Some(client)
	};
	let pot_path = project.root.join(project.pot);
	let context = translator_comments(&fs::read_to_string(&pot_path)?);
	let po_dir = project.root.join(project.po_dir);
	let mut po_files: Vec<PathBuf> = fs::read_dir(&po_dir)?
		.filter_map(Result::ok)
		.map(|e| e.path())
		.filter(|p| p.extension().and_then(|e| e.to_str()) == Some("po"))
		.collect();
	po_files.sort();
	let human_maintained = load_human_maintained_locales(project);
	let mut auto_langs = Vec::new();
	for po_path in &po_files {
		let lang = po_path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
		if human_maintained.contains(lang) {
			println!("{lang}: human-maintained, skipping");
			continue;
		}
		auto_langs.push(lang.to_string());
		translate_one(project, po_path, &pot_path, options, client.as_ref(), &context)?;
	}
	readme::sync_readmes(project, &auto_langs, client.as_ref(), options.dry_run)
}

/// The `TRANSLATORS:` note of each msgid in `pot` that has one, which is the difference between
/// translating "Open" as a verb and as an adjective.
fn translator_comments(pot: &str) -> HashMap<String, String> {
	PoDocument::parse(pot)
		.entries
		.into_iter()
		.filter_map(|entry| Some((entry.msgid, entry.comment?)))
		.filter(|(msgid, _)| !msgid.is_empty())
		.collect()
}

fn load_human_maintained_locales(project: &Project) -> HashSet<String> {
	let path = project.root.join(project.po_dir).join("human-maintained-locales.txt");
	fs::read_to_string(path).map(|content| parse_human_maintained_locales(&content)).unwrap_or_default()
}

fn parse_human_maintained_locales(content: &str) -> HashSet<String> {
	content
		.lines()
		.map(|line| line.split('#').next().unwrap_or("").trim())
		.filter(|line| !line.is_empty())
		.map(str::to_string)
		.collect()
}

/// The conventions a locale's translators wrote for the model in `style/<lang>.md`, or `None`
/// when there's no such file or it's blank.
fn load_style_note(project: &Project, lang: &str) -> Option<String> {
	let path = project.root.join(project.po_dir).join("style").join(format!("{lang}.md"));
	parse_style_note(&fs::read_to_string(path).ok()?)
}

fn parse_style_note(content: &str) -> Option<String> {
	let note = content.trim();
	(!note.is_empty()).then(|| note.to_string())
}

fn style_note_suffix(project: &Project, lang: &str, present: bool) -> String {
	if present {
		format!(
			", with {}",
			project.po_dir.join("style").join(format!("{lang}.md")).to_string_lossy().replace('\\', "/")
		)
	} else {
		String::new()
	}
}

/// What to splice into a document: the entry index and its translated result.
type Applied = Vec<(usize, Translation)>;

/// Translates the ordinary entries, returning what to apply and how many carried a note.
fn translate_singulars(
	client: &claude::ClaudeClient,
	target: &claude::Target,
	candidates: &[(usize, String)],
	context: &HashMap<String, String>,
) -> Result<(Applied, usize), Box<dyn Error>> {
	if candidates.is_empty() {
		return Ok((Vec::new(), 0));
	}
	let phrases: Vec<claude::Phrase> = candidates
		.iter()
		.map(|(_, text)| claude::Phrase { source: text.clone(), context: context.get(text).cloned() })
		.collect();
	let annotated = phrases.iter().filter(|p| p.context.is_some()).count();
	let results = client.translate_phrases(&phrases, target)?;
	let applied = candidates
		.iter()
		.map(|(i, _)| *i)
		.zip(results)
		.filter_map(|(i, result)| result.map(|text| (i, Translation::Singular(text))))
		.collect();
	Ok((applied, annotated))
}

/// Translates the plural entries, asking for `nplurals` forms of each.
fn translate_plurals(
	client: &claude::ClaudeClient,
	target: &claude::Target,
	candidates: &[(usize, String, String)],
	context: &HashMap<String, String>,
	nplurals: usize,
	rule: &str,
) -> Result<Applied, Box<dyn Error>> {
	if candidates.is_empty() {
		return Ok(Vec::new());
	}
	let phrases: Vec<claude::PluralPhrase> = candidates
		.iter()
		.map(|(_, singular, plural)| claude::PluralPhrase {
			singular: singular.clone(),
			plural: plural.clone(),
			// The note is above the singular, which is the msgid.
			context: context.get(singular).cloned(),
		})
		.collect();
	let results = client.translate_plurals(&phrases, target, nplurals, rule)?;
	Ok(candidates
		.iter()
		.map(|(i, _, _)| *i)
		.zip(results)
		.filter_map(|(i, result)| result.map(|forms| (i, Translation::Plural(forms))))
		.collect())
}

/// The `nplurals` count and the raw plural rule from a `.po` file's `Plural-Forms` header, so
/// the model gets the language's actual rule.
fn plural_forms(content: &str) -> Option<(usize, String)> {
	let nplurals = PoDocument::parse(content).nplurals()?;
	let line = content.lines().map(str::trim).find(|l| l.contains("Plural-Forms:"))?;
	let rule = line.trim_start_matches('"').trim_end_matches("\\n\"").trim().to_string();
	Some((nplurals, rule))
}

/// Adds the entries whose translation is damaged to the candidates, returning how many.
///
/// Only entries that fail a mechanical check are added. Translating again on suspicion would
/// change thousands of good entries.
fn add_damaged_entries(
	doc: &PoDocument,
	candidates: &mut Vec<(usize, String)>,
	plurals: &mut Vec<(usize, String, String)>,
) -> usize {
	let already: HashSet<usize> = candidates.iter().map(|(i, _)| *i).collect();
	let already_plural: HashSet<usize> = plurals.iter().map(|(i, _, _)| *i).collect();
	let mut count = 0;
	for (i, entry) in doc.entries.iter().enumerate() {
		match entry.msgid_plural.as_deref() {
			// The forms are translated together, so one damaged form means all of them.
			Some(plural) => {
				if !already_plural.contains(&i) && entry.msgstr_plural.iter().any(|form| is_damaged(plural, form)) {
					plurals.push((i, entry.msgid.clone(), plural.to_string()));
					count += 1;
				}
			}
			None => {
				if !already.contains(&i) && is_damaged(&entry.msgid, &entry.msgstr) {
					candidates.push((i, entry.msgid.clone()));
					count += 1;
				}
			}
		}
	}
	count
}

/// Adds the fuzzy entries whose translation is shared with an entry whose English says
/// something different, returning how many. These are usually copies `msgmerge` made from a
/// similar string.
///
/// Only fuzzy entries, because a person wrote the others, and a shared translation isn't enough
/// reason to replace their work.
fn add_copied_entries(doc: &PoDocument, candidates: &mut Vec<(usize, String)>) -> usize {
	let already: HashSet<usize> = candidates.iter().map(|(i, _)| *i).collect();
	let mut sources: HashMap<&str, HashSet<String>> = HashMap::new();
	for entry in &doc.entries {
		if entry.msgid_plural.is_none() && !entry.msgid.is_empty() && !entry.msgstr.is_empty() {
			sources.entry(entry.msgstr.as_str()).or_default().insert(same_meaning_key(&entry.msgid));
		}
	}
	let mut count = 0;
	for (i, entry) in doc.entries.iter().enumerate() {
		let shared = sources.get(entry.msgstr.as_str()).is_some_and(|keys| keys.len() > 1);
		if entry.is_fuzzy && entry.msgid_plural.is_none() && shared && !already.contains(&i) {
			candidates.push((i, entry.msgid.clone()));
			count += 1;
		}
	}
	count
}

/// The words of an English string, without accelerators, case, spaces, and end punctuation. Two
/// strings with the same key say the same thing, so they can share a translation.
fn same_meaning_key(msgid: &str) -> String {
	msgid
		.chars()
		.filter(|c| !matches!(c, '&' | '.' | ':' | '…' | '!' | '?') && !c.is_whitespace())
		.flat_map(char::to_lowercase)
		.collect()
}

fn translate_one(
	project: &Project,
	po_path: &Path,
	pot_path: &Path,
	options: Options,
	client: Option<&claude::ClaudeClient>,
	context: &HashMap<String, String>,
) -> Result<(), Box<dyn Error>> {
	let lang = po_path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
	let style = load_style_note(project, &lang);
	let original = fs::read_to_string(po_path)?;
	// A copy, so the file is written only when something really changed.
	let tmp = env::temp_dir().join(format!("patois-translate-{lang}-{}.po", process::id()));
	fs::write(&tmp, &original)?;
	// Without `--no-fuzzy-matching`, msgmerge gives a new string the translation of the most
	// similar old one, marked fuzzy. Apps that compile fuzzy entries then show that guess, and
	// nothing marks it reliably afterwards. With it, a new string arrives empty and is translated.
	let merged_ok = Command::new("msgmerge")
		.args(["--update", "--backup=none", "--no-wrap", "--no-fuzzy-matching"])
		.arg(&tmp)
		.arg(pot_path)
		.status()
		.is_ok_and(|s| s.success());
	if !merged_ok {
		eprintln!("warning: msgmerge failed for {lang}, leaving it untouched this run");
		let _ = fs::remove_file(&tmp);
		return Ok(());
	}
	let merged = fs::read_to_string(&tmp)?;
	let _ = fs::remove_file(&tmp);
	let mut doc = PoDocument::parse(&merged);
	let mut candidates: Vec<(usize, String)> = doc.needs_translation().map(|(i, m)| (i, m.to_string())).collect();
	let mut plurals: Vec<(usize, String, String)> =
		doc.needs_plural_translation().map(|(i, s, p)| (i, s.to_string(), p.to_string())).collect();
	let mut repaired = if options.repair { add_damaged_entries(&doc, &mut candidates, &mut plurals) } else { 0 };
	if options.repair_copies {
		repaired += add_copied_entries(&doc, &mut candidates);
	}
	let total = candidates.len() + plurals.len();
	let Some(client) = client else {
		let style_note = style_note_suffix(project, &lang, style.is_some());
		if total == 0 {
			println!("{lang}: fully translated, nothing to do{style_note}");
		} else {
			let plural_note = if plurals.is_empty() { String::new() } else { format!(", {} plural", plurals.len()) };
			let repair_note = if repaired > 0 { format!(" ({repaired} damaged)") } else { String::new() };
			println!("{lang}: {total} entries would be translated{plural_note}{repair_note}{style_note}");
		}
		return Ok(());
	};
	let final_content = if total == 0 {
		merged
	} else {
		let language = patois::language_name(&lang);
		let target = claude::Target { language, style: style.as_deref() };
		let (mut applied, annotated) = translate_singulars(client, &target, &candidates, context)?;
		let plural_done = match plural_forms(&merged) {
			// Without the header there's no way to know how many forms to ask for, and a wrong
			// count makes a file that gettext accepts without a warning.
			None if !plurals.is_empty() => {
				eprintln!("warning: {lang} has no usable Plural-Forms header, leaving its plural entries");
				0
			}
			None => 0,
			Some((nplurals, rule)) => {
				let translated = translate_plurals(client, &target, &plurals, context, nplurals, &rule)?;
				let done = translated.len();
				applied.extend(translated);
				done
			}
		};
		let singular_done = applied.len() - plural_done;
		let skipped = total - applied.len();
		doc.apply(&applied);
		print!("{lang} ({language}): translated {singular_done} entries");
		if plural_done > 0 {
			print!(", {plural_done} plural");
		}
		if repaired > 0 {
			print!(", {repaired} of them repaired");
		}
		if annotated > 0 {
			print!(", {annotated} with translator notes");
		}
		if skipped > 0 {
			print!(", skipped {skipped} (failed a placeholder, accelerator, or shortcut check, will retry next run)");
		}
		println!();
		doc.render()
	};
	if content_without_volatile_headers(&original) != content_without_volatile_headers(&final_content) {
		fs::write(po_path, final_content)?;
	}
	Ok(())
}

/// `content` without the `POT-Creation-Date` and `PO-Revision-Date` header lines, which
/// `msgmerge` changes on every run.
fn content_without_volatile_headers(content: &str) -> String {
	content
		.lines()
		.filter(|line| {
			let t = line.trim();
			!(t.starts_with("\"POT-Creation-Date:") || t.starts_with("\"PO-Revision-Date:"))
		})
		.collect::<Vec<_>>()
		.join("\n")
}

#[cfg(test)]
mod tests {
	use std::fmt::Write as _;

	use super::*;

	#[test]
	fn timestamp_only_changes_are_ignored() {
		let a = "msgid \"\"\nmsgstr \"\"\n\"POT-Creation-Date: 2026-01-01 00:00+0000\\n\"\n\"PO-Revision-Date: 2026-01-01 00:00+0000\\n\"\n\"Language: de\\n\"\n";
		let b = "msgid \"\"\nmsgstr \"\"\n\"POT-Creation-Date: 2026-06-01 12:00+0000\\n\"\n\"PO-Revision-Date: 2026-06-01 12:00+0000\\n\"\n\"Language: de\\n\"\n";
		assert_eq!(content_without_volatile_headers(a), content_without_volatile_headers(b));
	}

	#[test]
	fn real_content_changes_are_detected() {
		let a = "msgid \"Cancel\"\nmsgstr \"\"\n";
		let b = "msgid \"Cancel\"\nmsgstr \"Abbrechen\"\n";
		assert_ne!(content_without_volatile_headers(a), content_without_volatile_headers(b));
	}

	#[test]
	fn human_maintained_locales_ignores_comments_and_blank_lines() {
		let content = "# comment\n\nfi   # Jani Kinnunen\n\n  sr  \n";
		assert_eq!(parse_human_maintained_locales(content), HashSet::from(["fi".to_string(), "sr".to_string()]));
		assert!(parse_human_maintained_locales("# nothing here yet\n").is_empty());
	}

	#[test]
	fn a_style_note_is_trimmed() {
		let content = "\n\n# Dutch\n\nAddress the reader as \"je\".\n\n\n";
		assert_eq!(parse_style_note(content), Some("# Dutch\n\nAddress the reader as \"je\".".to_string()));
		assert_eq!(parse_style_note("  \n\n"), None);
	}

	#[test]
	fn the_dry_run_line_names_the_style_note_it_would_use() {
		let project = Project {
			root: Path::new("."),
			po_dir: Path::new("po"),
			pot: Path::new("po/app.pot"),
			readme: None,
			app: App::default(),
		};
		assert_eq!(style_note_suffix(&project, "nl", true), ", with po/style/nl.md");
		assert_eq!(style_note_suffix(&project, "nl", false), "");
	}

	#[test]
	fn a_translator_note_attaches_to_the_msgid_below_it_only() {
		let pot = "#. TRANSLATORS: about Ready\nmsgid \"Ready\"\nmsgstr \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n";
		let notes = translator_comments(pot);
		assert_eq!(notes.get("Ready").map(String::as_str), Some("about Ready"));
		assert!(!notes.contains_key("Cancel"));
	}

	#[test]
	fn keys_are_unescaped_and_joined_across_continuation_lines() {
		let pot =
			"#. TRANSLATORS: a two-line prompt\nmsgid \"\"\n\"No parser for {}.\\n\"\n\"Open it how?\"\nmsgstr \"\"\n";
		let notes = translator_comments(pot);
		assert_eq!(notes.get("No parser for {}.\nOpen it how?").map(String::as_str), Some("a two-line prompt"));
	}

	#[test]
	fn the_header_entry_never_takes_a_note() {
		let pot = "#. TRANSLATORS: stray\nmsgid \"\"\nmsgstr \"\"\n\nmsgid \"Ready\"\nmsgstr \"\"\n";
		assert!(translator_comments(pot).is_empty());
	}

	#[test]
	fn plural_forms_reads_the_count_and_the_rule() {
		let po = "msgid \"\"\nmsgstr \"\"\n\"Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : 1);\\n\"\n";
		let (nplurals, rule) = plural_forms(po).unwrap();
		assert_eq!(nplurals, 3);
		assert!(rule.starts_with("Plural-Forms: nplurals=3;"), "got: {rule}");
		assert!(!rule.ends_with("\\n\""), "got: {rule}");
		assert!(plural_forms("msgid \"\"\nmsgstr \"\"\n").is_none());
	}

	fn catalog(entries: &[(&str, &str, bool)]) -> PoDocument {
		let mut text = String::from("msgid \"\"\nmsgstr \"Content-Type: text/plain; charset=UTF-8\\n\"\n");
		for (msgid, msgstr, fuzzy) in entries {
			text.push('\n');
			if *fuzzy {
				text.push_str("#, fuzzy\n");
			}
			let _ = writeln!(text, "msgid \"{msgid}\"\nmsgstr \"{msgstr}\"");
		}
		PoDocument::parse(&text)
	}

	fn copied(doc: &PoDocument) -> Vec<String> {
		let mut candidates = Vec::new();
		add_copied_entries(doc, &mut candidates);
		candidates.into_iter().map(|(_, msgid)| msgid).collect()
	}

	#[test]
	fn a_translation_shared_by_strings_that_say_different_things_is_a_copy() {
		let doc = catalog(&[("1 minute", "1 minuto", true), ("10 minutes", "1 minuto", true)]);
		assert_eq!(vec!["1 minute", "10 minutes"], copied(&doc));
	}

	#[test]
	fn the_same_words_sharing_a_translation_are_not_a_copy() {
		let doc = catalog(&[
			("&Close", "Cerrar", true),
			("Close", "Cerrar", true),
			("Open...", "Abrir", true),
			("Open", "Abrir", true),
		]);
		assert!(copied(&doc).is_empty());
	}

	#[test]
	fn a_human_translation_is_never_selected() {
		let doc = catalog(&[("Match Case", "Mayusculas", false), ("Batch OCR", "Mayusculas", true)]);
		assert_eq!(vec!["Batch OCR"], copied(&doc));
	}
}
