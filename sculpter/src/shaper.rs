use std::collections::BTreeMap;

use fixed::types::I26F6;
use rustybuzz::shape_with_plan;

use crate::SculpterShapeError;

#[derive(Debug)]
pub(crate) struct GlyphPosition {
	pub(crate) x_advance: I26F6,
	pub(crate) x_offset: I26F6,
	#[allow(unused)]
	pub(crate) y_advance: I26F6,
	#[allow(unused)]
	pub(crate) y_offset: I26F6,
}

impl From<&rustybuzz::GlyphPosition> for GlyphPosition {
	fn from(value: &rustybuzz::GlyphPosition) -> Self {
		Self {
			x_advance: I26F6::from_bits(value.x_advance),
			x_offset: I26F6::from_bits(value.x_offset),
			y_advance: I26F6::from_bits(value.y_advance),
			y_offset: I26F6::from_bits(value.y_offset),
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

#[derive(Debug, PartialEq, PartialOrd, Eq, Ord)]
struct PlanKey(ShapeFaceRef, rustybuzz::Script);

pub struct SculptureShaper<'a> {
	faces: Vec<rustybuzz::Face<'a>>,
	plans: BTreeMap<PlanKey, rustybuzz::ShapePlan>,
	fallback: Vec<ShapeFaceRef>,
	buffer: Option<rustybuzz::UnicodeBuffer>,
}

impl<'a> SculptureShaper<'a> {
	pub(crate) fn new() -> Self {
		Self {
			faces: Vec::new(),
			plans: BTreeMap::new(),
			fallback: Vec::new(),
			buffer: None,
		}
	}

	pub(crate) fn add(&mut self, face: rustybuzz::Face<'a>, fallback: bool) -> ShapeFaceRef {
		let face_ref = ShapeFaceRef(self.faces.len() as u16);
		self.faces.push(face);
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
			rustybuzz::BufferFlags::BEGINNING_OF_TEXT & rustybuzz::BufferFlags::END_OF_TEXT,
		);
		buffer.push_str(input);
		buffer.set_direction(rustybuzz::Direction::LeftToRight);
		buffer.guess_segment_properties();
		// Looks a bit stupid, script() defaults to UNKNOWN so may be INVALID
		if buffer.script() == rustybuzz::script::UNKNOWN {
			buffer.set_script(rustybuzz::script::UNKNOWN);
		}

		let (face, shape_plan) =
			get_or_create_shape_plan(&mut self.plans, &self.faces, &buffer, face_ref)?;
		let shaped = shape_with_plan(face, shape_plan, buffer);

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
				pos: pos.into(),
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

			buffer.set_direction(rustybuzz::Direction::LeftToRight);
			buffer.set_script(rustybuzz::script::UNKNOWN);

			for cluster in invalid.keys() {
				let c_idx = input.floor_char_boundary(*cluster as usize);
				let c = input[c_idx..]
					.chars()
					.next()
					.expect("Failed to get original char");
				buffer.add(c, *cluster);
			}

			let (face, shape_plan) =
				get_or_create_shape_plan(&mut self.plans, &self.faces, &buffer, face_ref)?;
			let shaped = shape_with_plan(face, shape_plan, buffer);

			for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
				if info.glyph_id > 0 {
					let idx = invalid
						.remove(&info.cluster)
						.expect("Keys should be preserved");
					debug_assert!(glyphs.len() > idx, "Index outside of glyph range");
					debug_assert!(glyphs[idx].glyph_id == 0, "Glyph in array is not invalid");
					glyphs[idx] = GlyphPlan {
						face_ref,
						glyph_id: info.glyph_id as u16,
						pos: pos.into(),
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

fn get_or_create_shape_plan<'a, 'b>(
	plans: &'a mut BTreeMap<PlanKey, rustybuzz::ShapePlan>,
	faces: &'a [rustybuzz::Face<'b>],
	buffer: &rustybuzz::UnicodeBuffer,
	face_ref: ShapeFaceRef,
) -> Result<(&'a rustybuzz::Face<'b>, &'a rustybuzz::ShapePlan), SculpterShapeError> {
	let face = faces
		.get(face_ref.0 as usize)
		.ok_or(SculpterShapeError::FaceNotFound)?;

	let key = PlanKey(face_ref, buffer.script());
	let plan = plans.entry(key).or_insert_with(|| {
		let dir = buffer.direction();
		let script = Some(buffer.script());
		let language = buffer.language();
		rustybuzz::ShapePlan::new(face, dir, script, language.as_ref(), &[])
	});

	Ok((face, plan))
}
