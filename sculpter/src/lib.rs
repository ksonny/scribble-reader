#![allow(dead_code)]
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use crate::error::SculpterError;

mod error;
mod fonts;
mod shape;

#[derive(Debug, Default)]
pub enum Family<'a> {
	Name(&'a str),
	#[default]
	Serif,
	SansSerif,
}

pub struct FontEntry {
	hash: u64,
	families: Vec<(String, ttf_parser::Language)>,
	d: Cow<'static, [u8]>,
}

pub struct FontFallback {
	hash: u64,
	d: Cow<'static, [u8]>,
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

	pub fn load_builtin_fonts(&mut self) -> Result<(), SculpterError> {
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

	pub fn query(&self, family: Family) -> Option<&FontEntry> {
		let family_name = match family {
			Family::Name(s) => s,
			Family::Serif => &self.family_serif,
			Family::SansSerif => &self.family_sans_serif,
		};

		self.fonts.values().find(|entry| {
			entry
				.families
				.iter()
				.any(|(family, _)| family == family_name)
		})
	}
}

fn create_font_entry<D: Into<Cow<'static, [u8]>>>(d: D) -> Result<FontEntry, SculpterError> {
	let d = d.into();
	let mut s = DefaultHasher::new();
	d.hash(&mut s);
	let hash = s.finish();

	let face = ttf_parser::Face::parse(&d, 0)?;
	let families = collect_families(&face);

	Ok(FontEntry { hash, families, d })
}

fn create_font_fallback<D: Into<Cow<'static, [u8]>>>(d: D) -> FontFallback {
	let d = d.into();
	let mut s = DefaultHasher::new();
	d.hash(&mut s);
	let hash = s.finish();

	FontFallback { hash, d }
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
