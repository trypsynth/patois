use std::{
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
