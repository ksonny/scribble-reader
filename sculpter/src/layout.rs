use std::collections::BTreeMap;

use fixed::types::I26F6;

use crate::error::SculpterScaleError;
use crate::shape::GlyphPlan;
use crate::shape::ShapeFaceRef;

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

#[derive(Debug)]
enum BreakpointType {
	Newline,
	Wordbreak,
}

#[derive(Debug)]
struct Breakpoint {
	distance: u32,
	t: BreakpointType,
}

#[derive(Debug)]
struct SculpterBreakpoints<I: Iterator<Item = (usize, char)>> {
	iter: I,
	last_brk: usize,
}

impl<I: Iterator<Item = (usize, char)>> Iterator for SculpterBreakpoints<I> {
	type Item = Breakpoint;

	fn next(&mut self) -> Option<Self::Item> {
		for (idx, ch) in &mut self.iter {
			if ch.is_whitespace() {
				let distance = (idx - self.last_brk) as u32;
				self.last_brk = idx;
				return Some(Breakpoint {
					distance,
					t: if ch == '\n' {
						BreakpointType::Newline
					} else {
						BreakpointType::Wordbreak
					},
				});
			}
		}
		None
	}
}

#[derive(Debug)]
struct Line<'a> {
	glyphs: &'a [GlyphPlan],
}

#[derive(Debug)]
struct LinesResult<'a> {
	lines: &'a [Line<'a>],
	used_glyphs: usize,
}

#[derive(Debug)]
struct SculpterLines<'a> {
	lines: Vec<Line<'a>>,
}

impl<'a> SculpterLines<'a> {
	fn fill_lines<'b, IBreakpoints: Iterator<Item = &'b Breakpoint>>(
		&'a mut self,
		glyphs: &'a [GlyphPlan],
		lines_widths: &[u32],
		breakpoints: &mut IBreakpoints,
	) -> LinesResult<'a> {
		let lines_start = self.lines.len();

		let mut bps = breakpoints.peekable();
		let mut start = 0;
		let mut current = 0;
		for max_width in lines_widths
			.iter()
			.map(|max_width| I26F6::from_num(*max_width))
		{
			let mut line_width = I26F6::ZERO;
			if let Some(bp) = bps.peek() {
				let breakpoint = current + bp.distance as usize;
				let word_width = glyphs[current..breakpoint]
					.iter()
					.map(|g| g.pos.x_advance)
					.sum::<I26F6>();
				if line_width + word_width > max_width {
					self.lines.push(Line {
						glyphs: &glyphs[start..current],
					});
					line_width = I26F6::ZERO;
					start = current;
				}
				bps.next();
				current = breakpoint + 1;
				let breakpoint_width = glyphs[current].pos.x_advance;
				line_width += word_width + breakpoint_width;
			} else if start < current {
				// Handle partial line
				self.lines.push(Line {
					glyphs: &glyphs[start..current],
				});
			}
		}

		LinesResult {
			lines: &self.lines[lines_start..],
			used_glyphs: current,
		}
	}

	fn clear(&mut self) {
		self.lines.clear();
	}
}
