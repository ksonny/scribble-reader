#![allow(dead_code)]
use std::collections::HashMap;

use etagere::Allocation;
use etagere::BucketedAtlasAllocator;
use etagere::size2;
use fixed::types::I10F6;
use fixed::types::I26F6;
use skrifa::GlyphId16;
use skrifa::MetadataProvider;
use skrifa::OutlineGlyphCollection;

use crate::SculpterFontStack;
use crate::SculpterOptions;
use crate::ShapeFaceRef;
use crate::shaper::GlyphPlan;

pub const INITIAL_ATLAS_SIZE: u32 = 512;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct GlyphKey {
	face_ref: ShapeFaceRef,
	glyph_id: GlyphId16,
	font_size: u16,
	sub_pixel: I10F6,
}

impl GlyphKey {
	fn new(glyph: &GlyphPlan, font_size: I26F6, x_pos: I26F6, opts: &SculpterOptions) -> Self {
		todo!()
	}
}

struct SculpterRasterFace<'font> {
	outlines: OutlineGlyphCollection<'font>,
	location: skrifa::instance::Location,
}

struct SculpterRaster<'font> {
	faces: Vec<SculpterRasterFace<'font>>,
	max_texture_2d: etagere::Size,
	allocator: BucketedAtlasAllocator,
	glyphs: HashMap<GlyphKey, Allocation>,
	queue: Vec<GlyphKey>,
}

impl<'font> SculpterRaster<'font> {
	pub(crate) fn new(stack: &SculpterFontStack<'font>, max_texture_2d: [u32; 2]) -> Self {
		let mut faces = Vec::new();

		for entry in stack.entries() {
			let outlines = entry.font.outline_glyphs();
			let location = entry.location.clone();

			faces.push(SculpterRasterFace { outlines, location })
		}

		let max_texture_2d = size2(max_texture_2d[0] as i32, max_texture_2d[1] as i32);
		let size = size2(INITIAL_ATLAS_SIZE as i32, INITIAL_ATLAS_SIZE as i32).min(max_texture_2d);

		Self {
			faces,
			max_texture_2d,
			allocator: BucketedAtlasAllocator::new(size),
			glyphs: HashMap::new(),
			queue: Vec::new(),
		}
	}
}
