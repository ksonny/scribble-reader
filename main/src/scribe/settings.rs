use std::path::PathBuf;

use serde::Deserialize;

pub(crate) const DEFAULT_SCRIBE_CONFIG: &str = r#"
[library]
name = "Library"
path = "~/Documents/ebooks"
"#;

#[derive(Debug, Deserialize)]
pub(crate) struct Library {
	pub(crate) name: String,
	pub(crate) path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Scribe {
	pub(crate) library: Library,
}
