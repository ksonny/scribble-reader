use std::borrow::Cow;
use std::collections::BTreeMap;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use fixed::types::I26F6;
use language_tags::LanguageTag;
use read_fonts::TableProvider;
use skrifa::MetadataProvider;

use crate::Axis;
use crate::Family;
use crate::FontOptions;

pub(crate) struct FontEntry {
	pub(crate) hash: u64,
	pub(crate) units_per_em: I26F6,
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
	pub(crate) units_per_em: I26F6,
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
	let units_per_em = I26F6::from_bits(face.head()?.units_per_em() as i32);
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
		units_per_em,
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
	let units_per_em = I26F6::from_bits(face.head()?.units_per_em() as i32);
	let shaper_data = harfrust::ShaperData::new(&face);

	Ok(FontFallback {
		units_per_em,
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
