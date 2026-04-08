use std::io;
use std::path::Path;
use std::sync::Arc;

use config::Map;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigEditError {
	#[error("at {1}: {0}")]
	Io(io::Error, &'static std::panic::Location<'static>),
}

impl From<io::Error> for ConfigEditError {
	#[track_caller]
	fn from(err: std::io::Error) -> Self {
		Self::Io(err, std::panic::Location::caller())
	}
}

pub(crate) const DEFAULT_SCRIBE_CONFIG: &str = r#"
[library]
path = "~/Documents/ebooks"

[illustrator.serif]
name = "Serif"
font_size = 16.0
line_height = 1.5
h1 = { font_size_em = 1.8, padding_em = 1.5 }
h2 = { font_size_em = 1.4, padding_em = 1.5 }
h3 = { font_size_em = 1.2, padding_em = 1.5 }
h4 = { font_size_em = 1.0, padding_em = 1.5 }
h5 = { font_size_em = 1.0, padding_em = 1.5 }

[illustrator.default.font_regular]
family = "serif"
variation.wght = 400

[illustrator.default.font_italic]
family = "serif"
variation.wght = 400
variation.ital = 1.0

[illustrator.default.font_bold]
family = "serif"
variation.wght = 600

[illustrator.default.padding]
top_em = 2.0
left_em = 2.0
right_em = 2.0
bottom_em = 2.0
paragraph_em = 1.2

[illustrator.sansserif]
name = "Sans Serif"
font_size = 16.0
line_height = 1.5
h1 = { font_size_em = 1.8, padding_em = 1.5 }
h2 = { font_size_em = 1.4, padding_em = 1.5 }
h3 = { font_size_em = 1.2, padding_em = 1.5 }
h4 = { font_size_em = 1.0, padding_em = 1.5 }
h5 = { font_size_em = 1.0, padding_em = 1.5 }

[illustrator.default.font_regular]
family = "sansserif"
variation.wght = 400

[illustrator.default.font_italic]
family = "sansserif"
variation.wght = 400
variation.ital = 1.0

[illustrator.default.font_bold]
family = "sansserif"
variation.wght = 600

[illustrator.default.padding]
top_em = 2.0
left_em = 2.0
right_em = 2.0
bottom_em = 2.0
paragraph_em = 1.2
"#;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScribeConfig {
	pub library: Library,
	pub illustrator: IllustratorConfig,
}

impl ScribeConfig {
	pub fn load(config_path: &Path) -> Result<Self, config::ConfigError> {
		let config_path = config_path.join("config.toml");
		let config_builder = config::Config::builder()
			.add_source(config::File::from_str(
				DEFAULT_SCRIBE_CONFIG,
				config::FileFormat::Toml,
			))
			.add_source(config::File::from(config_path.as_path()).required(false))
			.add_source(config::Environment::with_prefix("SCRAPE").separator("_"));
		config_builder.build()?.try_deserialize()
	}
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Library {
	pub path: Arc<String>,
}

impl Default for Library {
	fn default() -> Self {
		Self {
			path: Arc::new("~/Documents/ebooks".to_string()),
		}
	}
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IllustratorConfig(Arc<Map<String, Arc<IllustratorProfile>>>);

impl AsRef<Map<String, Arc<IllustratorProfile>>> for IllustratorConfig {
	fn as_ref(&self) -> &Map<String, Arc<IllustratorProfile>> {
		&self.0
	}
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IllustratorProfile {
	pub name: String,

	#[serde(default = "default_font_regular")]
	pub font_regular: FontConfig,
	#[serde(default = "default_font_italic")]
	pub font_italic: FontConfig,
	#[serde(default = "default_font_bold")]
	pub font_bold: FontConfig,

	#[serde(default = "default_font_size")]
	pub font_size: f32,
	#[serde(default = "default_line_height")]
	pub line_height: f32,

	#[serde(default = "default_h1")]
	pub h1: HeaderConfig,
	#[serde(default = "default_h2")]
	pub h2: HeaderConfig,
	#[serde(default = "default_h3")]
	pub h3: HeaderConfig,
	#[serde(default = "default_h4")]
	pub h4: HeaderConfig,
	#[serde(default = "default_h5")]
	pub h5: HeaderConfig,

	#[serde(default = "default_padding")]
	pub padding: PaddingConfig,
}

impl Default for IllustratorProfile {
	fn default() -> Self {
		Self {
			name: Default::default(),
			font_regular: default_font_regular(),
			font_italic: default_font_italic(),
			font_bold: default_font_bold(),
			font_size: default_font_size(),
			line_height: default_line_height(),
			h1: default_h1(),
			h2: default_h2(),
			h3: default_h3(),
			h4: default_h4(),
			h5: default_h5(),
			padding: default_padding(),
		}
	}
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
pub struct HeaderConfig {
	pub font_size_em: f32,
	pub padding_em: f32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PaddingConfig {
	pub top_em: f32,
	pub left_em: f32,
	pub right_em: f32,
	pub bottom_em: f32,
	pub paragraph_em: f32,
}

fn default_font_regular() -> FontConfig {
	FontConfig {
		family: "serif".to_string(),
		variation: FontVariationConfig {
			wght: Some(400.),
			wdth: None,
			ital: None,
			slnt: None,
			opzs: None,
		},
	}
}

fn default_font_italic() -> FontConfig {
	FontConfig {
		family: "serif".to_string(),
		variation: FontVariationConfig {
			wght: Some(400.),
			wdth: None,
			ital: Some(1.),
			slnt: None,
			opzs: None,
		},
	}
}

fn default_font_bold() -> FontConfig {
	FontConfig {
		family: "serif".to_string(),
		variation: FontVariationConfig {
			wght: Some(600.),
			wdth: None,
			ital: None,
			slnt: None,
			opzs: None,
		},
	}
}

fn default_font_size() -> f32 {
	16.0
}

fn default_line_height() -> f32 {
	1.5
}

fn default_h1() -> HeaderConfig {
	HeaderConfig {
		font_size_em: 1.8,
		padding_em: 1.5,
	}
}

fn default_h2() -> HeaderConfig {
	HeaderConfig {
		font_size_em: 1.4,
		padding_em: 1.5,
	}
}

fn default_h3() -> HeaderConfig {
	HeaderConfig {
		font_size_em: 1.2,
		padding_em: 1.5,
	}
}

fn default_h4() -> HeaderConfig {
	HeaderConfig {
		font_size_em: 1.0,
		padding_em: 1.5,
	}
}

fn default_h5() -> HeaderConfig {
	HeaderConfig {
		font_size_em: 1.0,
		padding_em: 1.5,
	}
}

fn default_padding() -> PaddingConfig {
	PaddingConfig {
		top_em: 2.0,
		left_em: 2.0,
		right_em: 2.0,
		bottom_em: 2.0,
		paragraph_em: 1.2,
	}
}
