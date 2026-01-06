#![allow(dead_code)]
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use ab_glyph::VariableFont;
use fixed::types::I26F6;

use crate::error::SculpterCreateError;
use crate::error::SculpterLoadError;
use crate::printer::SculpturePrinter;
use crate::shaper::SculptureShaper;
use crate::shaper::ShapeFaceRef;

pub mod error;
mod fonts;
pub mod layout;
pub mod printer;
pub mod shaper;

#[derive(Debug, Default)]
pub enum Family<'a> {
	Name(&'a str),
	#[default]
	Serif,
	SansSerif,
}

pub struct FontOptions<'a> {
	family: Family<'a>,
	variations: Vec<ttf_parser::Variation>,
}

impl<'a> FontOptions<'a> {
	pub fn new(family: Family<'a>) -> Self {
		Self {
			family,
			variations: Vec::new(),
		}
	}

	pub fn with_variations<IVar: Iterator<Item = ttf_parser::Variation>>(self, vars: IVar) -> Self {
		let Self { family, .. } = self;
		Self {
			family,
			variations: vars.collect(),
		}
	}
}

pub struct FontEntry {
	hash: u64,
	families: Vec<(String, ttf_parser::Language)>,
	data: Cow<'static, [u8]>,
	font_index: u32,
}

impl FontEntry {
	fn has(&self, family_name: &str) -> bool {
		self.families
			.iter()
			.any(|(family, _)| family == family_name)
	}
}

pub struct FontFallback {
	hash: u64,
	data: Cow<'static, [u8]>,
	font_index: u32,
}

pub struct Sculpter {
	fonts: BTreeMap<u64, FontEntry>,
	font_fallbacks: Vec<FontFallback>,
	family_serif: Cow<'static, str>,
	family_sans_serif: Cow<'static, str>,
}

impl Default for Sculpter {
	fn default() -> Self {
		Self::new()
	}
}

impl Sculpter {
	pub fn new() -> Self {
		Self {
			fonts: BTreeMap::new(),
			font_fallbacks: Vec::new(),
			family_serif: Cow::from("EB Garamond"),
			family_sans_serif: Cow::from("Open Sans"),
		}
	}

	pub fn load_builtin_fonts(&mut self) -> Result<(), SculpterLoadError> {
		let e = create_font_entry(fonts::EB_GARAMOND_VF_TTF)?;
		self.fonts.insert(e.hash, e);
		let e = create_font_entry(fonts::EB_GARAMOND_ITALIC_VF_TTF)?;
		self.fonts.insert(e.hash, e);
		let e = create_font_entry(fonts::OPEN_SANS_VF_TTF)?;
		self.fonts.insert(e.hash, e);
		let e = create_font_entry(fonts::OPEN_SANS_ITALIC_VF_TTF)?;
		self.fonts.insert(e.hash, e);

		let e = create_font_fallback(fonts::NOTO_EMOJI_VF_TTF);
		self.font_fallbacks.push(e);

		Ok(())
	}

	pub fn set_serif_family<S: Into<Cow<'static, str>>>(&mut self, name: S) {
		self.family_serif = name.into();
	}

	pub fn set_sans_serif_family<S: Into<Cow<'static, str>>>(&mut self, name: S) {
		self.family_sans_serif = name.into();
	}

	pub fn create_shaper<'a>(
		&'a self,
		scale_factor: f32,
		font_opts: &[FontOptions<'_>],
	) -> Result<(Vec<ShapeFaceRef>, SculptureShaper<'a>, SculpturePrinter<'a>), SculpterCreateError>
	{
		let scale_factor = I26F6::from_num(scale_factor);
		let mut shaper = SculptureShaper::new(scale_factor);
		let mut printer = SculpturePrinter::new(scale_factor, [8192; 2]);

		let mut face_refs = Vec::with_capacity(font_opts.len());
		for fo in font_opts {
			let family_name = match fo.family {
				Family::Name(s) => s,
				Family::Serif => &self.family_serif,
				Family::SansSerif => &self.family_sans_serif,
			};
			let font = self
				.fonts
				.values()
				.find(|e| e.has(family_name))
				.ok_or_else(|| SculpterCreateError::NoFontFound(family_name.to_string()))?;
			let mut shaper_face =
				rustybuzz::Face::from_face(ttf_parser::Face::parse(&font.data, font.font_index)?);
			let mut printer_font =
				ab_glyph::FontRef::try_from_slice_and_index(&font.data, font.font_index)?;

			for v in &fo.variations {
				if shaper_face.set_variation(v.axis, v.value).is_none() {
					log::warn!("Font {family_name} does not have variation axis {}", v.axis);
				}
				printer_font.set_variation(&v.axis.to_bytes(), v.value);
			}

			let shaper_ref = shaper.add(shaper_face, false);
			let printer_ref = printer.add(printer_font);
			debug_assert_eq!(shaper_ref, printer_ref, "Missmatched face ref");
			face_refs.push(shaper_ref);
		}

		for font in &self.font_fallbacks {
			let shaper_face =
				rustybuzz::Face::from_face(ttf_parser::Face::parse(&font.data, font.font_index)?);
			let printer_font =
				ab_glyph::FontRef::try_from_slice_and_index(&font.data, font.font_index)?;

			let shaper_ref = shaper.add(shaper_face, true);
			let printer_ref = printer.add(printer_font);
			debug_assert_eq!(shaper_ref, printer_ref, "Missmatched face ref");
		}

		Ok((face_refs, shaper, printer))
	}
}

fn create_font_entry<D: Into<Cow<'static, [u8]>>>(d: D) -> Result<FontEntry, SculpterLoadError> {
	let data = d.into();
	let mut s = DefaultHasher::new();
	data.hash(&mut s);
	let hash = s.finish();

	let face = ttf_parser::Face::parse(&data, 0)?;
	let families = collect_families(&face);

	Ok(FontEntry {
		hash,
		families,
		data,
		font_index: 0,
	})
}

fn create_font_fallback<D: Into<Cow<'static, [u8]>>>(d: D) -> FontFallback {
	let data = d.into();
	let mut s = DefaultHasher::new();
	data.hash(&mut s);
	let hash = s.finish();

	FontFallback {
		hash,
		data,
		font_index: 0,
	}
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
