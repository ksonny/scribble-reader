use std::path::PathBuf;

use serde::Deserialize;

#[cfg(not(target_os = "android"))]
pub(crate) const DEFAULT_SCRIBE_CONFIG: &str = r#"
[library]
name = "Library"
path = "~/Documents/ebooks"

[illustrator]
font_size = 18.0
line_height = 1.5
h1 = { font_size_em = 2.4 }
h2 = { font_size_em = 2.0 }
h3 = { font_size_em = 1.8 }
h4 = { font_size_em = 1.2 }
h5 = { font_size_em = 1.0 }

[illustrator.font_regular]
family = "sans-serif"
variation = { wght = 400 }

[illustrator.font_italic]
family = "sans-serif"
variation = { wght = 400, ital = 1 }

[illustrator.font_bold]
family = "sans-serif"
variation = { wght = 700 }

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

[illustrator]
font_size = 18.0
line_height = 1.5
h1 = { font_size_em = 2.4 }
h2 = { font_size_em = 2.0 }
h3 = { font_size_em = 1.8 }
h4 = { font_size_em = 1.2 }
h5 = { font_size_em = 1.0 }

[illustrator.font_regular]
family = "EB Garamond"
variation = { wght = 400 }

[illustrator.font_italic]
family = "EB Garamond"
variation = { wght = 400, ital = 1 }

[illustrator.font_bold]
family = "EB Garamond"
variation = { wght = 700 }

[illustrator.padding]
top_em = 0.5
left_em = 0.5
right_em = 0.5
bottom_em = 1.0
paragraph_em = 0.5
"#;

#[derive(Debug, Deserialize)]
pub struct Library {
	pub name: String,
	pub path: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
pub struct FontVariationConfig {
	pub wght: Option<f32>,
	pub wdth: Option<f32>,
	pub ital: Option<f32>,
	pub slnt: Option<f32>,
	pub opzs: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct FontConfig {
	pub family: String,
	#[serde(default)]
	pub variation: FontVariationConfig,
}

impl AsRef<FontConfig> for FontConfig {
	fn as_ref(&self) -> &FontConfig {
		self
	}
}

#[derive(Debug, Deserialize)]
pub struct H1TextConfig {
	pub font_size_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct H2TextConfig {
	pub font_size_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct H3TextConfig {
	pub font_size_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct H4TextConfig {
	pub font_size_em: f32,
}

#[derive(Debug, Deserialize)]
pub struct H5TextConfig {
	pub font_size_em: f32,
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
	pub font_regular: FontConfig,
	pub font_italic: FontConfig,
	pub font_bold: FontConfig,

	pub font_size: f32,
	pub line_height: f32,

	pub h1: H1TextConfig,
	pub h2: H2TextConfig,
	pub h3: H3TextConfig,
	pub h4: H4TextConfig,
	pub h5: H5TextConfig,
	pub padding: PaddingConfig,
}

#[derive(Debug, Deserialize)]
pub struct Paths {
	pub cache_path: PathBuf,
	pub config_path: PathBuf,
	pub data_path: PathBuf,
}
