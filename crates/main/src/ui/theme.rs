use std::sync::LazyLock;

use egui::Color32;
use egui::FontFamily;
use egui::FontId;
use egui::TextStyle;

pub const DEFAULT_SIZE: f32 = 14.0;
pub const S_SIZE: f32 = 14.0;
pub const M_SIZE: f32 = 18.0;
pub const L_SIZE: f32 = 24.0;
pub const XL_SIZE: f32 = 48.0;
pub const CLOCK_SIZE: f32 = 16.0;
pub const SECONDARY_COLOR: Color32 = Color32::GRAY;

pub struct Icons {
	pub icons_font_family: FontFamily,
	pub icons_font: FontId,
	pub icons_style: TextStyle,
	pub icons_style_large: TextStyle,
	pub icons_style_xlarge: TextStyle,
	pub heading2: TextStyle,
}

pub static TYPOGRAPHY: LazyLock<Icons> = LazyLock::new(|| {
	let font_family = FontFamily::Name("lucide-icons".into());
	Icons {
		icons_font_family: font_family.clone(),
		icons_font: FontId::new(DEFAULT_SIZE, font_family.clone()),
		icons_style: TextStyle::Name("ICON_STYLE".into()),
		icons_style_large: TextStyle::Name("ICON_L_STYLE".into()),
		icons_style_xlarge: TextStyle::Name("ICON_XL_STYLE".into()),
		heading2: TextStyle::Name("HEADING2".into()),
	}
});
