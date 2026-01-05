use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

#[cfg(not(target_os = "android"))]
pub(crate) const DEFAULT_SCRIBE_CONFIG: &str = r#"
[library]
name = "Library"
path = "~/Documents/ebooks"

[illustrator.body]
family = "Noto Serif"
size_px = 18.0
line_height = 1.5

[illustrator.h1]
size_em = 3.0

[illustrator.h2]
size_em = 2.5

[illustrator.h3]
size_em = 2.0

[illustrator.h4]
size_em = 1.7

[illustrator.h5]
size_em = 1.4

[illustrator.padding]
top_em = 2.0
left_em = 2.0
right_em = 2.0
bottom_em = 3.0
paragraph_em = 0.5
"#;

#[cfg(target_os = "android")]
pub(crate) const DEFAULT_SCRIBE_CONFIG: &str = r#"
[library]
name = "Library"
path = "/sdcard/Books/ebooks"

[illustrator.body]
family = "Noto Serif"
size_px = 18.0
line_height = 1.5

[illustrator.h1]
size_em = 3.0

[illustrator.h2]
size_em = 2.5

[illustrator.h3]
size_em = 2.0

[illustrator.h4]
size_em = 1.7

[illustrator.h5]
size_em = 1.4

[illustrator.padding]
top_em = 0.5
left_em = 0.5
right_em = 0.5
bottom_em = 1.0
paragraph_em = 0.5
"#;

#[allow(unused)]
#[derive(Debug, Deserialize)]
pub struct Library {
	pub name: String,
	pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct BodyTextConfig {
	pub family: String,
	pub size_px: f32,
	pub line_height: f32,
}

#[derive(Debug, Deserialize)]
pub struct H1TextConfig {
	pub size_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct H2TextConfig {
	pub size_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct H3TextConfig {
	pub size_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct H4TextConfig {
	pub size_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct H5TextConfig {
	pub size_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct PaddingConfig {
	pub top_em: f32,
	pub left_em: f32,
	pub right_em: f32,
	pub bottom_em: f32,

	pub paragraph_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct Illustrator {
	pub body: BodyTextConfig,
	pub h1: H1TextConfig,
	pub h2: H2TextConfig,
	pub h3: H3TextConfig,
	pub h4: H4TextConfig,
	pub h5: H5TextConfig,
	pub padding: PaddingConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Paths {
	pub cache_path: PathBuf,
	pub config_path: PathBuf,
	pub data_path: PathBuf,
}
