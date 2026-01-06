use std::collections::BTreeMap;

use fixed::types::I26F6;
use rustybuzz::shape_with_plan;

use crate::error::SculpterScaleError;
use crate::error::SculpterShapeError;

#[derive(Debug)]
struct GlyphPosition {
	x_advance: I26F6,
	y_advance: I26F6,
	x_offset: I26F6,
	y_offset: I26F6,
}

impl From<&rustybuzz::GlyphPosition> for GlyphPosition {
	fn from(value: &rustybuzz::GlyphPosition) -> Self {
		Self {
			x_advance: I26F6::from_bits(value.x_advance),
			y_advance: I26F6::from_bits(value.y_advance),
			x_offset: I26F6::from_bits(value.x_offset),
			y_offset: I26F6::from_bits(value.x_offset),
		}
	}
}

impl GlyphPosition {
	fn scale(&self, scale: I26F6) -> Self {
		Self {
			x_advance: self.x_advance * scale,
			y_advance: self.y_advance * scale,
			x_offset: self.x_offset * scale,
			y_offset: self.y_offset * scale,
		}
	}
}

#[derive(Debug)]
struct GlyphPlan {
	face_ref: ShapeFaceRef,
	glyph_id: u16,
	pos: GlyphPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShapeFaceRef(u32);

#[derive(Debug)]
pub struct SculpterScaler {
	units_per_em_map: BTreeMap<ShapeFaceRef, I26F6>,
	scale_factor: I26F6,
}

impl SculpterScaler {
	fn new(scale_factor: f32) -> Self {
		Self {
			units_per_em_map: BTreeMap::new(),
			scale_factor: I26F6::from_num(scale_factor),
		}
	}

	fn add(&mut self, face_ref: ShapeFaceRef, face: ttf_parser::Face<'_>) {
		let units_per_em = I26F6::from_bits(face.units_per_em() as i32);
		self.units_per_em_map.insert(face_ref, units_per_em);
	}

	fn scale(&self, font_size: I26F6, glyphs: &mut [GlyphPlan]) -> Result<(), SculpterScaleError> {
		let font_size = font_size * self.scale_factor;

		for glyph in glyphs {
			let units_per_em = self
				.units_per_em_map
				.get(&glyph.face_ref)
				.ok_or(SculpterScaleError::MissingEntryForPlanRef(glyph.face_ref))?;
			let font_scale = font_size / units_per_em;
			glyph.pos = glyph.pos.scale(font_scale);
		}

		Ok(())
	}

	fn clear(&mut self) {
		self.units_per_em_map.clear();
	}
}

struct ShapePlan<'a> {
	face: rustybuzz::Face<'a>,
	plan: Option<rustybuzz::ShapePlan>,
}

pub struct SculpterEmptyState<'a> {
	plans: Vec<ShapePlan<'a>>,
	fallback: Vec<ShapeFaceRef>,
	inputs: Vec<(usize, &'a str)>,
	buffer: rustybuzz::UnicodeBuffer,
}

pub struct SculpterState<'a> {
	plans: Vec<ShapePlan<'a>>,
	fallback: Vec<ShapeFaceRef>,
	inputs: Vec<(usize, &'a str)>,
	buffer: rustybuzz::UnicodeBuffer,
}

impl<'a> SculpterState<'a> {
	fn add_face(&mut self, face: ttf_parser::Face<'a>, fallback: bool) -> ShapeFaceRef {
		let face = rustybuzz::Face::from_face(face);
		let face_ref = ShapeFaceRef(self.plans.len() as u32);
		self.plans.push(ShapePlan { face, plan: None });
		if fallback {
			self.fallback.push(face_ref);
		}
		face_ref
	}

	fn shape<I: IntoIterator<Item = &'a str>>(
		self,
		face_ref: ShapeFaceRef,
		input_iter: I,
		glyphs: &mut Vec<GlyphPlan>,
	) -> Result<SculpterState<'a>, SculpterShapeError> {
		let SculpterState {
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
				pos: pos.into(),
			});
		}

		shape_fallback(
			plans,
			fallback,
			inputs,
			shaped.clear(),
			invalid,
			&mut glyphs[glyphs_start..],
		)
	}
}

fn shape_fallback<'a>(
	mut plans: Vec<ShapePlan<'a>>,
	fallback: Vec<ShapeFaceRef>,
	inputs: Vec<(usize, &'a str)>,
	mut buffer: rustybuzz::UnicodeBuffer,
	mut invalid: Vec<usize>,
	glyphs: &mut [GlyphPlan],
) -> Result<SculpterState<'a>, SculpterShapeError> {
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

		invalid.clear();
		for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
			let idx = info.cluster as usize;
			if info.glyph_id > 0 {
				debug_assert!(glyphs.len() > idx, "Index outside of glyph range");
				debug_assert!(glyphs[idx].glyph_id == 0, "Glyph in array is not invalid");
				glyphs[idx] = GlyphPlan {
					face_ref: *face_ref,
					glyph_id: info.glyph_id as u16,
					pos: pos.into(),
				};
			} else {
				invalid.push(idx);
			}
		}
		buffer = shaped.clear();
	}

	Ok(SculpterState {
		plans,
		fallback,
		inputs,
		buffer,
	})
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
