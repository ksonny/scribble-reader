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

	pub(crate) fn end(&self) -> usize {
		self.offset + self.cursor + self.glyphs.len()
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
		max_line_width_px: I26F6,
	) -> Self {
		let pt_per_px = I26F6::lit("72") / I26F6::lit("96");
		let max_line_width = max_line_width_px * pt_per_px;

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
		if self.glyphs[self.cursor..].is_empty() {
			return None;
		}

		let mut idx = self.cursor;
		let mut segment_width = I26F6::ZERO;
		let mut last_break = None;

		let rest = StyledGlyphs::new(
			self.offset + self.cursor,
			&self.glyphs[self.cursor..],
			self.styles,
		);
		for (s, g) in rest.clone() {
			let width = g.pos.x_advance * s.font_scale;
			if segment_width + width > self.max_line_width {
				let used = self.cursor;
				self.cursor = last_break.unwrap_or(idx);
				return Some(StyledGlyphs::new(
					self.offset + used,
					&self.glyphs[used..self.cursor],
					self.styles,
				));
			}

			idx += 1;
			segment_width += width;
			if !matches!(g.br, BreakpointType::No) {
				last_break = Some(idx);
			}
		}
		self.cursor = idx;
		Some(rest)
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn test_basic() {
		todo!()
	}
}
