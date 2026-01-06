use fixed::traits::ToFixed;
use fixed::types::I26F6;

use crate::shaper::GlyphPlan;

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
pub struct Line<'a> {
	pub(crate) glyphs: &'a [GlyphPlan],
}

#[derive(Debug)]
struct LinesResult<'a> {
	lines: &'a [Line<'a>],
	used_glyphs: usize,
	rest: &'a [GlyphPlan],
}

#[derive(Debug, Default)]
struct ShapeLayouter<'a> {
	lines: Vec<Line<'a>>,
}

impl<'a> ShapeLayouter<'a> {
	pub(crate) fn fill_lines<
		'b,
		ILines: Iterator<Item: ToFixed>,
		IBreakpoints: Iterator<Item = &'b Breakpoint>,
	>(
		&'a mut self,
		glyphs: &'a [GlyphPlan],
		lines_widths: ILines,
		breakpoints: &mut IBreakpoints,
	) -> LinesResult<'a> {
		let lines_start = self.lines.len();

		let mut bps = breakpoints.peekable();
		let (mut start, mut current) = (0, 0);
		for max_width in lines_widths.map(|w| w.to_fixed::<I26F6>()) {
			let mut line_width = I26F6::ZERO;
			while let Some(bp) = bps.peek() {
				let breakpoint = current + bp.distance as usize;
				let word_width = glyphs[current..breakpoint]
					.iter()
					.map(|g| g.pos.x_advance)
					.sum::<I26F6>();
				if line_width + word_width > max_width {
					self.lines.push(Line {
						glyphs: &glyphs[start..current],
					});
					start = current;
					break;
				}
				current = breakpoint + 1;
				let breakpoint_width = glyphs[current].pos.x_advance;
				line_width += word_width + breakpoint_width;
				bps.next();
			}
			if start < current {
				// Handle partial line
				self.lines.push(Line {
					glyphs: &glyphs[start..current],
				});
			}
		}

		LinesResult {
			lines: &self.lines[lines_start..],
			used_glyphs: current,
			rest: &glyphs[current..],
		}
	}

	fn clear(&mut self) {
		self.lines.clear();
	}
}
