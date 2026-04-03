use std::fmt::Display;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Range;

use ab_glyph::VariableFont;
use fixed::types::I26F6;
use ttf_parser::Tag;

use crate::fonts::FontEntry;
pub use crate::fonts::SculpterFontErrors;
pub use crate::fonts::SculpterFonts;
pub use crate::fonts::SculpterFontsBuilder;
use crate::lines::StyledLines;
pub use crate::printer::AtlasImage;
use crate::printer::SculpterPrinter;
use crate::shaper::GlyphPlan;
use crate::shaper::SculptureShaper;
use crate::shaper::ShapeFaceRef;

mod fonts;
mod lines;
mod printer;
mod shaper;

pub type Fixed = I26F6;

const PX_PER_PT: I26F6 = I26F6::lit("96").strict_div(I26F6::lit("72"));
const PT_PER_PX: I26F6 = I26F6::lit("72").strict_div(I26F6::lit("96"));

#[derive(Debug, Default, Hash)]
pub enum Family<'a> {
	Name(&'a str),
	#[default]
	Serif,
	SansSerif,
}

impl Display for Family<'_> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Family::Name(family) => write!(f, "{}", family),
			Family::Serif => write!(f, "serif"),
			Family::SansSerif => write!(f, "sans-serif"),
		}
	}
}

#[derive(Debug, Clone, Copy, Hash)]
pub enum Axis {
	Wght,
	Wdth,
	Ital,
	Slnt,
	Opzs,
}

impl Axis {
	fn as_bytes(&self) -> &'static [u8; 4] {
		match self {
			Axis::Wght => b"wght",
			Axis::Wdth => b"wdth",
			Axis::Ital => b"ital",
			Axis::Slnt => b"slnt",
			Axis::Opzs => b"opzs",
		}
	}
}

impl From<Axis> for Tag {
	fn from(value: Axis) -> Self {
		Tag::from_bytes(value.as_bytes())
	}
}

impl Display for Axis {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Axis::Wght => write!(f, "wght"),
			Axis::Wdth => write!(f, "wdth"),
			Axis::Ital => write!(f, "ital"),
			Axis::Slnt => write!(f, "slnt"),
			Axis::Opzs => write!(f, "opzs"),
		}
	}
}

#[derive(Debug, Hash)]
pub struct Variation {
	pub axis: Axis,
	pub value: Fixed,
}

impl Variation {
	pub fn new(axis: Axis, value: Fixed) -> Self {
		Self { axis, value }
	}
}

#[derive(Debug)]
pub struct FontOptions<'a> {
	pub family: Family<'a>,
	pub variations: Vec<Variation>,
}

impl<'a> FontOptions<'a> {
	pub fn new(family: Family<'a>, variations: Vec<Variation>) -> Self {
		Self { family, variations }
	}
}

impl Hash for FontOptions<'_> {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.family.hash(state);
		for v in &self.variations {
			v.axis.hash(state);
			v.value.hash(state);
		}
	}
}

impl Display for FontOptions<'_> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "[{}", self.family)?;
		if !self.variations.is_empty() {
			write!(f, " v:")?;
			let mut iter = self.variations.iter().peekable();
			while let Some(v) = iter.next() {
				write!(f, "{}={}", v.axis, v.value)?;
				if iter.peek().is_some() {
					write!(f, ",")?;
				}
			}
		}
		write!(f, "]")?;
		Ok(())
	}
}

#[derive(Debug)]
pub struct FontStyle<'a> {
	pub font_opts: &'a FontOptions<'a>,
	pub font_size: Fixed,
	pub line_height_em: Fixed,
}

#[derive(Debug)]
pub struct SculpterOptions {
	/// Optimize atlas usage by masking out sub pixel bits
	///
	/// Only fraction bits are used.
	pub atlas_sub_pixel_mask: I26F6,
}

impl Default for SculpterOptions {
	fn default() -> Self {
		Self {
			atlas_sub_pixel_mask: I26F6::from_bits(!0),
		}
	}
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

pub fn create_sculpter<'a>(
	fonts: &'a SculpterFonts,
	font_options: &[&FontOptions<'_>],
	options: SculpterOptions,
) -> Result<Sculpter<'a>, SculpterCreateError> {
	let mut shaper = SculptureShaper::new();
	let mut printer = SculpterPrinter::new([8192; 2]);

	let mut faces = Vec::with_capacity(font_options.len());
	for option in font_options {
		let font = fonts
			.find_font(option)
			.ok_or(SculpterCreateError::NoFontFound(option.family.to_string()))?;
		let mut shaper_face =
			rustybuzz::Face::from_face(ttf_parser::Face::parse(&font.data, font.font_index)?);
		let mut printer_font =
			ab_glyph::FontRef::try_from_slice_and_index(&font.data, font.font_index)?;

		for v in &option.variations {
			if shaper_face
				.set_variation(v.axis.into(), v.value.to_num())
				.is_none()
			{
				log::warn!(
					"Font {} does not have variable axis {}",
					option.family,
					v.axis
				);
			}
			printer_font.set_variation(v.axis.as_bytes(), v.value.to_num());
		}

		let shaper_ref = shaper.add(shaper_face, false);
		let printer_ref = printer.add(printer_font);
		debug_assert_eq!(shaper_ref, printer_ref, "Missmatched face ref");

		let hash = {
			let mut s = DefaultHasher::new();
			option.hash(&mut s);
			s.finish()
		};
		faces.push(SculpterFace {
			hash,
			face_ref: shaper_ref,
			font,
		});
	}

	for font in fonts.font_fallbacks() {
		let shaper_face =
			rustybuzz::Face::from_face(ttf_parser::Face::parse(&font.data, font.font_index)?);
		let printer_font =
			ab_glyph::FontRef::try_from_slice_and_index(&font.data, font.font_index)?;

		let shaper_ref = shaper.add(shaper_face, true);
		let printer_ref = printer.add(printer_font);
		debug_assert_eq!(shaper_ref, printer_ref, "Missmatched face ref");
	}

	Ok(Sculpter {
		faces,
		shaper,
		printer,
		glyphs: Vec::new(),
		#[cfg(debug_assertions)]
		glyph_set_id: 0,
		styles: Vec::new(),
		options,
	})
}

#[derive(Debug)]
pub struct SculpterInput<'a> {
	pub style: FontStyle<'a>,
	pub input: &'a str,
}

#[derive(Debug)]
pub struct SculpterHandle {
	glyphs_start: usize,
	glyphs_end: usize,
	#[cfg(debug_assertions)]
	glyph_set_id: u32,
}

impl SculpterHandle {
	pub fn is_empty(&self) -> bool {
		self.glyph_range().is_empty()
	}

	pub fn glyph_range(&self) -> Range<usize> {
		self.glyphs_start..self.glyphs_end
	}
}

#[derive(Debug)]
struct Style {
	face_ref: ShapeFaceRef,
	font_size: I26F6,
	font_scale: I26F6,
	line_height_em: I26F6,
	end_index: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SculpterShapeError {
	#[error(transparent)]
	FaceParsing(#[from] ttf_parser::FaceParsingError),
	#[error("Face not found")]
	FaceNotFound,
}

pub struct SculpterFace<'font> {
	hash: u64,
	face_ref: ShapeFaceRef,
	font: &'font FontEntry,
}

pub struct Sculpter<'font> {
	faces: Vec<SculpterFace<'font>>,
	shaper: SculptureShaper<'font>,
	printer: SculpterPrinter<'font>,
	glyphs: Vec<GlyphPlan>,
	#[cfg(debug_assertions)]
	glyph_set_id: u32,
	styles: Vec<Style>,
	options: SculpterOptions,
}

impl Sculpter<'_> {
	pub fn shape<'input>(
		&mut self,
		inputs: impl Iterator<Item = SculpterInput<'input>>,
	) -> Result<SculpterHandle, SculpterShapeError> {
		let glyphs_start = self.glyphs.len();
		for SculpterInput { style, input } in inputs {
			let font_opts = style.font_opts;
			let font_size = style.font_size;
			let line_height_em = style.line_height_em;

			let font_opts_h = {
				let mut s = DefaultHasher::new();
				font_opts.hash(&mut s);
				s.finish()
			};
			let (face_ref, units_per_em) = self
				.faces
				.iter()
				.find_map(
					|SculpterFace {
					     hash,
					     face_ref,
					     font,
					     ..
					 }| { (*hash == font_opts_h).then_some((*face_ref, font.units_per_em)) },
				)
				.ok_or(SculpterShapeError::FaceNotFound)?;

			self.shaper.shape(face_ref, input, &mut self.glyphs)?;

			self.styles.push(Style {
				face_ref,
				font_size,
				font_scale: font_size / units_per_em,
				line_height_em,
				end_index: self.glyphs.len(),
			});
		}
		let glyphs_end = self.glyphs.len();

		Ok(SculpterHandle {
			glyph_set_id: self.glyph_set_id,
			glyphs_start,
			glyphs_end,
		})
	}
}

#[derive(Debug)]
pub struct MeasureResult {
	pub height: I26F6,
	pub lines: u32,
}

impl Sculpter<'_> {
	pub fn measure(
		&self,
		handle: &SculpterHandle,
		width_px: u32,
		empty_line_height_px: I26F6,
	) -> MeasureResult {
		debug_assert_eq!(
			self.glyph_set_id, handle.glyph_set_id,
			"Glyph set missmatch"
		);

		let empty_line_height = empty_line_height_px.round();

		let mut measure_height = I26F6::ZERO;
		let mut lines = 0;
		let lines_iter = StyledLines::new(
			handle.glyphs_start,
			&self.styles,
			&self.glyphs[handle.glyph_range()],
			I26F6::from_num(width_px) * PT_PER_PX,
		);
		for line in lines_iter {
			if line.glyphs.is_empty() {
				measure_height += empty_line_height;
				continue;
			}

			let line_style = line
				.height_decider_style()
				.expect("Should never happen, no style for line with glyphs");

			let font_height = line_style.font_size * PX_PER_PT;
			measure_height += font_height;

			let line_space = font_height * (line_style.line_height_em - I26F6::ONE);
			// Round to nearest pixel
			measure_height = (measure_height + line_space).round();
			lines += 1;
		}

		MeasureResult {
			height: measure_height,
			lines,
		}
	}
}

#[derive(Debug)]
pub struct DisplayGlyph {
	pub pos: [f32; 2],
	pub size: [f32; 2],
	pub uv_pos: [u32; 2],
	pub uv_size: [u32; 2],
}

#[derive(Debug)]
pub struct TextBlock {
	pub block_height: I26F6,
	pub glyphs: Vec<DisplayGlyph>,
}

#[derive(Debug, thiserror::Error)]
pub enum SculpterPrinterError {
	#[error(transparent)]
	FaceParsing(#[from] ttf_parser::FaceParsingError),
	#[error("Face not found")]
	FaceNotFound,
	#[error("Font size outside range: {0}")]
	FontSizeOutsideRange(f32),
	#[error("Failed to grow atlas")]
	GrowAtlasFailed,
	#[error("Failed to resize atlas texture")]
	ResizeAtlasTextureFailed,
}

impl Sculpter<'_> {
	pub fn render_block(
		&mut self,
		handle: &mut SculpterHandle,
		width_px: u32,
		height_px: u32,
		empty_line_height_px: I26F6,
	) -> Result<TextBlock, SculpterPrinterError> {
		debug_assert_eq!(
			self.glyph_set_id, handle.glyph_set_id,
			"Glyph set missmatch"
		);

		let empty_line_height = empty_line_height_px.round();

		let mut output = Vec::new();
		let mut block_height = I26F6::ZERO;
		let lines_iter = StyledLines::new(
			handle.glyphs_start,
			&self.styles,
			&self.glyphs[handle.glyph_range()],
			I26F6::from_num(width_px) * PT_PER_PX,
		);
		for line in lines_iter {
			if block_height + empty_line_height > height_px {
				break;
			}
			if line.glyphs.is_empty() {
				log::info!("empty line, end {}", line.end());
				handle.glyphs_start = line.end();
				block_height += empty_line_height;
				continue;
			}

			let line_style = line
				.clone()
				.height_decider_style()
				.expect("Should never happen, no style for line with glyphs");

			let font_height = line_style.font_size * PX_PER_PT;
			if block_height + font_height > height_px {
				break;
			}
			block_height += font_height;

			let x_origin = I26F6::ZERO;
			let y_origin = block_height;

			handle.glyphs_start = line.end();
			self.printer
				.print_line(x_origin, y_origin, line, &mut output, &self.options)?;

			let line_space = font_height * (line_style.line_height_em - I26F6::ONE);
			if block_height + line_space > height_px {
				break;
			}
			// Round to nearest pixel
			block_height = (block_height + line_space).round();
		}

		Ok(TextBlock {
			block_height,
			glyphs: output,
		})
	}
}

impl Sculpter<'_> {
	pub fn clear_glyphs(self) -> Self {
		let Self {
			faces,
			shaper,
			printer,
			mut glyphs,
			#[cfg(debug_assertions)]
			glyph_set_id,
			mut styles,
			options,
		} = self;

		glyphs.clear();
		styles.clear();

		Self {
			faces,
			shaper,
			printer,
			glyphs,
			#[cfg(debug_assertions)]
			glyph_set_id: glyph_set_id + 1,
			styles,
			options,
		}
	}

	pub fn write_glyph_atlas(
		&mut self,
		atlas: &mut AtlasImage,
	) -> Result<(), SculpterPrinterError> {
		self.printer.write_glyph_atlas(atlas)
	}
}
