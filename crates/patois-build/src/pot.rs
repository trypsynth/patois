//! Regenerating a `.pot` template from Rust sources via `xgettext`.

use std::{
	collections::HashSet,
	env, error, fs,
	path::{Path, PathBuf},
	process::{self, Command},
};

use crate::{
	entries::{collect_pot_entries_ordered, collect_pot_msgids, pot_entry_block},
	sanitize_rust,
};

/// Regenerate `<po_dir>/<package_name>.pot` from explicit source directories.
///
/// Unlike [`gen_pot`], this does not invoke `cargo` and is safe to call from a build script.
/// Requires `xgettext` on `PATH`; returns `Err` (not a hard failure) if it is missing.
pub fn gen_pot_from_dirs(
	source_dirs: &[impl AsRef<Path>],
	po_dir: impl AsRef<Path>,
	package_name: &str,
	package_version: &str,
) -> Result<(), Box<dyn error::Error>> {
	let po_dir = po_dir.as_ref();
	if Command::new("xgettext").arg("--version").output().is_err() {
		return Err("xgettext not found; install gettext tools".into());
	}
	let mut files: Vec<PathBuf> = Vec::new();
	for dir in source_dirs {
		// Sort within each directory (safe: paths share a common prefix, so absolute-path
		// order matches relative order on every machine) rather than sorting the combined
		// list (unsafe: `source_dirs` entries can resolve to unrelated absolute locations,
		// e.g. one under the project checkout and one under a dependency's cache, whose
		// relative order then depends on incidental, machine-specific path spelling like
		// `.cargo` vs `scoop`). See the longer comment in `gen_pot`.
		let mut dir_files: Vec<PathBuf> = Vec::new();
		collect_rust_files(dir.as_ref(), &mut dir_files)?;
		dir_files.sort();
		files.extend(dir_files);
	}
	if files.is_empty() {
		return Ok(());
	}
	write_pot(&files, po_dir, package_name, package_version)?;
	Ok(())
}

/// Regenerate `<po_dir>/<package_name>.pot` by scanning all workspace crates tagged with `[package.metadata.patois] translatable = true`.
///
/// Pass the name of the primary package, used for the output filename, `--package-name`, and `--package-version` in the generated header. Requires `xgettext` and `cargo` on `PATH`.
pub fn gen_pot(
	project_root: impl AsRef<Path>,
	po_dir: impl AsRef<Path>,
	package_name: &str,
) -> Result<(), Box<dyn error::Error>> {
	let root = project_root.as_ref();
	let po_dir = po_dir.as_ref();
	fs::create_dir_all(po_dir)?;
	if Command::new("xgettext").arg("--version").output().is_err() {
		return Err("xgettext not found; install gettext tools (e.g. `scoop install gettext`)".into());
	}
	let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
	let meta_output = Command::new(&cargo).args(["metadata", "--format-version", "1"]).current_dir(root).output()?;
	if !meta_output.status.success() {
		return Err("cargo metadata failed".into());
	}
	let meta: serde_json::Value = serde_json::from_slice(&meta_output.stdout)?;
	let mut packages: Vec<&serde_json::Value> =
		meta["packages"].as_array().ok_or("cargo metadata: missing packages")?.iter().collect();
	// `fs::read_dir` order isn't guaranteed stable across runs, and neither is `cargo
	// metadata`'s package ordering — sort packages by name (not filesystem path: a package's
	// manifest_path can resolve under the project checkout for one contributor and under
	// their CARGO_HOME's registry/git cache for another, e.g. `.cargo` vs `scoop`, or `git` vs
	// `pb`, and those absolute prefixes sort differently from machine to machine) so package
	// order is deterministic and independent of where things happen to live on disk.
	packages.sort_by_key(|pkg| pkg["name"].as_str().unwrap_or_default().to_string());
	let mut files: Vec<PathBuf> = Vec::new();
	for pkg in &packages {
		if pkg["metadata"]["patois"]["translatable"] != true {
			continue;
		}
		let manifest = pkg["manifest_path"].as_str().ok_or("cargo metadata: missing manifest_path")?;
		let src = Path::new(manifest).parent().unwrap().join("src");
		// Sort each package's own files by absolute path — safe here since they all share
		// that package's src root as a common prefix, so relative order is stable across
		// machines even though the shared prefix itself isn't. This is what keeps the file
		// list (and therefore xgettext's output) deterministic between runs, so pot_changed's
		// comparison below isn't fooled by incidental reordering into a spurious rewrite (with
		// a fresh POT-Creation-Date) on every build.
		let mut pkg_files: Vec<PathBuf> = Vec::new();
		collect_rust_files(&src, &mut pkg_files)?;
		pkg_files.sort();
		files.extend(pkg_files);
	}
	if files.is_empty() {
		return Err("no translatable source files found — check [package.metadata.patois] translatable = true".into());
	}
	let version = packages
		.iter()
		.find(|p| p["name"] == package_name)
		.and_then(|p| p["version"].as_str())
		.unwrap_or("0.0.0")
		.to_string();
	let output_file = po_dir.join(format!("{package_name}.pot"));
	if write_pot(&files, po_dir, package_name, &version)? {
		println!("Updated {}", output_file.display());
	} else {
		println!("No changes ({})", output_file.display());
	}
	Ok(())
}

/// Run `xgettext` over `files` and merge the result into `<po_dir>/<package_name>.pot`,
/// returning whether the pot on disk actually changed.
///
/// The sources are sanitized on the way in (see [`sanitize_rust`]): xgettext has no Rust
/// mode, and the C tokenizer it is asked to use instead runs on past lifetimes and raw
/// strings, swallowing or splicing unrelated strings further down the file, and in later
/// files too since they all go to one invocation. Doing it here rather than exposing a
/// helper keeps `--language=C` an implementation detail of this crate, so no caller has to
/// know about it or remember to work around it.
fn write_pot(
	files: &[PathBuf],
	po_dir: &Path,
	package_name: &str,
	package_version: &str,
) -> Result<bool, Box<dyn error::Error>> {
	fs::create_dir_all(po_dir)?;
	let output_file = po_dir.join(format!("{package_name}.pot"));
	let temp_file = po_dir.join(format!("{package_name}.pot.new"));
	let scratch = env::temp_dir().join(format!("patois-build-pot-{}", process::id()));
	let sanitized = sanitize_files_into(&scratch, files);
	let result = sanitized.and_then(|sources| run_xgettext(&sources, &temp_file, package_name, package_version));
	let _ = fs::remove_dir_all(&scratch);
	result?;
	strip_format_flags(&temp_file)?;
	preserve_foreign_entries(&output_file, &temp_file)?;
	if pot_changed(&output_file, &temp_file) {
		fs::rename(&temp_file, &output_file)?;
		Ok(true)
	} else {
		fs::remove_file(&temp_file)?;
		Ok(false)
	}
}

/// Write a sanitized copy of every file in `files` into `scratch`, returning the copies in
/// the same order.
///
/// The copies live flat in one directory, named by their position in `files`: two crates can
/// each have a `main.rs`, and the names never reach the pot (`--no-location`), so they only
/// have to be unique and recognisable in an xgettext error message.
fn sanitize_files_into(scratch: &Path, files: &[PathBuf]) -> Result<Vec<PathBuf>, Box<dyn error::Error>> {
	fs::create_dir_all(scratch)?;
	let mut sources = Vec::with_capacity(files.len());
	for (index, file) in files.iter().enumerate() {
		let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "src.rs".to_string());
		let dest = scratch.join(format!("{index:05}-{name}"));
		fs::write(&dest, sanitize_rust::sanitize_for_xgettext(&fs::read_to_string(file)?))?;
		sources.push(dest);
	}
	Ok(sources)
}

fn run_xgettext(
	sources: &[PathBuf],
	output: &Path,
	package_name: &str,
	package_version: &str,
) -> Result<(), Box<dyn error::Error>> {
	let mut cmd = Command::new("xgettext");
	cmd.arg("--keyword=t")
		.arg("--keyword=nt:1,2")
		.arg("--language=C")
		.arg("--from-code=UTF-8")
		.arg("--add-comments=TRANSLATORS")
		.arg("--no-location")
		.arg("--no-wrap")
		.arg(format!("--package-name={package_name}"))
		.arg(format!("--package-version={package_version}"))
		.arg(format!("--output={}", output.display()));
	for source in sources {
		cmd.arg(source);
	}
	if !cmd.status()?.success() {
		return Err("xgettext failed".into());
	}
	Ok(())
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn error::Error>> {
	if !dir.is_dir() {
		return Ok(());
	}
	for entry in fs::read_dir(dir)? {
		let path = entry?.path();
		if path.is_dir() {
			collect_rust_files(&path, files)?;
		} else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
			files.push(path);
		}
	}
	Ok(())
}

/// Strips `c-format`/`no-c-format` flags that `xgettext`'s built-in heuristic attaches to `#,`
/// comment lines based on scanning each msgid for `%`-style directives. Whether a given msgid
/// gets flagged is a heuristic guess that differs across `xgettext` versions and platforms
/// (confirmed: the same source produced different flags on different machines), and nothing
/// in typical downstream tooling (`msgfmt --check-format`, custom placeholder validation)
/// necessarily reads this flag, so keeping it around only adds version-dependent churn to the
/// generated file for no guaranteed benefit. Leaves other flags (e.g. `fuzzy`) untouched.
fn strip_format_flags(path: &Path) -> Result<(), Box<dyn error::Error>> {
	let content = fs::read_to_string(path)?;
	let mut out = String::with_capacity(content.len());
	for line in content.lines() {
		if let Some(rest) = line.strip_prefix("#,") {
			let kept: Vec<&str> =
				rest.split(',').map(str::trim).filter(|flag| *flag != "c-format" && *flag != "no-c-format").collect();
			if kept.is_empty() {
				continue;
			}
			out.push_str("#, ");
			out.push_str(&kept.join(", "));
			out.push('\n');
			continue;
		}
		out.push_str(line);
		out.push('\n');
	}
	fs::write(path, out)?;
	Ok(())
}

/// Returns true if `.pot` content changed, ignoring the `POT-Creation-Date` header line.
fn pot_changed(old: &Path, new: &Path) -> bool {
	let strip_date = |s: &str| -> String {
		s.lines().filter(|l| !l.starts_with("\"POT-Creation-Date:")).collect::<Vec<_>>().join("\n")
	};
	let old = fs::read_to_string(old).unwrap_or_default();
	let new = match fs::read_to_string(new) {
		Ok(c) => c,
		Err(_) => return true,
	};
	strip_date(&old) != strip_date(&new)
}

/// Copy entries present in `old` but absent from `new` into `new`, appending them at the end.
///
/// `gen_pot`/`gen_pot_from_dirs` regenerate the `.pot` from a single language's sources (Rust),
/// but callers often layer other languages on top afterward via
/// [`extend_pot_from_source_dirs`] (e.g. iOS/Android). Without this step, the freshly
/// regenerated file would always be missing those entries relative to the accumulated file on
/// disk, so `pot_changed` would see a spurious difference and bump `POT-Creation-Date` on every
/// single build even when no msgid actually changed.
fn preserve_foreign_entries(old: &Path, new: &Path) -> Result<(), Box<dyn error::Error>> {
	let Ok(old_content) = fs::read_to_string(old) else {
		return Ok(());
	};
	let new_content = fs::read_to_string(new)?;
	let new_ids = collect_pot_msgids(&new_content);
	let mut additions = String::new();
	let mut seen: HashSet<String> = HashSet::new();
	for entry in collect_pot_entries_ordered(&old_content) {
		if !new_ids.contains(&entry.msgid) && seen.insert(entry.msgid.clone()) {
			additions.push_str(&pot_entry_block(&entry));
		}
	}
	if !additions.is_empty() {
		let mut content = new_content;
		content.push_str(&additions);
		fs::write(new, content)?;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn strip_format_flags_drops_c_format_lines_but_keeps_other_flags() {
		let content = "\
#. a comment
#, c-format
msgid \"Page %d\"
msgstr \"\"

#, fuzzy, c-format
msgid \"Old %s\"
msgstr \"\"

#, fuzzy
msgid \"Cancel\"
msgstr \"\"

msgid \"No flags here\"
msgstr \"\"
";
		let path = env::temp_dir().join(format!("patois-build-strip-format-flags-test-{}.pot", std::process::id()));
		fs::write(&path, content).unwrap();
		strip_format_flags(&path).unwrap();
		let result = fs::read_to_string(&path).unwrap();
		fs::remove_file(&path).unwrap();
		assert!(!result.contains("c-format"));
		assert!(result.contains("#, fuzzy\nmsgid \"Old %s\""));
		assert!(result.contains("#, fuzzy\nmsgid \"Cancel\""));
		assert!(result.contains("msgid \"No flags here\""));
	}

	#[test]
	fn preserve_foreign_entries_keeps_plural_entries_intact() {
		let dir = env::temp_dir().join(format!("patois-build-preserve-plural-test-{}", std::process::id()));
		fs::create_dir_all(&dir).unwrap();
		let old = dir.join("old.pot");
		let new = dir.join("new.pot");
		// The old pot has a plural entry that the fresh xgettext scan (simulated by `new`)
		// no longer produced, e.g. because gap 1 (missing --keyword=nt:1,2) meant it was
		// hand-added and would otherwise get flattened on the next regeneration.
		fs::write(
			&old,
			"msgid \"\"\nmsgstr \"\"\n\nmsgid \"{} result\"\nmsgid_plural \"{} results\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n",
		)
		.unwrap();
		fs::write(&new, "msgid \"\"\nmsgstr \"\"\n").unwrap();
		preserve_foreign_entries(&old, &new).unwrap();
		let result = fs::read_to_string(&new).unwrap();
		fs::remove_dir_all(&dir).unwrap();
		assert!(result.contains("msgid \"{} result\"\nmsgid_plural \"{} results\""));
		assert!(result.contains("msgstr[0] \"\""));
		assert!(result.contains("msgstr[1] \"\""));
	}
}
