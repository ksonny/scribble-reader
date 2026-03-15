use std::collections::BTreeMap;

use ab_glyph::Font;
use ab_glyph::GlyphId;
use ab_glyph::OutlinedGlyph;
use ab_glyph::PxScale;
use etagere::Allocation;
use etagere::BucketedAtlasAllocator;
use etagere::Size;
use etagere::size2;
use fixed::types::I26F6;
use fixed::types::U0F8;
use image::GrayImage;
use image::Pixel;

use crate::DisplayGlyph;
use crate::SculpterPrinterError;
use crate::lines::StyledGlyphs;
use crate::shaper::GlyphPlan;
use crate::shaper::ShapeFaceRef;

pub const INITIAL_ATLAS_SIZE: u32 = 512;

#[derive(Debug)]
pub struct AtlasImage(GrayImage);

impl Default for AtlasImage {
	fn default() -> Self {
		Self(GrayImage::new(INITIAL_ATLAS_SIZE, INITIAL_ATLAS_SIZE))
	}
}

impl AtlasImage {
	pub fn height(&self) -> u32 {
		let AtlasImage(image) = self;
		image.height()
	}

	pub fn width(&self) -> u32 {
		let AtlasImage(image) = self;
		image.width()
	}

	pub fn as_raw(&self) -> &[u8] {
		let AtlasImage(image) = self;
		image.as_raw()
	}
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GlyphIdent {
	face_ref: ShapeFaceRef,
	glyph_id: GlyphId,
	font_size: I26F6,
}

impl GlyphIdent {
	fn new(glyph: &GlyphPlan, font_size: I26F6) -> Self {
		Self {
			face_ref: glyph.face_ref,
			glyph_id: GlyphId(glyph.glyph_id),
			font_size,
		}
	}
}

struct GlyphMapEntry {
	alloc: Allocation,
	outline: OutlinedGlyph,
}

pub(crate) struct SculpturePrinter<'a> {
	fonts: Vec<ab_glyph::FontRef<'a>>,
	allocator: BucketedAtlasAllocator,
	glyph_map: BTreeMap<GlyphIdent, Option<GlyphMapEntry>>,
	need_texture_refresh: bool,
	max_texture_2d: Size,
}

impl<'a> SculpturePrinter<'a> {
	const ATLAS_MARGIN: i32 = 1;

	pub(crate) fn new(max_texture_2d: [u32; 2]) -> Self {
		let max_texture_2d = size2(max_texture_2d[0] as i32, max_texture_2d[1] as i32);
		let size = size2(INITIAL_ATLAS_SIZE as i32, INITIAL_ATLAS_SIZE as i32).min(max_texture_2d);
		Self {
			fonts: Vec::new(),
			allocator: BucketedAtlasAllocator::new(size),
			glyph_map: BTreeMap::new(),
			need_texture_refresh: false,
			max_texture_2d,
		}
	}

	pub(crate) fn add(&mut self, font: ab_glyph::FontRef<'a>) -> ShapeFaceRef {
		let face_ref = ShapeFaceRef(self.fonts.len() as u16);
		self.fonts.push(font);
		face_ref
	}

	pub(crate) fn print_line(
		&mut self,
		x_origin: I26F6,
		y_origin: I26F6,
		styled_glyphs: StyledGlyphs<'_>,
		min_render_px: I26F6,
		glyphs: &mut Vec<DisplayGlyph>,
	) -> Result<(), SculpterPrinterError> {
		let px_per_pt = I26F6::lit("96") / I26F6::lit("72");
		let mut x_pos = x_origin;
		for (style, glyph) in styled_glyphs {
			let x_advance = glyph.pos.x_advance * style.font_scale * px_per_pt;
			let x_offset = glyph.pos.x_offset * style.font_scale * px_per_pt;

			let font_size = (style.font_size * px_per_pt).max(min_render_px);
			let ident = GlyphIdent::new(glyph, font_size);
			let entry = if let Some(entry) = self.glyph_map.get(&ident) {
				entry
			} else {
				self.alloc_glyph(ident)?
			};

			if let Some(GlyphMapEntry { alloc, outline }) = entry {
				let bounds = outline.px_bounds();
				let scale = ((style.font_size * px_per_pt) / font_size).to_num::<f32>();

				let x = x_pos + x_offset + I26F6::from_num(bounds.min.x * scale);
				let y = y_origin + I26F6::from_num(bounds.min.y * scale);
				let w = bounds.width() * scale;
				let h = bounds.height() * scale;

				let u = alloc.rectangle.min.x;
				let v = alloc.rectangle.min.y;
				let uv_w = bounds.width();
				let uv_h = bounds.height();

				glyphs.push(DisplayGlyph {
					pos: [x.to_num(), y.to_num()],
					size: [w, h],
					uv_pos: [u as u32, v as u32],
					uv_size: [uv_w as u32, uv_h as u32],
				});
			}
			x_pos += x_advance;
		}
		Ok(())
	}

	fn alloc_glyph(
		&mut self,
		ident: GlyphIdent,
	) -> Result<&Option<GlyphMapEntry>, SculpterPrinterError> {
		let font = &self.fonts[ident.face_ref.0 as usize];

		let units_per_em = font.units_per_em().unwrap();
		let height = font.height_unscaled();
		let scale = PxScale::from(ident.font_size.to_num::<f32>() * height / units_per_em);

		if let Some(outline) = font.outline_glyph(ident.glyph_id.with_scale(scale)) {
			let bounds = outline.px_bounds();
			let margin = Self::ATLAS_MARGIN;
			let size = size2(
				bounds.width() as i32 + margin,
				bounds.height() as i32 + margin,
			);
			let alloc = if let Some(alloc) = self.allocator.allocate(size) {
				alloc
			} else {
				let incr = size2(INITIAL_ATLAS_SIZE as i32, INITIAL_ATLAS_SIZE as i32);
				self.allocator
					.grow((self.allocator.size() + incr).min(self.max_texture_2d));
				self.allocator
					.allocate(size)
					.ok_or(SculpterPrinterError::GrowAtlasFailed)?
			};
			self.need_texture_refresh = true;
			self.glyph_map
				.insert(ident.clone(), Some(GlyphMapEntry { alloc, outline }));
			let entry = self
				.glyph_map
				.get(&ident)
				.expect("Missing entry after insert");
			Ok(entry)
		} else {
			self.glyph_map.insert(ident.clone(), None);
			Ok(&None)
		}
	}

	pub fn write_glyph_atlas(&self, image: &mut AtlasImage) -> Result<(), SculpterPrinterError> {
		let AtlasImage(image) = image;
		if self.need_texture_refresh {
			let atlas_size = self.allocator.size();
			let atlas_width = atlas_size.width as u32;
			let atlas_height = atlas_size.height as u32;

			if image.width() != atlas_width || image.height() != atlas_height {
				let new_len = image::Luma::<u8>::CHANNEL_COUNT as usize
					* (atlas_width * atlas_height) as usize;

				let mut data = std::mem::take(image).into_raw();
				data.resize(new_len, 0u8);
				*image = GrayImage::from_raw(atlas_width, atlas_height, data)
					.ok_or(SculpterPrinterError::ResizeAtlasTextureFailed)?;
			};
			for entry in self.glyph_map.values().flatten() {
				let x0 = entry.alloc.rectangle.min.x as u32;
				let y0 = entry.alloc.rectangle.min.y as u32;
				entry.outline.draw(|x, y, c| {
					let c = U0F8::saturating_from_num(c);
					let c = image::Luma([c.to_bits()]);
					image.put_pixel(x0 + x, y0 + y, c);
				});
			}
		}
		Ok(())
	}
}
