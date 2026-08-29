//! Build-script and `xtask` helpers for [patois](https://docs.rs/patois): compile `.po`
//! catalogs into the `.mo` files an app loads at runtime, regenerate a `.pot` template from
//! source, and generate the equivalent translation assets for iOS and Android.

mod entries;
mod extract;
mod mo;
mod mobile;
pub mod po;
mod pot;
mod sanitize_rust;

pub use extract::extend_pot_from_source_dirs;
pub use mo::compile_translations;
pub use mobile::{gen_android_strings, gen_ios_strings};
pub use pot::{gen_pot, gen_pot_from_dirs};
