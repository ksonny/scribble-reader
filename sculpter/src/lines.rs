use fixed::types::I26F6;

use crate::Style;
use crate::shaper::BreakpointType;
use crate::shaper::GlyphPlan;

#[derive(Debug, Clone)]
pub(crate) struct StyledGlyphs<'a> {
	hyphen: bool,
	pub(crate) glyphs: &'a [GlyphPlan],
	styles: &'a [Style],
	offset: usize,
	cursor: usize,
}

impl<'a> StyledGlyphs<'a> {
	fn new(offset: usize, glyphs: &'a [GlyphPlan], styles: &'a [Style]) -> Self {
		Self {
			hyphen: false,
			glyphs,
			styles,
			offset,
			cursor: 0,
		}
	}

	fn with_hyphen(self, hyphen: bool) -> Self {
		let Self {
			glyphs,
			styles,
			offset,
			cursor,
			..
		} = self;
		Self {
			hyphen,
			glyphs,
			styles,
			offset,
			cursor,
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

	pub(crate) fn hyphen_style(&self) -> Option<&'a Style> {
		if self.hyphen {
			let last = self.offset + self.glyphs.len();
			self.styles
				.iter()
				.find(|s| s.end_index > last)
				.or(self.styles.last())
		} else {
			None
		}
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
			.find(|s| s.end_index > self.offset + self.cursor)
			.or(self.styles.last())?;

		self.cursor += 1;
		Some((style, glyph))
	}
}

#[derive(Debug, Clone)]
pub(crate) struct StyledLines<'a> {
	max_line_width: I26F6,
	styles: &'a [Style],
	glyphs: &'a [GlyphPlan],
	offset: usize,
	cursor: usize,
}

impl<'a> StyledLines<'a> {
	pub(crate) fn new(
		offset: usize,
		styles: &'a [Style],
		glyphs: &'a [GlyphPlan],
		max_line_width: I26F6,
	) -> Self {
		Self {
			max_line_width,
			styles,
			glyphs,
			offset,
			cursor: 0,
		}
	}
}

impl<'a> Iterator for StyledLines<'a> {
	type Item = StyledGlyphs<'a>;

	fn next(&mut self) -> Option<Self::Item> {
		if self.glyphs[self.cursor..].is_empty() {
			return None;
		}

		let mut idx = self.cursor;
		let mut segment_width = I26F6::ZERO;
		let mut last_line_break = None;
		let mut iter = StyledGlyphs::new(
			self.offset + self.cursor,
			&self.glyphs[self.cursor..],
			self.styles,
		);
		for (s, g) in &mut iter {
			if matches!(g.br, BreakpointType::Newline) {
				let used = self.cursor;
				self.cursor = idx + 1;
				return Some(StyledGlyphs::new(
					self.offset + used,
					&self.glyphs[used..self.cursor],
					self.styles,
				));
			}
			let width = g.pos.x_advance * s.font_scale;
			if segment_width + width > self.max_line_width {
				let used = self.cursor;
				self.cursor = last_line_break.unwrap_or(idx);
				return Some(
					StyledGlyphs::new(
						self.offset + used,
						&self.glyphs[used..self.cursor],
						self.styles,
					)
					.with_hyphen(last_line_break.is_none() && matches!(g.br, BreakpointType::No)),
				);
			}

			idx += 1;
			segment_width += width;
			if !matches!(g.br, BreakpointType::No) {
				last_line_break = Some(idx);
			}
		}

		let rest = StyledGlyphs::new(
			self.offset + self.cursor,
			&self.glyphs[self.cursor..],
			self.styles,
		);
		self.cursor = idx;
		Some(rest)
	}
}

#[cfg(test)]
mod tests {
	use fixed::types::I26F6;

	use crate::PX_PER_PT;
	use crate::Style;
	use crate::lines::StyledGlyphs;
	use crate::lines::StyledLines;
	use crate::shaper::BreakpointType;
	use crate::shaper::GlyphPlan;
	use crate::shaper::GlyphPosition;

	fn mock_style(end_index: usize) -> Style {
		Style {
			face_ref: crate::shaper::ShapeFaceRef(0),
			font_size: I26F6::ONE,
			font_scale: I26F6::ONE,
			line_height_em: I26F6::ONE,
			end_index,
		}
	}

	fn mock_glyph(size: I26F6, glyph_id: u16, br: BreakpointType) -> GlyphPlan {
		GlyphPlan {
			face_ref: crate::shaper::ShapeFaceRef(0),
			glyph_id,
			pos: GlyphPosition {
				x_advance: size,
				x_offset: I26F6::ZERO,
				y_advance: size,
				y_offset: I26F6::ZERO,
			},
			br,
		}
	}

	#[test]
	fn test_styled_glyphs_styled() {
		let styles = vec![mock_style(usize::MAX)];
		let glyphs = vec![
			mock_glyph(I26F6::ONE, 0, BreakpointType::No),
			mock_glyph(I26F6::ONE, 1, BreakpointType::No),
		];
		let sg = StyledGlyphs::new(0, &glyphs, &styles);

		assert_eq!(sg.len(), 2, "Enexpected glyph count");
		let mut idx = 0;
		for (s, g) in sg {
			assert_eq!(s.face_ref, styles[0].face_ref);
			assert_eq!(g.face_ref, glyphs[idx].face_ref);
			assert_eq!(g.glyph_id, glyphs[idx].glyph_id);
			idx += 1;
		}
		assert_eq!(idx, 2);
	}

	#[test]
	fn test_styled_glyphs_style_fallback_if_exceeded() {
		// Not enough style
		let styles = vec![mock_style(1)];
		let glyphs = vec![
			mock_glyph(I26F6::ONE, 0, BreakpointType::No),
			mock_glyph(I26F6::ONE, 1, BreakpointType::No),
		];
		let sg = StyledGlyphs::new(0, &glyphs, &styles);

		assert_eq!(sg.len(), 2, "Enexpected glyph count");
		let mut idx = 0;
		for (s, g) in sg {
			assert_eq!(s.face_ref, styles[0].face_ref);
			assert_eq!(g.face_ref, glyphs[idx].face_ref);
			assert_eq!(g.glyph_id, glyphs[idx].glyph_id);
			idx += 1;
		}
		// Still iterates
		assert_eq!(idx, 2);
	}

	#[test]
	fn test_lines_single_fits() {
		let styles = vec![mock_style(usize::MAX)];
		let glyphs = vec![
			mock_glyph(I26F6::ONE, 0, BreakpointType::No),
			mock_glyph(I26F6::ONE, 1, BreakpointType::No),
		];

		let lines = StyledLines::new(0, &styles, &glyphs, 3 * PX_PER_PT);
		let mut idx = 0;
		for line in lines {
			idx += 1;
			assert!(!line.hyphen, "Unexpected hyphen in line");
			assert_eq!(line.len(), 2, "Unexpected line length");
		}
		assert_eq!(idx, 1);
	}

	#[test]
	fn test_lines_hyphen_break_perfect() {
		let styles = vec![mock_style(usize::MAX)];
		let glyphs = vec![
			mock_glyph(I26F6::ONE, 0, BreakpointType::No),
			mock_glyph(I26F6::ONE, 1, BreakpointType::No),
		];

		let lines = StyledLines::new(0, &styles, &glyphs, PX_PER_PT);
		let mut idx = 0;
		for line in lines {
			assert_eq!(idx == 0, line.hyphen, "Expected hyphen in first segment");
			idx += 1;
		}
		assert_eq!(idx, 2);
	}

	#[test]
	fn test_lines_hyphen_break_sparse() {
		let styles = vec![mock_style(usize::MAX)];
		let glyphs = vec![
			mock_glyph(I26F6::ONE, 0, BreakpointType::No),
			mock_glyph(I26F6::ONE, 1, BreakpointType::No),
			mock_glyph(I26F6::ONE, 2, BreakpointType::No),
		];

		let lines = StyledLines::new(0, &styles, &glyphs, 2 * PX_PER_PT);
		let mut idx = 0;
		for line in lines {
			assert_eq!(idx == 0, line.hyphen, "Expected hyphen in first segment");
			idx += 1;
		}
		assert_eq!(idx, 2);
	}

	#[test]
	fn test_lines_hyphen_break_newline() {
		let styles = vec![mock_style(usize::MAX)];
		let glyphs = vec![
			mock_glyph(I26F6::ONE, 0, BreakpointType::No),
			mock_glyph(I26F6::ONE, 1, BreakpointType::Newline),
			mock_glyph(I26F6::ONE, 2, BreakpointType::No),
		];

		let lines = StyledLines::new(0, &styles, &glyphs, 20 * PX_PER_PT);
		assert_eq!(lines.count(), 2);
	}
}
