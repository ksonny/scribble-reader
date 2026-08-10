use std::borrow::Cow;
use std::collections::BTreeMap;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use ab_glyph::VariableFont;
use fixed::types::I26F6;
use language_tags::LanguageTag;
use read_fonts::TableProvider;
use skrifa::MetadataProvider;
use skrifa::instance::Location;

use crate::Axis;
use crate::Family;
use crate::FontOptions;
use crate::shaper::ShapeFaceRef;

pub(crate) struct FontEntry {
	pub(crate) hash: u64,
	pub(crate) families: Vec<(String, Option<LanguageTag>)>,
	pub(crate) italic: bool,
	pub(crate) shaper_data: harfrust::ShaperData,
	pub(crate) data: Cow<'static, [u8]>,
	pub(crate) font_index: u32,
}

impl FontEntry {
	fn has(&self, family_name: &str) -> bool {
		self.families
			.iter()
			.any(|(family, _)| family == family_name)
	}
}

pub(crate) struct FontFallback {
	pub(crate) shaper_data: harfrust::ShaperData,
	pub(crate) data: Cow<'static, [u8]>,
	pub(crate) font_index: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SculpterFontErrors {
	#[error(transparent)]
	ReadFonts(#[from] read_fonts::ReadError),
}

pub struct SculpterFontsBuilder {
	fonts: BTreeMap<u64, FontEntry>,
	font_fallbacks: Vec<FontFallback>,
	family_serif: Cow<'static, str>,
	family_sans_serif: Cow<'static, str>,
}

impl SculpterFontsBuilder {
	pub fn new<S: Into<Cow<'static, str>>>(family_serif: S, family_sans_serif: S) -> Self {
		Self {
			fonts: BTreeMap::new(),
			font_fallbacks: Vec::new(),
			family_serif: family_serif.into(),
			family_sans_serif: family_sans_serif.into(),
		}
	}

	pub fn add_font<D: Into<Cow<'static, [u8]>>>(
		self,
		data: D,
		font_index: u32,
	) -> Result<Self, SculpterFontErrors> {
		let Self {
			mut fonts,
			font_fallbacks,
			family_serif,
			family_sans_serif,
		} = self;

		let e = create_font_entry(data, font_index)?;
		fonts.insert(e.hash, e);

		Ok(Self {
			fonts,
			font_fallbacks,
			family_serif,
			family_sans_serif,
		})
	}

	pub fn add_fallback<D: Into<Cow<'static, [u8]>>>(
		self,
		data: D,
		font_index: u32,
	) -> Result<Self, SculpterFontErrors> {
		let Self {
			fonts,
			mut font_fallbacks,
			family_serif,
			family_sans_serif,
		} = self;

		let e = create_font_fallback(data, font_index)?;
		font_fallbacks.push(e);

		Ok(Self {
			fonts,
			font_fallbacks,
			family_serif,
			family_sans_serif,
		})
	}

	pub fn build(self) -> SculpterFonts {
		let Self {
			fonts,
			font_fallbacks,
			family_serif,
			family_sans_serif,
		} = self;
		SculpterFonts(Arc::new(SculpterFontsInner {
			fonts,
			font_fallbacks,
			family_serif,
			family_sans_serif,
		}))
	}
}

struct SculpterFontsInner {
	fonts: BTreeMap<u64, FontEntry>,
	font_fallbacks: Vec<FontFallback>,
	family_serif: Cow<'static, str>,
	family_sans_serif: Cow<'static, str>,
}

#[derive(Clone)]
pub struct SculpterFonts(Arc<SculpterFontsInner>);

impl SculpterFonts {
	pub(crate) fn find_font<'a>(&'a self, fo: &FontOptions<'_>) -> Option<&'a FontEntry> {
		let family_name = match fo.family {
			Family::Name(s) => s,
			Family::Serif => &self.0.family_serif,
			Family::SansSerif => &self.0.family_sans_serif,
		};
		let italic = fo.variations.iter().any(|v| matches!(v.axis, Axis::Ital));
		let font = self
			.0
			.fonts
			.values()
			.find(|e| e.italic == italic && e.has(family_name))?;
		Some(font)
	}

	pub(crate) fn font_fallbacks(&self) -> &[FontFallback] {
		&self.0.font_fallbacks
	}
}

fn create_font_entry<D: Into<Cow<'static, [u8]>>>(
	d: D,
	font_index: u32,
) -> Result<FontEntry, SculpterFontErrors> {
	let data = d.into();
	let mut s = DefaultHasher::new();
	data.hash(&mut s);
	let hash = s.finish();

	let face = read_fonts::FontRef::from_index(&data, font_index)?;
	let families = collect_families(&face)?;

	let attrs = face.attributes();
	let italic = match attrs.style {
		skrifa::attribute::Style::Normal => false,
		skrifa::attribute::Style::Italic => true,
		skrifa::attribute::Style::Oblique(angle) => angle.is_some_and(|a| a != 0.),
	};

	let shaper_data = harfrust::ShaperData::new(&face);

	Ok(FontEntry {
		hash,
		families,
		italic,
		shaper_data,
		data,
		font_index,
	})
}

fn create_font_fallback<D: Into<Cow<'static, [u8]>>>(
	d: D,
	font_index: u32,
) -> Result<FontFallback, SculpterFontErrors> {
	let data = d.into();
	let face = read_fonts::FontRef::from_index(&data, font_index)?;
	let shaper_data = harfrust::ShaperData::new(&face);

	Ok(FontFallback {
		shaper_data,
		data,
		font_index,
	})
}

fn collect_families(
	face: &read_fonts::FontRef<'_>,
) -> Result<Vec<(String, Option<LanguageTag>)>, read_fonts::ReadError> {
	let mut families = Vec::new();

	families.extend(
		face.localized_strings(skrifa::string::StringId::TYPOGRAPHIC_FAMILY_NAME)
			.map(|name| {
				(
					name.to_string(),
					name.language().and_then(|l| LanguageTag::parse(l).ok()),
				)
			}),
	);

	if families.is_empty() {
		families.extend(
			face.localized_strings(skrifa::string::StringId::FAMILY_NAME)
				.map(|name| {
					(
						name.to_string(),
						name.language().and_then(|l| LanguageTag::parse(l).ok()),
					)
				}),
		);
	}

	// Promote "best" name to first
	let en_us = LanguageTag::parse("en-US").unwrap();
	let en = LanguageTag::parse("en").unwrap();
	let mut best_rank = 0;
	let mut best_index = 0;
	for (i, s) in families.iter().enumerate() {
		let rank = match s {
			(_, Some(l)) if l == &en_us => 3,
			(_, Some(l)) if l == &en => 2,
			(_, None) => 1,
			_ => continue,
		};
		if rank > best_rank {
			best_rank = rank;
			best_index = i;
		}
	}
	if best_index != 0 {
		families.swap(0, best_index);
	}

	Ok(families)
}

pub(crate) struct FontStackEntry<'font> {
	pub(crate) face_ref: ShapeFaceRef,
	pub(crate) font: harfrust::FontRef<'font>,
	pub(crate) location: Location,
	pub(crate) shaper_data: &'font harfrust::ShaperData,
	pub(crate) shaper_instance: Option<harfrust::ShaperInstance>,
	pub(crate) printer_font: ab_glyph::FontRef<'font>,
	pub(crate) units_per_em: I26F6,
	pub(crate) hash: Option<u64>,
	pub(crate) fallback: bool,
}

pub struct SculpterFontStack<'font> {
	fonts: &'font SculpterFonts,
	stack: Vec<FontStackEntry<'font>>,
}

#[derive(Debug, thiserror::Error)]
pub enum SculpterFontStackError {
	#[error("No font found with family name {0}")]
	NoFontFound(String),
	#[error(transparent)]
	InvalidFont(#[from] ab_glyph::InvalidFont),
	#[error(transparent)]
	ReadFont(#[from] read_fonts::ReadError),
}

impl<'font> SculpterFontStack<'font> {
	pub fn new(fonts: &'font SculpterFonts) -> Self {
		Self {
			fonts,
			stack: Vec::new(),
		}
	}

	pub fn add(
		&mut self,
		font_opts: &FontOptions<'_>,
	) -> Result<ShapeFaceRef, SculpterFontStackError> {
		let entry = self
			.fonts
			.find_font(font_opts)
			.ok_or(SculpterFontStackError::NoFontFound(
				font_opts.family.to_string(),
			))?;
		let face_ref = ShapeFaceRef(self.stack.len() as u16);
		let font = harfrust::FontRef::from_index(&entry.data, entry.font_index)?;
		let location = font.axes().location(font_opts.variations.iter().map(|v| {
			skrifa::setting::VariationSetting::new(
				skrifa::Tag::new(v.axis.as_bytes()),
				v.value.to_num(),
			)
		}));
		let shaper_instance =
			harfrust::ShaperInstance::from_coords(&font, location.coords().iter().cloned());

		let printer_font = {
			let mut f = ab_glyph::FontRef::try_from_slice_and_index(&entry.data, entry.font_index)?;
			for v in &font_opts.variations {
				f.set_variation(v.axis.as_bytes(), v.value.to_num());
			}
			f
		};
		let units_per_em = I26F6::from_bits(font.head()?.units_per_em() as i32);
		let hash = {
			let mut s = DefaultHasher::new();
			font_opts.hash(&mut s);
			s.finish()
		};

		self.stack.push(FontStackEntry {
			face_ref,
			font,
			location,
			shaper_data: &entry.shaper_data,
			shaper_instance: Some(shaper_instance),
			printer_font,
			units_per_em,
			fallback: false,
			hash: Some(hash),
		});

		Ok(face_ref)
	}

	pub fn add_fallbacks(&mut self) -> Result<(), SculpterFontStackError> {
		for entry in self.fonts.font_fallbacks() {
			let face_ref = ShapeFaceRef(self.stack.len() as u16);
			let font = harfrust::FontRef::from_index(&entry.data, entry.font_index)?;
			let printer_font =
				ab_glyph::FontRef::try_from_slice_and_index(&entry.data, entry.font_index)?;
			let units_per_em = I26F6::from_bits(font.head()?.units_per_em() as i32);

			self.stack.push(FontStackEntry {
				face_ref,
				font,
				location: Location::default(),
				shaper_data: &entry.shaper_data,
				shaper_instance: None,
				printer_font,
				units_per_em,
				fallback: true,
				hash: None,
			});
		}
		Ok(())
	}

	pub fn face_ref(&self, font_opts: &FontOptions<'_>) -> Option<ShapeFaceRef> {
		let hash = {
			let mut s = DefaultHasher::new();
			font_opts.hash(&mut s);
			s.finish()
		};
		self.stack
			.iter()
			.find_map(|e| e.hash.is_some_and(|h| h == hash).then_some(e.face_ref))
	}

	pub fn fallback(&self) -> ShapeFaceRef {
		ShapeFaceRef(0)
	}

	pub(crate) fn entries(&self) -> &[FontStackEntry<'font>] {
		&self.stack
	}
}
