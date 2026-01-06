use std::collections::BTreeMap;

use ab_glyph::Font;
use ab_glyph::GlyphId;
use ab_glyph::OutlinedGlyph;
use etagere::Allocation;
use etagere::BucketedAtlasAllocator;
use etagere::Size;
use etagere::size2;
use fixed::types::I26F6;
use fixed::types::U0F8;
use image::Pixel;

use crate::error::SculpturePrinterError;
use crate::layout::Line;
use crate::shaper::GlyphPlan;
use crate::shaper::ShapeFaceRef;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GlyphIdent {
	face_ref: ShapeFaceRef,
	glyph_id: GlyphId,
	size: I26F6,
}

impl GlyphIdent {
	fn new(font_size: I26F6, glyph: &GlyphPlan) -> Self {
		Self {
			face_ref: glyph.face_ref,
			glyph_id: GlyphId(glyph.glyph_id),
			size: font_size,
		}
	}
}

pub struct DisplayGlyph {
	pos: [f32; 2],
	size: [f32; 2],
	uv_pos: [f32; 2],
	uv_size: [f32; 2],
}

pub struct SculpturePrinter<'a> {
	scale_factor: I26F6,
	fonts: Vec<ab_glyph::FontRef<'a>>,
	allocator: BucketedAtlasAllocator,
	glyph_map: BTreeMap<GlyphIdent, (Allocation, OutlinedGlyph)>,
	need_texture_refresh: bool,
	max_texture_2d: Size,
}

impl<'a> SculpturePrinter<'a> {
	const INITIAL_SIZE: u32 = 1024;

	pub(crate) fn new(scale_factor: I26F6, max_texture_2d: [u32; 2]) -> Self {
		let max_texture_2d = size2(max_texture_2d[0] as i32, max_texture_2d[1] as i32);
		let size = size2(Self::INITIAL_SIZE as i32, Self::INITIAL_SIZE as i32).min(max_texture_2d);
		Self {
			scale_factor,
			fonts: Vec::new(),
			allocator: BucketedAtlasAllocator::new(size),
			glyph_map: BTreeMap::new(),
			need_texture_refresh: false,
			max_texture_2d,
		}
	}

	pub(crate) fn add(&mut self, font: ab_glyph::FontRef<'a>) -> ShapeFaceRef {
		let face_ref = ShapeFaceRef(self.fonts.len() as u32);
		self.fonts.push(font);
		face_ref
	}

	pub fn print_lines(
		&mut self,
		font_size: I26F6,
		line_height_em: I26F6,
		lines: &[Line<'_>],
	) -> Result<Vec<DisplayGlyph>, SculpturePrinterError> {
		let px_per_pt = self.scale_factor * I26F6::from_num(96) / I26F6::from_num(72);
		let line_adv = font_size * line_height_em * px_per_pt;

		let mut output = Vec::new();
		let (mut x_pos, mut y_pos) = (I26F6::ZERO, I26F6::ZERO);
		for line in lines {
			for glyph in line.glyphs {
				let ident = GlyphIdent::new(font_size, glyph);
				let alloc = if let Some((alloc, _)) = self.glyph_map.get(&ident) {
					alloc
				} else {
					self.alloc_glyph(font_size, glyph, ident)?
				};

				let x = x_pos + glyph.pos.x_offset;
				let y = y_pos + glyph.pos.y_offset;
				let w = glyph.pos.x_advance;
				let h = glyph.pos.y_advance;

				let u = alloc.rectangle.min.x;
				let v = alloc.rectangle.min.y;
				let u_w = alloc.rectangle.width();
				let u_h = alloc.rectangle.height();

				output.push(DisplayGlyph {
					pos: [x.to_num(), y.to_num()],
					size: [w.to_num(), h.to_num()],
					uv_pos: [u as f32, v as f32],
					uv_size: [u_w as f32, u_h as f32],
				});
				x_pos = x + w;
			}
			y_pos += line_adv;
		}

		Ok(output)
	}

	fn alloc_glyph(
		&mut self,
		font_size: I26F6,
		glyph: &GlyphPlan,
		ident: GlyphIdent,
	) -> Result<&Allocation, SculpturePrinterError> {
		let font = &self.fonts[glyph.face_ref.0 as usize];
		let scale = font
			.pt_to_px_scale((self.scale_factor * font_size).to_num())
			.ok_or_else(|| SculpturePrinterError::FontSizeOutsideRange(font_size.to_num()))?;
		let outline = font
			.outline_glyph(ident.glyph_id.with_scale(scale))
			.ok_or(SculpturePrinterError::OutlineMissing(glyph.glyph_id))?;
		let bounds = outline.px_bounds();
		let size = size2(bounds.width() as i32, bounds.height() as i32);
		let alloc = if let Some(alloc) = self.allocator.allocate(size) {
			alloc
		} else {
			let incr = size2(Self::INITIAL_SIZE as i32, Self::INITIAL_SIZE as i32);
			self.allocator
				.grow((self.allocator.size() + incr).min(self.max_texture_2d));
			self.allocator
				.allocate(size)
				.ok_or(SculpturePrinterError::GrowAtlasFailed)?
		};
		self.need_texture_refresh = true;
		self.glyph_map.insert(ident.clone(), (alloc, outline));
		let (alloc, _) = self
			.glyph_map
			.get(&ident)
			.expect("Missing entry after insert");
		Ok(alloc)
	}

	pub fn update_glyph_atlas(
		&self,
		image: image::GrayAlphaImage,
	) -> Result<image::GrayAlphaImage, SculpturePrinterError> {
		if self.need_texture_refresh {
			let atlas_size = self.allocator.size();
			let atlas_width = atlas_size.width as u32;
			let atlas_height = atlas_size.height as u32;

			let mut image = if image.width() != atlas_width || image.height() != atlas_height {
				let new_len = image::LumaA::<u8>::CHANNEL_COUNT as usize
					* (atlas_width * atlas_height) as usize;
				let mut data = image.into_raw();
				data.resize(new_len, 0u8);
				image::GrayAlphaImage::from_raw(atlas_width, atlas_height, data)
					.ok_or(SculpturePrinterError::ResizeAtlasTextureFailed)?
			} else {
				image
			};

			for (alloc, outline) in self.glyph_map.values() {
				let x0 = alloc.rectangle.min.x as u32;
				let y0 = alloc.rectangle.min.y as u32;
				outline.draw(|x, y, c| {
					let c = image::LumaA([0u8, 255u8 - U0F8::from_num(c.min(1.0)).to_bits()]);
					image.put_pixel(x0 + x, y0 + y, c);
				});
			}
			Ok(image)
		} else {
			Ok(image)
		}
	}
}
