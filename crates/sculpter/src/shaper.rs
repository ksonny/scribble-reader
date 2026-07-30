use std::collections::BTreeMap;

use fixed::types::I26F6;

use crate::SculpterShapeError;
use crate::Variation;

#[derive(Debug)]
pub(crate) struct GlyphPosition {
	pub(crate) x_advance: I26F6,
	pub(crate) x_offset: I26F6,
	#[allow(unused)]
	pub(crate) y_advance: I26F6,
	#[allow(unused)]
	pub(crate) y_offset: I26F6,
}

impl GlyphPosition {
	fn from(value: &harfrust::GlyphPosition, em_per_unit: I26F6) -> Self {
		Self {
			x_advance: I26F6::from_bits(value.x_advance) * em_per_unit,
			x_offset: I26F6::from_bits(value.x_offset) * em_per_unit,
			y_advance: I26F6::from_bits(value.y_advance) * em_per_unit,
			y_offset: I26F6::from_bits(value.y_offset) * em_per_unit,
		}
	}
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) enum BreakpointType {
	#[default]
	No,
	Newline,
	Wordbreak,
}

pub struct GlyphPlan {
	pub(crate) face_ref: ShapeFaceRef,
	pub(crate) glyph_id: u16,
	pub(crate) pos: GlyphPosition,
	pub(crate) br: BreakpointType,
}

impl std::fmt::Debug for GlyphPlan {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("GlyphPlan")
			.field("face_ref", &self.face_ref)
			.field("glyph_id", &self.glyph_id)
			.field("br", &self.br)
			.finish()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShapeFaceRef(pub(crate) u16);

struct SculpterFace<'font> {
	face: harfrust::FontRef<'font>,
	shaper_data: &'font harfrust::ShaperData,
	shaper_instance: Option<harfrust::ShaperInstance>,
	em_per_unit: I26F6,
}

pub struct SculptureShaper<'font> {
	faces: Vec<SculpterFace<'font>>,
	fallback: Vec<ShapeFaceRef>,
	buffer: Option<harfrust::UnicodeBuffer>,
}

impl<'font> SculptureShaper<'font> {
	pub(crate) fn new() -> Self {
		Self {
			faces: Vec::new(),
			fallback: Vec::new(),
			buffer: None,
		}
	}

	pub(crate) fn add(
		&mut self,
		face: harfrust::FontRef<'font>,
		shaper_data: &'font harfrust::ShaperData,
		variations: Option<&[Variation]>,
		units_per_em: I26F6,
		fallback: bool,
	) -> ShapeFaceRef {
		let face_ref = ShapeFaceRef(self.faces.len() as u16);
		let shaper_instance =
			variations.map(|vs| harfrust::ShaperInstance::from_variations(&face, vs));
		self.faces.push(SculpterFace {
			face,
			shaper_data,
			shaper_instance,
			em_per_unit: I26F6::ONE / units_per_em,
		});
		if fallback {
			self.fallback.push(face_ref);
		}
		face_ref
	}

	pub fn shape(
		&mut self,
		face_ref: ShapeFaceRef,
		input: &str,
		glyphs: &mut Vec<GlyphPlan>,
	) -> Result<usize, SculpterShapeError> {
		let mut buffer = self.buffer.take().unwrap_or_default();
		buffer.set_flags(
			harfrust::BufferFlags::BEGINNING_OF_TEXT & harfrust::BufferFlags::END_OF_TEXT,
		);
		buffer.push_str(input);
		buffer.set_direction(harfrust::Direction::LeftToRight);
		buffer.guess_segment_properties();
		// Looks a bit stupid, script() defaults to UNKNOWN so may be INVALID
		if buffer.script() == harfrust::script::UNKNOWN {
			buffer.set_script(harfrust::script::UNKNOWN);
		}

		let face = self
			.faces
			.get(face_ref.0 as usize)
			.ok_or(SculpterShapeError::FaceNotFound)?;
		let shaper = face
			.shaper_data
			.shaper(&face.face)
			.instance(face.shaper_instance.as_ref())
			.build();
		let shaped = shaper.shape(buffer, harfrust::ShapeOptions::new());
		let glyphs_start = glyphs.len();
		let glyphs_added = shaped.len();
		glyphs.reserve(shaped.len());

		let mut invalid = BTreeMap::new();
		for (idx, (info, pos)) in shaped
			.glyph_infos()
			.iter()
			.zip(shaped.glyph_positions())
			.enumerate()
		{
			let i = input.floor_char_boundary(info.cluster as usize);
			let c = input[i..]
				.chars()
				.next()
				.expect("Failed to get original char");

			if info.glyph_id == 0 && !c.is_whitespace() {
				invalid.insert(info.cluster, idx);
			}

			let br = if c == '\n' {
				BreakpointType::Newline
			} else if c.is_whitespace() {
				BreakpointType::Wordbreak
			} else {
				BreakpointType::No
			};

			glyphs.push(GlyphPlan {
				face_ref,
				glyph_id: info.glyph_id as u16,
				pos: GlyphPosition::from(pos, face.em_per_unit),
				br,
			});
		}

		self.buffer.replace(shaped.clear());

		self.shape_fallback(input, &mut glyphs[glyphs_start..], invalid)?;

		Ok(glyphs_added)
	}

	fn shape_fallback(
		&mut self,
		input: &str,
		glyphs: &mut [GlyphPlan],
		mut invalid: BTreeMap<u32, usize>,
	) -> Result<(), SculpterShapeError> {
		let mut buffer = self
			.buffer
			.take()
			.expect("Buffer should always be available from shape()");

		for face_ref in self.fallback.iter().cloned() {
			if invalid.is_empty() {
				break;
			}

			buffer.set_direction(harfrust::Direction::LeftToRight);
			buffer.set_script(harfrust::script::UNKNOWN);

			for cluster in invalid.keys() {
				let c_idx = input.floor_char_boundary(*cluster as usize);
				let c = input[c_idx..]
					.chars()
					.next()
					.expect("Failed to get original char");
				buffer.add(c, *cluster);
			}

			let face = self
				.faces
				.get(face_ref.0 as usize)
				.ok_or(SculpterShapeError::FaceNotFound)?;
			// TODO: Cache shaper too?
			let shaper = face
				.shaper_data
				.shaper(&face.face)
				.instance(face.shaper_instance.as_ref())
				.build();
			let shaped = shaper.shape(buffer, harfrust::ShapeOptions::new());

			for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
				if info.glyph_id > 0 {
					log::debug!(
						"Found fallback for cluster {}, width {}",
						info.cluster,
						pos.x_advance
					);

					let idx = invalid
						.remove(&info.cluster)
						.expect("Keys should be preserved");
					debug_assert!(glyphs.len() > idx, "Index outside of glyph range");
					debug_assert!(glyphs[idx].glyph_id == 0, "Glyph in array is not invalid");
					glyphs[idx] = GlyphPlan {
						face_ref,
						glyph_id: info.glyph_id as u16,
						pos: GlyphPosition::from(pos, face.em_per_unit),
						br: glyphs[idx].br,
					};
				}
			}
			buffer = shaped.clear();
		}

		self.buffer.replace(buffer);

		if !invalid.is_empty() && log::log_enabled!(log::Level::Debug) {
			let s = invalid
				.keys()
				.map(|cluster| {
					let c_idx = input.floor_char_boundary(*cluster as usize);
					input[c_idx..]
						.chars()
						.next()
						.expect("Failed to get original char")
				})
				.collect::<String>();
			log::debug!("Failed to shape {} glyphs: '{}'", invalid.len(), s);
		}

		Ok(())
	}
}
