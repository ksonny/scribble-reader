use std::collections::BTreeMap;

use ab_glyph::Font;
use ab_glyph::GlyphId;
use ab_glyph::OutlinedGlyph;
use ab_glyph::Point;
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
use crate::SculpterOptions;
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
struct GlyphKey {
	face_ref: ShapeFaceRef,
	glyph_id: GlyphId,
	font_size: I26F6,
	sub_pixel: I26F6,
}

impl GlyphKey {
	fn new(
		glyph: &GlyphPlan,
		font_size: I26F6,
		sub_pixel: I26F6,
		options: &SculpterOptions,
	) -> Self {
		let sub_pixel = sub_pixel & options.atlas_sub_pixel_mask;

		Self {
			face_ref: glyph.face_ref,
			glyph_id: GlyphId(glyph.glyph_id),
			font_size,
			sub_pixel,
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
	glyph_map: BTreeMap<GlyphKey, Option<GlyphMapEntry>>,
	write_queue: Vec<GlyphKey>,
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
			write_queue: Vec::new(),
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
		glyphs: &mut Vec<DisplayGlyph>,
		options: &SculpterOptions,
	) -> Result<(), SculpterPrinterError> {
		let px_per_pt = I26F6::lit("96") / I26F6::lit("72");
		let mut x_pos = x_origin;
		for (style, glyph) in styled_glyphs {
			let x_advance = glyph.pos.x_advance * style.font_scale * px_per_pt;
			let x_offset = glyph.pos.x_offset * style.font_scale * px_per_pt;
			let y_offset = glyph.pos.y_offset * style.font_scale * px_per_pt;

			let font_size = style.font_size * px_per_pt;
			let sub_pixel = (x_pos + x_offset).frac();
			let key = GlyphKey::new(glyph, font_size, sub_pixel, options);
			let entry = if let Some(entry) = self.glyph_map.get(&key) {
				entry
			} else {
				self.alloc_glyph(key)?
			};

			if let Some(GlyphMapEntry { alloc, outline }) = entry {
				let bounds = outline.px_bounds();

				let x = x_pos + x_offset - sub_pixel + I26F6::from_num(bounds.min.x);
				let y = y_origin + y_offset + I26F6::from_num(bounds.min.y);
				let w = bounds.width();
				let h = bounds.height();

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
		key: GlyphKey,
	) -> Result<&Option<GlyphMapEntry>, SculpterPrinterError> {
		let font = &self.fonts[key.face_ref.0 as usize];

		let units_per_em = font.units_per_em().unwrap();
		let height = font.height_unscaled();
		let scale = PxScale::from(key.font_size.to_num::<f32>() * height / units_per_em);

		let pos = Point {
			x: key.sub_pixel.to_num(),
			y: 0.,
		};

		if let Some(outline) = font.outline_glyph(key.glyph_id.with_scale_and_position(scale, pos))
		{
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
			self.glyph_map
				.insert(key.clone(), Some(GlyphMapEntry { alloc, outline }));
			let entry = self
				.glyph_map
				.get(&key)
				.expect("Missing entry after insert");
			self.write_queue.push(key);
			Ok(entry)
		} else {
			self.glyph_map.insert(key, None);
			Ok(&None)
		}
	}

	pub fn write_glyph_atlas(
		&mut self,
		image: &mut AtlasImage,
	) -> Result<(), SculpterPrinterError> {
		if !self.write_queue.is_empty() {
			let AtlasImage(image) = image;

			if log::log_enabled!(log::Level::Debug) {
				self.log_atlas_stats(log::Level::Debug);
			}

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

				// Glyph resize, refresh all glyphs
				for entry in self.glyph_map.values().flatten() {
					let x0 = entry.alloc.rectangle.min.x as u32;
					let y0 = entry.alloc.rectangle.min.y as u32;
					entry.outline.draw(|x, y, c| {
						let c = U0F8::saturating_from_num(c);
						let c = image::Luma([c.to_bits()]);
						image.put_pixel(x0 + x, y0 + y, c);
					});
				}
				self.write_queue.clear();
			} else {
				for key in self.write_queue.drain(..) {
					let Some(Some(entry)) = self.glyph_map.get(&key) else {
						continue;
					};

					let x0 = entry.alloc.rectangle.min.x as u32;
					let y0 = entry.alloc.rectangle.min.y as u32;
					entry.outline.draw(|x, y, c| {
						let c = U0F8::saturating_from_num(c);
						let c = image::Luma([c.to_bits()]);
						image.put_pixel(x0 + x, y0 + y, c);
					});
				}
			}
		}
		Ok(())
	}

	fn log_atlas_stats(&self, level: log::Level) {
		let mut cnt = 0;
		let mut repeat = BTreeMap::new();
		let mut size_repeat = BTreeMap::new();
		for k in self.glyph_map.keys() {
			cnt += 1;
			let e: &mut usize = repeat.entry((k.face_ref, k.glyph_id)).or_default();
			*e += 1;
			let e: &mut usize = size_repeat
				.entry((k.face_ref, k.glyph_id, k.font_size))
				.or_default();
			*e += 1;
		}

		let atlas_size = self.allocator.size();
		let atlas_avail = I26F6::from_num(atlas_size.width * atlas_size.height);
		let atlas_used = I26F6::from_num(self.allocator.allocated_space());
		let atlas_perc = atlas_used / atlas_avail;
		log::log!(level, "Atlas entries: {}", cnt);
		log::log!(
			level,
			"Atlas size {}x{}, {} used",
			atlas_size.width,
			atlas_size.height,
			atlas_perc
		);

		let mut face_rev_glyph = BTreeMap::new();
		let repeat = {
			let mut v = repeat.into_iter().collect::<Vec<_>>();
			v.sort_by_key(|(_, n)| *n);
			v
		};
		log::log!(level, "Glyphs repeated {}:", repeat.len());
		for (i, ((face_ref, glyph_id), n)) in repeat.into_iter().rev().take(10).enumerate() {
			let i = i + 1;
			let rev_glyph = face_rev_glyph.entry(face_ref).or_insert_with(|| {
				let font = &self.fonts[face_ref.0 as usize];
				font.codepoint_ids().collect::<BTreeMap<_, _>>()
			});
			let c = rev_glyph.get(&glyph_id).unwrap_or(&' ');
			log::log!(
				level,
				"{i}. f{} g{:04} '{}' {}",
				face_ref.0,
				glyph_id.0,
				c,
				n
			);
		}

		let size_repeat = {
			let mut v = size_repeat.into_iter().collect::<Vec<_>>();
			v.sort_by_key(|(_, n)| *n);
			v
		};
		log::log!(level, "Glyphs size repeated {}:", size_repeat.len());
		for (i, ((face_ref, glyph_id, font_size), n)) in
			size_repeat.into_iter().rev().take(10).enumerate()
		{
			let i = i + 1;
			let rev_glyph = face_rev_glyph.entry(face_ref).or_insert_with(|| {
				let font = &self.fonts[face_ref.0 as usize];
				font.codepoint_ids().collect::<BTreeMap<_, _>>()
			});
			let c = rev_glyph.get(&glyph_id).unwrap_or(&' ');
			log::log!(
				level,
				"{i}. f{} g{:04} s{} '{}' {}",
				face_ref.0,
				glyph_id.0,
				font_size,
				c,
				n
			);
		}
	}
}
