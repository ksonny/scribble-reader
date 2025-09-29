use std::path::PathBuf;

use serde::Deserialize;

#[cfg(not(target_os = "android"))]
pub(crate) const DEFAULT_SCRIBE_CONFIG: &str = r#"
[library]
name = "Library"
path = "~/Documents/ebooks"
"#;

#[cfg(target_os = "android")]
pub(crate) const DEFAULT_SCRIBE_CONFIG: &str = r#"
[library]
name = "Library"
path = "/sdcard/Books/ebooks"
"#;

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct Library {
	pub(crate) name: String,
	pub(crate) path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Scribe {
	pub(crate) library: Library,
}
