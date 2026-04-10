use std::borrow::Cow;
use std::collections::BTreeMap;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use fixed::types::I26F6;

use crate::Axis;
use crate::Family;
use crate::FontOptions;

#[derive(Debug)]
pub(crate) struct FontEntry {
	pub(crate) hash: u64,
	pub(crate) units_per_em: I26F6,
	pub(crate) families: Vec<(String, ttf_parser::Language)>,
	pub(crate) italic: bool,
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

#[derive(Debug)]
pub(crate) struct FontFallback {
	pub(crate) units_per_em: I26F6,
	pub(crate) data: Cow<'static, [u8]>,
	pub(crate) font_index: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SculpterFontErrors {
	#[error(transparent)]
	FaceParsing(#[from] ttf_parser::FaceParsingError),
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
	) -> Result<Self, SculpterFontErrors> {
		let Self {
			mut fonts,
			font_fallbacks,
			family_serif,
			family_sans_serif,
		} = self;

		let e = create_font_entry(data)?;
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
	) -> Result<Self, SculpterFontErrors> {
		let Self {
			fonts,
			mut font_fallbacks,
			family_serif,
			family_sans_serif,
		} = self;

		let e = create_font_fallback(data)?;
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

#[derive(Debug)]
struct SculpterFontsInner {
	fonts: BTreeMap<u64, FontEntry>,
	font_fallbacks: Vec<FontFallback>,
	family_serif: Cow<'static, str>,
	family_sans_serif: Cow<'static, str>,
}

#[derive(Debug, Clone)]
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

fn create_font_entry<D: Into<Cow<'static, [u8]>>>(d: D) -> Result<FontEntry, SculpterFontErrors> {
	let data = d.into();
	let mut s = DefaultHasher::new();
	data.hash(&mut s);
	let hash = s.finish();

	let face = ttf_parser::Face::parse(&data, 0)?;
	let units_per_em = I26F6::from_bits(face.units_per_em() as i32);
	let families = collect_families(&face);
	let italic = face.is_italic();

	Ok(FontEntry {
		hash,
		units_per_em,
		families,
		italic,
		data,
		font_index: 0,
	})
}

fn create_font_fallback<D: Into<Cow<'static, [u8]>>>(
	d: D,
) -> Result<FontFallback, SculpterFontErrors> {
	let data = d.into();
	let face = ttf_parser::Face::parse(&data, 0)?;
	let units_per_em = I26F6::from_bits(face.units_per_em() as i32);

	Ok(FontFallback {
		units_per_em,
		data,
		font_index: 0,
	})
}

fn collect_families(face: &ttf_parser::Face<'_>) -> Vec<(String, ttf_parser::Language)> {
	use ttf_parser::name_id::FAMILY;
	use ttf_parser::name_id::TYPOGRAPHIC_FAMILY;

	let mut families = Vec::new();

	families.extend(
		face.names()
			.into_iter()
			.filter(|name| name.name_id == TYPOGRAPHIC_FAMILY && name.is_unicode())
			.filter_map(|name| Some((name.to_string()?, name.language()))),
	);

	if families.is_empty() {
		families.extend(
			face.names()
				.into_iter()
				.filter(|name| name.name_id == FAMILY && name.is_unicode())
				.filter_map(|name| Some((name.to_string()?, name.language()))),
		);
	}

	if let Some(index) = families
		.iter()
		.position(|f| f.1 == ttf_parser::Language::English_UnitedStates)
		&& index != 0
	{
		families.swap(0, index);
	}

	families
}
