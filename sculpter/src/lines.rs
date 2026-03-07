use fixed::types::I26F6;

use crate::Style;
use crate::shaper::BreakpointType;
use crate::shaper::GlyphPlan;

#[derive(Debug, Clone)]
pub(crate) struct StyledGlyphs<'a> {
	pub(crate) glyphs: &'a [GlyphPlan],
	styles: &'a [Style],
	offset: usize,
	cursor: usize,
}

impl<'a> StyledGlyphs<'a> {
	fn new(offset: usize, glyphs: &'a [GlyphPlan], styles: &'a [Style]) -> Self {
		Self {
			glyphs,
			styles,
			offset,
			cursor: 0,
		}
	}

	pub(crate) fn height_decider_style(self) -> Option<&'a Style> {
		let (line_style, _) = self.reduce(|acc @ (max_s, _), sg @ (s, _)| {
			if max_s.font_size < s.font_size {
				sg
			} else {
				acc
			}
		})?;
		Some(line_style)
	}
}

impl<'a> ExactSizeIterator for StyledGlyphs<'a> {
	fn len(&self) -> usize {
		self.glyphs.len() - self.cursor
	}
}

impl<'a> Iterator for StyledGlyphs<'a> {
	type Item = (&'a Style, &'a GlyphPlan);

	fn next(&mut self) -> Option<Self::Item> {
		let glyph = self.glyphs.get(self.cursor)?;
		let style = self
			.styles
			.iter()
			.find(|s| s.end_index > self.offset + self.cursor)?;

		self.cursor += 1;
		Some((style, glyph))
	}
}

#[derive(Debug, Clone)]
pub(crate) struct ShapeLines<'a> {
	max_line_width: I26F6,
	glyphs: &'a [GlyphPlan],
	styles: &'a [Style],
	offset: usize,
	cursor: usize,
}

impl<'a> ShapeLines<'a> {
	pub(crate) fn new(
		offset: usize,
		glyphs: &'a [GlyphPlan],
		styles: &'a [Style],
		max_line_width: I26F6,
	) -> Self {
		Self {
			max_line_width,
			glyphs,
			styles,
			offset,
			cursor: 0,
		}
	}
}

impl<'a> Iterator for ShapeLines<'a> {
	type Item = StyledGlyphs<'a>;

	fn next(&mut self) -> Option<Self::Item> {
		let glyphs = self.glyphs;
		let glyphs_len = glyphs.len();
		let max_line_width = self.max_line_width;
		let px_per_pt = I26F6::lit("96") / I26F6::lit("72");

		if glyphs[self.cursor..].is_empty() {
			return None;
		}

		let used = self.cursor;
		let mut line_width = I26F6::ZERO;

		while self.cursor < glyphs_len {
			let (br_idx, br, br_width) = StyledGlyphs::new(
				self.offset + self.cursor,
				&glyphs[self.cursor..],
				self.styles,
			)
			.enumerate()
			.find_map(|(idx, (s, g))| {
				(!matches!(g.br, BreakpointType::No)).then_some((
					self.cursor + idx,
					g.br,
					(g.pos.x_advance + g.pos.x_offset) * s.font_scale * px_per_pt,
				))
			})
			.unwrap_or((glyphs_len, BreakpointType::No, I26F6::ZERO));

			if matches!(br, BreakpointType::Newline) {
				self.cursor = (br_idx + 1).min(glyphs_len);
				return Some(StyledGlyphs::new(
					self.offset + used,
					&glyphs[used..br_idx],
					self.styles,
				));
			}

			let word_width = StyledGlyphs::new(
				self.offset + self.cursor,
				&glyphs[self.cursor..br_idx],
				self.styles,
			)
			.map(|(s, g)| (g.pos.x_offset + g.pos.x_advance) * s.font_scale * px_per_pt)
			.sum::<I26F6>();
			if line_width + word_width > max_line_width {
				return Some(StyledGlyphs::new(
					self.offset + used,
					&glyphs[used..self.cursor],
					self.styles,
				));
			}
			line_width += word_width + br_width;
			self.cursor = (br_idx + 1).min(glyphs_len);
		}

		if used < self.cursor {
			Some(StyledGlyphs::new(
				self.offset + used,
				&glyphs[used..self.cursor],
				self.styles,
			))
		} else {
			None
		}
	}
}
