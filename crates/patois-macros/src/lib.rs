use std::{env, fs, path::Path};

use proc_macro::TokenStream;
use quote::quote;

/// Embed the calling crate's locale files into the binary at compile time.
///
/// Scans `<crate_root>/locale/<lang>/LC_MESSAGES/<crate_name>.mo` for every compiled `.mo` file and registers them with the patois global registry via the `inventory` crate. Call once at module level in a library crate.
///
/// The locale directory and `.mo` files must exist at compile time. Typically
/// this means generating them in a `build.rs` that emits `cargo:rerun-if-changed=locale`.
#[proc_macro]
pub fn embed_domain(_input: TokenStream) -> TokenStream {
	let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set during macro expansion");
	let pkg_name = env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME not set during macro expansion");
	let locale_dir = Path::new(&manifest_dir).join("locale");
	let mut file_entries = vec![];
	if locale_dir.exists() {
		if let Ok(entries) = fs::read_dir(&locale_dir) {
			let mut locales: Vec<_> = entries.flatten().collect();
			locales.sort_by_key(|e| e.file_name());
			for entry in locales {
				let locale_name = entry.file_name().to_string_lossy().to_string();
				let mo_path = entry.path().join("LC_MESSAGES").join(format!("{pkg_name}.mo"));
				if mo_path.exists() {
					let mo_path_str = mo_path.to_string_lossy().to_string();
					file_entries.push(quote! {
						(#locale_name, &include_bytes!(#mo_path_str)[..])
					});
				}
			}
		}
	}
	quote! {
		::patois::inventory::submit! {
			::patois::EmbeddedDomain {
				name: ::env!("CARGO_PKG_NAME"),
				files: &[ #(#file_entries),* ],
			}
		}
	}
	.into()
}
