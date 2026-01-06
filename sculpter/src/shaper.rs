use fixed::types::I26F6;
use rustybuzz::shape_with_plan;

use crate::error::SculpterShapeError;

#[derive(Debug)]
pub(crate) struct GlyphPosition {
	pub(crate) x_advance: I26F6,
	pub(crate) y_advance: I26F6,
	pub(crate) x_offset: I26F6,
	pub(crate) y_offset: I26F6,
}

impl GlyphPosition {
	pub(crate) fn from(pos: &rustybuzz::GlyphPosition, glyph_scale: I26F6) -> Self {
		Self {
			x_advance: I26F6::from_bits(pos.x_advance) * glyph_scale,
			y_advance: I26F6::from_bits(pos.y_advance) * glyph_scale,
			x_offset: I26F6::from_bits(pos.x_offset) * glyph_scale,
			y_offset: I26F6::from_bits(pos.y_offset) * glyph_scale,
		}
	}
}

#[derive(Debug)]
pub struct GlyphPlan {
	pub(crate) face_ref: ShapeFaceRef,
	pub(crate) glyph_id: u16,
	pub(crate) pos: GlyphPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShapeFaceRef(pub(crate) u32);

struct ScaledGlyphs<'a> {
	font_size: I26F6,
	glyphs: &'a [GlyphPlan],
}

struct ShapePlan<'a> {
	face: rustybuzz::Face<'a>,
	plan: Option<rustybuzz::ShapePlan>,
}

pub struct SculptureShaper<'a> {
	scale_factor: I26F6,
	plans: Vec<ShapePlan<'a>>,
	fallback: Vec<ShapeFaceRef>,
	inputs: Vec<(usize, &'a str)>,
	buffer: rustybuzz::UnicodeBuffer,
}

impl<'a> SculptureShaper<'a> {
	pub(crate) fn new(scale_factor: I26F6) -> Self {
		Self {
			scale_factor,
			plans: Vec::new(),
			fallback: Vec::new(),
			inputs: Vec::new(),
			buffer: rustybuzz::UnicodeBuffer::new(),
		}
	}

	pub(crate) fn add(&mut self, face: rustybuzz::Face<'a>, fallback: bool) -> ShapeFaceRef {
		let face_ref = ShapeFaceRef(self.plans.len() as u32);
		self.plans.push(ShapePlan { face, plan: None });
		if fallback {
			self.fallback.push(face_ref);
		}
		face_ref
	}

	pub fn shape<I: Iterator<Item = &'a str>>(
		self,
		face_ref: ShapeFaceRef,
		font_size: I26F6,
		input_iter: I,
		glyphs: &mut Vec<GlyphPlan>,
	) -> Result<SculptureShaper<'a>, SculpterShapeError> {
		let SculptureShaper {
			scale_factor,
			mut plans,
			fallback,
			mut inputs,
			mut buffer,
		} = self;

		inputs.clear();
		for str in input_iter {
			let start = buffer.len();
			inputs.push((start, str));
			buffer.push_str(str);
		}
		buffer.set_direction(rustybuzz::Direction::LeftToRight);
		buffer.guess_segment_properties();

		let (face, shape_plan) = get_or_create_shape_plan(&mut plans, &buffer, face_ref)?;
		let shaped = shape_with_plan(face, shape_plan, buffer);
		let glyph_scale = (scale_factor * font_size) / I26F6::from_bits(face.units_per_em());

		let glyphs_start = glyphs.len();
		glyphs.reserve(shaped.len());

		let mut invalid = Vec::new();
		for (idx, (info, pos)) in shaped
			.glyph_infos()
			.iter()
			.zip(shaped.glyph_positions())
			.enumerate()
		{
			if info.glyph_id == 0 {
				invalid.push(idx);
			}
			glyphs.push(GlyphPlan {
				face_ref,
				glyph_id: info.glyph_id as u16,
				pos: GlyphPosition::from(pos, glyph_scale),
			});
		}

		SculptureShaper {
			scale_factor,
			plans,
			fallback,
			inputs,
			buffer: shaped.clear(),
		}
		.shape_fallback(font_size, invalid, &mut glyphs[glyphs_start..])
	}

	fn shape_fallback(
		self,
		font_size: I26F6,
		mut invalid: Vec<usize>,
		glyphs: &mut [GlyphPlan],
	) -> Result<SculptureShaper<'a>, SculpterShapeError> {
		let SculptureShaper {
			scale_factor,
			mut plans,
			fallback,
			inputs,
			mut buffer,
		} = self;

		let px_per_pt = self.scale_factor * I26F6::from_num(96) / I26F6::from_num(72);

		for face_ref in &fallback {
			if invalid.is_empty() {
				break;
			}

			for idx in &invalid {
				let invalid_c = inputs.iter().find_map(|(offset, s)| {
					if offset < idx {
						s.chars().nth(idx - offset)
					} else {
						None
					}
				});
				if let Some(c) = invalid_c {
					buffer.add(c, *idx as u32);
				} else {
					log::warn!("Failed to find char for invalid idx {idx}");
				}
			}

			let (face, shape_plan) = get_or_create_shape_plan(&mut plans, &buffer, *face_ref)?;
			let shaped = shape_with_plan(face, shape_plan, buffer);
			let scale = px_per_pt * font_size / I26F6::from_bits(face.units_per_em());

			invalid.clear();
			for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
				let idx = info.cluster as usize;
				if info.glyph_id > 0 {
					debug_assert!(glyphs.len() > idx, "Index outside of glyph range");
					debug_assert!(glyphs[idx].glyph_id == 0, "Glyph in array is not invalid");
					glyphs[idx] = GlyphPlan {
						face_ref: *face_ref,
						glyph_id: info.glyph_id as u16,
						pos: GlyphPosition::from(pos, scale),
					};
				} else {
					invalid.push(idx);
				}
			}
			buffer = shaped.clear();
		}

		Ok(SculptureShaper {
			scale_factor,
			plans,
			fallback,
			inputs,
			buffer,
		})
	}
}

fn get_or_create_shape_plan<'a, 'b>(
	plans: &'a mut Vec<ShapePlan<'b>>,
	buffer: &rustybuzz::UnicodeBuffer,
	face_ref: ShapeFaceRef,
) -> Result<(&'a rustybuzz::Face<'b>, &'a rustybuzz::ShapePlan), SculpterShapeError> {
	let plan = plans
		.get_mut(face_ref.0 as usize)
		.ok_or(SculpterShapeError::UnknownShapePlan(face_ref))?;
	let shape_plan = if let Some(ref plan) = plan.plan {
		plan
	} else {
		let dir = buffer.direction();
		let script = Some(buffer.script());
		let language = buffer.language();
		let shape_plan = rustybuzz::ShapePlan::new(&plan.face, dir, script, language.as_ref(), &[]);
		plan.plan = Some(shape_plan);
		plan.plan.as_ref().unwrap()
	};
	Ok((&plan.face, shape_plan))
}
