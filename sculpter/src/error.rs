#[derive(Debug, thiserror::Error)]
pub enum SculpterLoadError {
	#[error(transparent)]
	FaceParsing(#[from] ttf_parser::FaceParsingError),
}

#[derive(Debug, thiserror::Error)]
pub enum SculpterShapeError {
	#[error("Unknown shape plan: {0:?}")]
	UnknownShapePlan(crate::shaper::ShapeFaceRef),
	#[error("Create rustybuzz face failed")]
	CreateRustybuzzFaceFailed,
}

#[derive(Debug, thiserror::Error)]
pub enum SculpterCreateError {
	#[error(transparent)]
	FaceParsing(#[from] ttf_parser::FaceParsingError),
	#[error("No font found with family name {0}")]
	NoFontFound(String),
	#[error(transparent)]
	InvalidFont(#[from] ab_glyph::InvalidFont),
}

#[derive(Debug, thiserror::Error)]
pub enum SculpturePrinterError {
	#[error("Font size outside range: {0}")]
	FontSizeOutsideRange(f32),
	#[error("Outline missing for glyph_id {0}")]
	OutlineMissing(u16),
	#[error("Failed to grow atlas")]
	GrowAtlasFailed,
	#[error("Failed to resize atlas texture")]
	ResizeAtlasTextureFailed,
}
