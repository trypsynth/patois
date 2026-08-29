//! Compiling `.po` catalogs into the binary `.mo` files an app loads at runtime.

use std::{
	env, fs,
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
	// `--use-fuzzy`: `PoDocument::apply_all` leaves every machine-translated entry flagged
	// `#, fuzzy` permanently (see its doc comment and `needs_translation`), as the marker
	// that it's translated-but-unreviewed rather than untranslated. msgfmt's default of
	// dropping fuzzy entries from the compiled catalog would make that distinction pointless
	// here: the entry would sit in the .po translated forever but never reach the app.
	match Command::new("msgfmt").arg("--use-fuzzy").arg(input).arg("-o").arg(output).status() {
		Ok(s) if s.success() => {}
		Ok(s) => println!("cargo:warning=patois-build: msgfmt exited with {s} compiling {}", input.display()),
		Err(e) => println!("cargo:warning=patois-build: msgfmt not available ({}); install gettext tools", e),
	}
}
