use std::{
	collections::HashSet,
	env, error, fs,
	path::{Path, PathBuf},
	process::Command,
};

/// Compile all `.po` files in `po_dir` into `.mo` files under `locale_dir`.
///
/// Output path for each language: `<locale_dir>/<lang>/LC_MESSAGES/<domain>.mo` where `<domain>` is the crate name (`CARGO_PKG_NAME`).
///
/// Relative paths are resolved from `CARGO_MANIFEST_DIR`. Emits `cargo:rerun-if-changed` lines for the input directory and every `.po` file. Requires `msgfmt` on `PATH`; prints a `cargo:warning` if it is missing.
pub fn compile_translations(po_dir: impl AsRef<Path>, locale_dir: impl AsRef<Path>) {
	let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
	let abs = |p: &Path| if p.is_absolute() { p.to_path_buf() } else { manifest_dir.join(p) };
	let po_dir = abs(po_dir.as_ref());
	let locale_dir = abs(locale_dir.as_ref());
	let domain = env::var("CARGO_PKG_NAME").unwrap_or_default();
	println!("cargo:rerun-if-changed={}", po_dir.display());
	println!("cargo:rerun-if-changed={}", locale_dir.display());
	let entries = match fs::read_dir(&po_dir) {
		Ok(e) => e,
		Err(e) => {
			println!("cargo:warning=patois-build: could not read {}: {e}", po_dir.display());
			return;
		}
	};
	for entry in entries {
		let path = match entry {
			Ok(e) => e.path(),
			Err(e) => {
				println!("cargo:warning=patois-build: {e}");
				continue;
			}
		};
		if path.extension().and_then(|e| e.to_str()) != Some("po") {
			continue;
		}
		let lang = match path.file_stem().and_then(|s| s.to_str()) {
			Some(l) => l.to_string(),
			None => continue,
		};
		println!("cargo:rerun-if-changed={}", path.display());
		let out_dir = locale_dir.join(&lang).join("LC_MESSAGES");
		if let Err(e) = fs::create_dir_all(&out_dir) {
			println!("cargo:warning=patois-build: could not create {}: {e}", out_dir.display());
			continue;
		}
		run_msgfmt(&path, &out_dir.join(format!("{domain}.mo")));
	}
}

fn run_msgfmt(input: &Path, output: &Path) {
	match Command::new("msgfmt").arg(input).arg("-o").arg(output).status() {
		Ok(s) if s.success() => {}
		Ok(s) => println!("cargo:warning=patois-build: msgfmt exited with {s} compiling {}", input.display()),
		Err(e) => println!("cargo:warning=patois-build: msgfmt not available ({}); install gettext tools", e),
	}
}

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
		collect_rust_files(dir.as_ref(), &mut files)?;
	}
	if files.is_empty() {
		return Ok(());
	}
	fs::create_dir_all(po_dir)?;
	let output_file = po_dir.join(format!("{package_name}.pot"));
	let temp_file = po_dir.join(format!("{package_name}.pot.new"));
	let mut cmd = Command::new("xgettext");
	cmd.arg("--keyword=t")
		.arg("--language=C")
		.arg("--from-code=UTF-8")
		.arg("--add-comments=TRANSLATORS")
		.arg("--no-location")
		.arg(format!("--package-name={package_name}"))
		.arg(format!("--package-version={package_version}"))
		.arg(format!("--output={}", temp_file.display()));
	for file in &files {
		cmd.arg(file);
	}
	if !cmd.status()?.success() {
		return Err("xgettext failed".into());
	}
	if pot_changed(&output_file, &temp_file) {
		fs::rename(&temp_file, &output_file)?;
	} else {
		fs::remove_file(&temp_file)?;
	}
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
	let packages = meta["packages"].as_array().ok_or("cargo metadata: missing packages")?;
	let mut files: Vec<PathBuf> = Vec::new();
	for pkg in packages {
		if pkg["metadata"]["patois"]["translatable"] != true {
			continue;
		}
		let manifest = pkg["manifest_path"].as_str().ok_or("cargo metadata: missing manifest_path")?;
		let src = Path::new(manifest).parent().unwrap().join("src");
		collect_rust_files(&src, &mut files)?;
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
	let temp_file = po_dir.join(format!("{package_name}.pot.new"));
	let mut cmd = Command::new("xgettext");
	cmd.arg("--keyword=t")
		.arg("--language=C")
		.arg("--from-code=UTF-8")
		.arg("--add-comments=TRANSLATORS")
		.arg("--no-location")
		.arg(format!("--package-name={package_name}"))
		.arg(format!("--package-version={version}"))
		.arg(format!("--output={}", temp_file.display()));
	for file in &files {
		cmd.arg(file);
	}
	if !cmd.status()?.success() {
		return Err("xgettext failed".into());
	}
	if pot_changed(&output_file, &temp_file) {
		fs::rename(&temp_file, &output_file)?;
		println!("Updated {}", output_file.display());
	} else {
		fs::remove_file(&temp_file)?;
		println!("No changes ({})", output_file.display());
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
/// Scans files matching `extension` (e.g. `"swift"` or `"kt"`) for `t("...")` calls using a
/// native Rust parser — no xgettext required. Handles standard C-style escape sequences in
/// string literals and skips strings that are already present in the pot file.
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

	// Collect t("...") string literals from every source file, preserving first-seen order.
	let mut new_strings: Vec<String> = Vec::new();
	let mut seen_in_scan: HashSet<String> = HashSet::new();
	for file in &files {
		let content = fs::read_to_string(file)?;
		for s in extract_t_strings(&content) {
			if seen_in_scan.insert(s.clone()) {
				new_strings.push(s);
			}
		}
	}
	if new_strings.is_empty() {
		return Ok(());
	}

	// Read the existing pot and collect msgids already present.
	let existing = fs::read_to_string(pot_file)?;
	let existing_ids = collect_pot_msgids(&existing);

	// Append only truly new entries.
	let mut additions = String::new();
	for s in &new_strings {
		if !existing_ids.contains(s) {
			additions.push_str(&format!("\nmsgid \"{}\"\nmsgstr \"\"\n", pot_escape(s)));
		}
	}
	if !additions.is_empty() {
		let content = format!("{existing}{additions}");
		fs::write(pot_file, content)?;
	}
	Ok(())
}

/// Extract every `t("literal")` value from `content`.
///
/// Handles standard C/Swift/Kotlin escape sequences (`\\`, `\"`, `\n`, `\t`). Ignores `t(` when
/// preceded by an alphanumeric character or underscore (e.g. `stateDescription`).
fn extract_t_strings(content: &str) -> Vec<String> {
	let chars: Vec<char> = content.chars().collect();
	let n = chars.len();
	let mut out: Vec<String> = Vec::new();
	let mut i = 0;
	while i < n {
		// Match `t(` not preceded by an identifier character.
		if chars[i] == 't' && i + 1 < n && chars[i + 1] == '(' {
			let preceded_by_ident = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
			if !preceded_by_ident {
				let mut j = i + 2;
				while j < n && chars[j].is_ascii_whitespace() {
					j += 1;
				}
				if j < n && chars[j] == '"' {
					j += 1;
					let mut s = String::new();
					let mut valid = false;
					'string: loop {
						if j >= n {
							break;
						}
						match chars[j] {
							'"' => {
								valid = true;
								j += 1;
								break 'string;
							}
							'\\' if j + 1 < n => {
								j += 1;
								match chars[j] {
									'n' => s.push('\n'),
									't' => s.push('\t'),
									'"' => s.push('"'),
									'\\' => s.push('\\'),
									c => {
										s.push('\\');
										s.push(c);
									}
								}
								j += 1;
							}
							'\n' | '\r' => break 'string, // unterminated
							c => {
								s.push(c);
								j += 1;
							}
						}
					}
					if valid && !s.is_empty() {
						out.push(s);
					}
					i = j;
					continue;
				}
			}
		}
		i += 1;
	}
	out
}

/// Parse msgid values already present in a pot/po file.
fn collect_pot_msgids(content: &str) -> HashSet<String> {
	let mut ids: HashSet<String> = HashSet::new();
	let mut current = String::new();
	let mut in_msgid = false;
	for line in content.lines() {
		let line = line.trim();
		if let Some(rest) = line.strip_prefix("msgid ") {
			if !current.is_empty() {
				ids.insert(std::mem::take(&mut current));
			}
			current = po_unescape(rest);
			in_msgid = true;
		} else if line.starts_with("msgstr ") {
			if !current.is_empty() {
				ids.insert(std::mem::take(&mut current));
			}
			in_msgid = false;
		} else if in_msgid && line.starts_with('"') {
			current.push_str(&po_unescape(line));
		}
	}
	if !current.is_empty() {
		ids.insert(current);
	}
	ids
}

/// Escape a string for use as a pot msgid value (between the outer double-quotes).
fn pot_escape(s: &str) -> String {
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

/// Parse a gettext `.po` file and return `(msgid, msgstr)` pairs where `msgstr` is non-empty.
fn parse_po_entries(content: &str) -> Vec<(String, String)> {
	let mut entries: Vec<(String, String)> = Vec::new();
	let mut msgid = String::new();
	let mut msgstr = String::new();
	let mut in_msgid = false;
	let mut in_msgstr = false;
	let mut pending_id: Option<String> = None;

	let flush = |pending_id: &mut Option<String>, msgstr: &mut String, entries: &mut Vec<(String, String)>| {
		if let Some(id) = pending_id.take() {
			if !id.is_empty() && !msgstr.is_empty() {
				entries.push((id, std::mem::take(msgstr)));
			} else {
				msgstr.clear();
			}
		}
	};

	for line in content.lines() {
		let line = line.trim();
		if let Some(rest) = line.strip_prefix("msgid ") {
			flush(&mut pending_id, &mut msgstr, &mut entries);
			msgid = po_unescape(rest);
			in_msgid = true;
			in_msgstr = false;
		} else if let Some(rest) = line.strip_prefix("msgstr ") {
			pending_id = Some(std::mem::take(&mut msgid));
			msgstr = po_unescape(rest);
			in_msgid = false;
			in_msgstr = true;
		} else if line.starts_with('"') {
			let cont = po_unescape(line);
			if in_msgid {
				msgid.push_str(&cont);
			} else if in_msgstr {
				msgstr.push_str(&cont);
			}
		} else if line.is_empty() || line.starts_with('#') {
			in_msgid = false;
			in_msgstr = false;
		}
	}
	flush(&mut pending_id, &mut msgstr, &mut entries);
	entries
}

fn po_unescape(s: &str) -> String {
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

fn escape_for_localizable_strings(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for c in s.chars() {
		match c {
			'"' => out.push_str("\\\""),
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			c => out.push(c),
		}
	}
	out
}

/// Generate `<lang>.lproj/Localizable.strings` files for iOS from `.po` translation files.
///
/// For each `<lang>.po` in `po_dir`, creates `<ios_dir>/<lang>.lproj/Localizable.strings`
/// containing only translated (non-empty msgstr) entries. Call this from the iOS build step.
pub fn gen_ios_strings(po_dir: impl AsRef<Path>, ios_dir: impl AsRef<Path>) -> Result<(), Box<dyn error::Error>> {
	let po_dir = po_dir.as_ref();
	let ios_dir = ios_dir.as_ref();
	let dir_entries = fs::read_dir(po_dir).map_err(|e| format!("cannot read {}: {e}", po_dir.display()))?;
	for entry in dir_entries {
		let path = entry?.path();
		if path.extension().and_then(|e| e.to_str()) != Some("po") {
			continue;
		}
		let lang = match path.file_stem().and_then(|s| s.to_str()) {
			Some(l) => l.to_string(),
			None => continue,
		};
		let content = fs::read_to_string(&path)?;
		let translations = parse_po_entries(&content);
		if translations.is_empty() {
			continue;
		}
		let lproj = ios_dir.join(format!("{lang}.lproj"));
		fs::create_dir_all(&lproj)?;
		let out_path = lproj.join("Localizable.strings");
		let mut out = String::new();
		for (msgid, msgstr) in &translations {
			out.push_str(&format!(
				"\"{}\" = \"{}\";\n",
				escape_for_localizable_strings(msgid),
				escape_for_localizable_strings(msgstr)
			));
		}
		let existing = fs::read_to_string(&out_path).unwrap_or_default();
		if existing != out {
			fs::write(&out_path, &out)?;
			println!("Updated {}", out_path.display());
		}
	}
	Ok(())
}

/// Generate `translations/<lang>.json` asset files for Android from `.po` translation files.
///
/// For each `<lang>.po` in `po_dir`, creates `<assets_dir>/translations/<lang>.json`
/// with `{"msgid": "msgstr"}` entries for all translated strings. Call this from the Android
/// build step so the APK bundles the translations as assets.
pub fn gen_android_strings(
	po_dir: impl AsRef<Path>,
	assets_dir: impl AsRef<Path>,
) -> Result<(), Box<dyn error::Error>> {
	let po_dir = po_dir.as_ref();
	let translations_dir = assets_dir.as_ref().join("translations");
	let dir_entries = fs::read_dir(po_dir).map_err(|e| format!("cannot read {}: {e}", po_dir.display()))?;
	for entry in dir_entries {
		let path = entry?.path();
		if path.extension().and_then(|e| e.to_str()) != Some("po") {
			continue;
		}
		let lang = match path.file_stem().and_then(|s| s.to_str()) {
			Some(l) => l.to_string(),
			None => continue,
		};
		let content = fs::read_to_string(&path)?;
		let translations = parse_po_entries(&content);
		if translations.is_empty() {
			continue;
		}
		fs::create_dir_all(&translations_dir)?;
		let out_path = translations_dir.join(format!("{lang}.json"));
		let map: serde_json::Map<String, serde_json::Value> =
			translations.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect();
		let json = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
		let json = json + "\n";
		let existing = fs::read_to_string(&out_path).unwrap_or_default();
		if existing != json {
			fs::write(&out_path, &json)?;
			println!("Updated {}", out_path.display());
		}
	}
	Ok(())
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
}
