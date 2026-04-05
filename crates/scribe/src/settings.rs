use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;

#[cfg(not(target_os = "android"))]
pub(crate) const DEFAULT_SCRIBE_CONFIG: &str = r#"
[library]
path = "~/Documents/ebooks"

[illustrator]
font_size = 16.0
line_height = 1.5
h1 = { font_size_em = 1.8 }
h2 = { font_size_em = 1.4 }
h3 = { font_size_em = 1.2 }
h4 = { font_size_em = 1.0 }
h5 = { font_size_em = 1.0 }

[illustrator.font_regular]
family = "serif"
variation = { wght = 400 }

[illustrator.font_italic]
family = "serif"
variation = { wght = 400, ital = 1 }

[illustrator.font_bold]
family = "serif"
variation = { wght = 600 }

[illustrator.padding]
top_em = 2.0
left_em = 2.0
right_em = 2.0
bottom_em = 2.0
paragraph_em = 1.2
"#;

#[cfg(target_os = "android")]
pub(crate) const DEFAULT_SCRIBE_CONFIG: &str = r#"
[library]

[illustrator]
font_size = 16.0
line_height = 1.5
h1 = { font_size_em = 1.8 }
h2 = { font_size_em = 1.4 }
h3 = { font_size_em = 1.2 }
h4 = { font_size_em = 1.0 }
h5 = { font_size_em = 1.0 }

[illustrator.font_regular]
family = "serif"
variation = { wght = 400 }

[illustrator.font_italic]
family = "serif"
variation = { wght = 400, ital = 1 }

[illustrator.font_bold]
family = "serif"
variation = { wght = 600 }

[illustrator.padding]
top_em = 2.0
left_em = 2.0
right_em = 2.0
bottom_em = 2.0
paragraph_em = 1.2
"#;

#[derive(Debug, Deserialize, Serialize)]
pub struct Library {
	pub path: Option<Arc<String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct FontVariationConfig {
	pub wght: Option<f32>,
	pub wdth: Option<f32>,
	pub ital: Option<f32>,
	pub slnt: Option<f32>,
	pub opzs: Option<f32>,
}

#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Debug, Deserialize, Serialize)]
pub struct H1TextConfig {
	pub font_size_em: f32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct H2TextConfig {
	pub font_size_em: f32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct H3TextConfig {
	pub font_size_em: f32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct H4TextConfig {
	pub font_size_em: f32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct H5TextConfig {
	pub font_size_em: f32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PaddingConfig {
	pub top_em: f32,
	pub left_em: f32,
	pub right_em: f32,
	pub bottom_em: f32,

	pub paragraph_em: f32,
}

#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Debug, Deserialize, Serialize)]
pub struct Settings {
	pub library: Library,
	pub illustrator: Illustrator,
}

#[derive(Debug)]
pub struct Paths {
	pub cache_path: Arc<PathBuf>,
	pub config_path: Arc<PathBuf>,
	pub data_path: Arc<PathBuf>,
}
