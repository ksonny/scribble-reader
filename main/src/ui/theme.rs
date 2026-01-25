use egui::Color32;
use egui::FontFamily;
use egui::FontId;
use egui::TextStyle;
use lazy_static::lazy_static;

pub const DEFAULT_SIZE: f32 = 14.0;
pub const S_SIZE: f32 = 12.0;
pub const M_SIZE: f32 = 18.0;
pub const L_SIZE: f32 = 24.0;
pub const XL_SIZE: f32 = 48.0;
pub const ACCENT_COLOR: Color32 = Color32::DARK_RED;

lazy_static! {
	pub static ref ICON_FONT_FAMILY: FontFamily = FontFamily::Name("lucide-icons".into());
	pub static ref ICON_FONT: FontId = FontId::new(DEFAULT_SIZE, ICON_FONT_FAMILY.clone());
	pub static ref ICON_L_FONT: FontId = FontId::new(L_SIZE, ICON_FONT_FAMILY.clone());
	pub static ref ICON_XL_FONT: FontId = FontId::new(XL_SIZE, ICON_FONT_FAMILY.clone());
	pub static ref ICON_STYLE: TextStyle = TextStyle::Name("ICON_STYLE".into());
	pub static ref ICON_L_STYLE: TextStyle = TextStyle::Name("ICON_L_STYLE".into());
	pub static ref ICON_XL_STYLE: TextStyle = TextStyle::Name("ICON_XL_STYLE".into());
	pub static ref HEADING2: TextStyle = TextStyle::Name("HEADING2".into());
}
