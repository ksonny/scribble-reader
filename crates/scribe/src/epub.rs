use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Display;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use quick_xml::escape::unescape;
use quick_xml::events::BytesStart;
use quick_xml::name::QName;
use zip::ZipArchive;

pub const EPUB_CONTAINER_PATH: &str = "META-INF/container.xml";

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord)]
pub struct ResourceId(Arc<String>);

impl Display for ResourceId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.deref())
	}
}

impl Deref for ResourceId {
	type Target = str;

	fn deref(&self) -> &Self::Target {
		let Self(id) = self;
		id.as_str()
	}
}

impl Borrow<str> for ResourceId {
	fn borrow(&self) -> &str {
		let Self(id) = self;
		id.as_str()
	}
}

impl ResourceId {
	fn into_inner(self) -> Arc<String> {
		let ResourceId(inner) = self;
		inner
	}
}

/// Extracted epub metadata
#[derive(Debug, Default)]
pub struct Metadata {
	/// Unique identifier for book
	///
	/// Maps to `dc:identifier`.
	pub identifier: Option<String>,

	/// Book title
	///
	/// Maps to `dc:title`.
	pub title: Option<String>,

	/// Creator of book
	///
	/// Maps to `dc:creator`.
	pub creator: Option<String>,

	/// Publisher of book
	///
	/// Maps to `dc:publisher`.
	pub publisher: Option<String>,

	/// Language code
	///
	/// Maps to `dc:language`.
	pub language: Option<String>,

	/// Publication date
	///
	/// Maps to `dc:date`.
	pub date: Option<String>,

	/// The copyright or rights element
	///
	/// Maps to `dc:rights`.
	pub rights: Option<String>,

	/// Book cover
	///
	/// Maps to meta `cover` or item with `cover-image` property
	pub cover: Option<ResourceId>,

	/// Book navigation
	///
	/// Maps to item with `nav` property
	pub navigation: Option<ResourceId>,
}

#[derive(Debug)]
pub struct ResourceItem {
	pub id: ResourceId,
	pub href: String,
	pub mime: String,
	pub properties: Option<String>,
}

impl ResourceItem {
	pub fn as_path(&self) -> &Path {
		Path::new(&self.href)
	}
}

#[derive(Debug, thiserror::Error)]
pub enum EpubError {
	#[error(transparent)]
	QuickXml(#[from] quick_xml::Error),
	#[error("at {1}: {0}")]
	Zip(
		zip::result::ZipError,
		&'static std::panic::Location<'static>,
	),
	#[error("No epub package root file in zip")]
	NoEpubRootFile,
}

impl From<zip::result::ZipError> for EpubError {
	#[track_caller]
	fn from(err: zip::result::ZipError) -> Self {
		Self::Zip(err, std::panic::Location::caller())
	}
}

pub struct EpubMetadata<'a, R> {
	archive: &'a mut ZipArchive<R>,
	package: Option<Arc<Package>>,
}

impl<'a, R: io::Read + io::Seek> EpubMetadata<'a, R> {
	pub fn new(archive: &'a mut ZipArchive<R>) -> Self {
		Self {
			archive,
			package: None,
		}
	}

	pub fn into_inner(self) -> &'a mut ZipArchive<R> {
		self.archive
	}

	pub fn package(&mut self) -> Result<Arc<Package>, EpubError> {
		if let Some(package) = &self.package {
			Ok(package.clone())
		} else {
			let file = self.archive.by_path(Path::new(EPUB_CONTAINER_PATH))?;
			let package_path =
				parse_container(quick_xml::Reader::from_reader(BufReader::new(file)))?;
			let Some(root_path) = package_path else {
				return Err(EpubError::NoEpubRootFile);
			};

			let root_dir = root_path.as_path().parent().unwrap_or(Path::new(""));
			let file = self.archive.by_path(&root_path)?;
			let package = parse_package(
				root_dir,
				quick_xml::Reader::from_reader(BufReader::new(file)),
			)?;

			let package = Arc::new(package);
			self.package = Some(package.clone());
			Ok(package)
		}
	}

	pub fn navigation(&mut self) -> Result<Navigation, EpubError> {
		fn last_resort(package: &Package) -> Navigation {
			// Last resort, all spine items
			let nav_points = package
				.spine
				.iter()
				.enumerate()
				.map(|(index, id)| NavPoint {
					idref: id.clone(),
					title: id.clone().into_inner(),
					parent: None,
					spine: Some(index as u32),
				})
				.collect();
			Navigation {
				doc_title: None,
				nav_points,
			}
		}

		let package = self.package()?;

		let ncx_nav = package.manifest.get("ncx");
		if let Some(nav) = package
			.metadata
			.navigation
			.as_ref()
			.and_then(|id| package.manifest.get(id))
			.or_else(|| package.manifest.get("nav"))
			.or(ncx_nav)
		{
			let path = Path::new(&nav.href);
			if path.extension().is_some_and(|e| e == "xhtml") {
				let file = self.archive.by_path(path)?;
				let nav = parse_nav(
					&package,
					quick_xml::Reader::from_reader(BufReader::new(file)),
				)?;
				if let Some(nav) = nav {
					log::trace!("Navigation parsed, {} nav points", nav.nav_points.len());
					Ok(nav)
				} else if let Some(nav) = ncx_nav {
					let path = Path::new(&nav.href);
					let file = self.archive.by_path(path)?;
					let nav = parse_ncx(
						&package,
						quick_xml::Reader::from_reader(BufReader::new(file)),
					)?;
					log::trace!("Ncx fallback parsed, {} nav points", nav.nav_points.len());
					Ok(nav)
				} else {
					Ok(last_resort(&package))
				}
			} else if path.extension().is_some_and(|e| e == "ncx") {
				let file = self.archive.by_path(path)?;
				let nav = parse_ncx(
					&package,
					quick_xml::Reader::from_reader(BufReader::new(file)),
				)?;
				log::trace!("Ncx parsed, {} nav points", nav.nav_points.len());
				Ok(nav)
			} else {
				Ok(last_resort(&package))
			}
		} else {
			Ok(last_resort(&package))
		}
	}
}

#[derive(Debug, Default, Clone, Copy)]
enum ContainerElement {
	#[default]
	Unknown,

	Container,
	RootFiles,
	RootFile,
}

impl ContainerElement {
	fn from(name: QName<'_>) -> Self {
		let prefix = name.prefix();
		let local = name.local_name();

		if prefix.is_none() {
			match local.as_ref() {
				b"container" => Self::Container,
				b"rootfiles" => Self::RootFiles,
				b"rootfile" => Self::RootFile,
				_ => Self::Unknown,
			}
		} else {
			Self::Unknown
		}
	}
}

pub fn parse_container<R: BufRead>(
	mut reader: quick_xml::Reader<R>,
) -> Result<Option<PathBuf>, quick_xml::Error> {
	use quick_xml::events::Event;

	let mut buf = Vec::new();

	loop {
		match reader.read_event_into(&mut buf)? {
			Event::Start(e) | Event::Empty(e) => {
				let el = ContainerElement::from(e.name());
				if !matches!(el, ContainerElement::RootFile) {
					continue;
				}

				let path = e.attributes().find_map(|attr| {
					let attr = attr.inspect_err(|e| log::warn!("Attr error: {e}")).ok()?;
					(attr.key.as_ref() == b"full-path").then(|| {
						attr.decode_and_unescape_value(reader.decoder())
							.inspect_err(|e| log::warn!("Attr value decode error: {e}"))
							.unwrap_or_default()
					})
				});
				if let Some(path) = path {
					return Ok(Some(Path::new(path.as_ref()).to_path_buf()));
				}
			}
			Event::Eof => break,
			_ => {}
		}
	}
	Ok(None)
}

pub struct Package {
	pub package_root: PathBuf,
	pub metadata: Metadata,
	pub manifest: BTreeMap<ResourceId, ResourceItem>,
	pub spine: Vec<ResourceId>,
}

impl Package {
	pub fn metadata_by_spine(&self, idx: usize) -> Option<&ResourceItem> {
		self.manifest.get(self.spine.get(idx)?)
	}
}

#[derive(Debug, Default, Clone, Copy)]
enum PackageElement {
	#[default]
	Unknown,

	Package,
	Metadata,
	Meta,
	Manifest,
	Item,
	Spine,
	ItemRef,
	Guide,
	Reference,

	DcIdentifier,
	DcTitle,
	DcCreator,
	DcPublisher,
	DcLanguage,
	DcDate,
	DcRights,
}

impl PackageElement {
	fn from(name: QName<'_>) -> Self {
		let prefix = name.prefix();
		let local = name.local_name();

		if prefix.is_none() {
			match local.as_ref() {
				b"package" => Self::Package,
				b"metadata" => Self::Metadata,
				b"meta" => Self::Meta,
				b"manifest" => Self::Manifest,
				b"item" => Self::Item,
				b"spine" => Self::Spine,
				b"itemref" => Self::ItemRef,
				b"guide" => Self::Guide,
				b"reference" => Self::Reference,
				_ => Self::Unknown,
			}
		} else if prefix.is_some_and(|p| p.as_ref() == b"dc") {
			match local.as_ref() {
				b"identifier" => Self::DcIdentifier,
				b"title" => Self::DcTitle,
				b"creator" => Self::DcCreator,
				b"publisher" => Self::DcPublisher,
				b"language" => Self::DcLanguage,
				b"date" => Self::DcDate,
				b"rights" => Self::DcRights,
				_ => Self::Unknown,
			}
		} else {
			Self::Unknown
		}
	}
}

pub fn parse_package<R: BufRead>(
	package_root: &Path,
	mut reader: quick_xml::Reader<R>,
) -> Result<Package, quick_xml::Error> {
	use quick_xml::events::Event;

	let package_root = package_root.to_path_buf();

	let mut resource_ids = BTreeSet::new();

	let mut metadata = Metadata::default();
	let mut resources = BTreeMap::new();
	let mut spine = Vec::new();

	let mut buf = Vec::new();
	let mut txt_buf = Vec::new();
	let mut path = Vec::new();

	loop {
		match reader.read_event_into(&mut buf)? {
			Event::Start(e) => {
				let el = PackageElement::from(e.name());

				let field = match el {
					PackageElement::DcIdentifier => Some(&mut metadata.identifier),
					PackageElement::DcTitle => Some(&mut metadata.title),
					PackageElement::DcCreator => Some(&mut metadata.creator),
					PackageElement::DcPublisher => Some(&mut metadata.publisher),
					PackageElement::DcLanguage => Some(&mut metadata.language),
					PackageElement::DcDate => Some(&mut metadata.date),
					PackageElement::DcRights => Some(&mut metadata.rights),
					_ => {
						path.push(el);
						None
					}
				};

				if let Some(field) = field {
					let value = reader.read_text_into(e.name(), &mut txt_buf)?.decode()?;
					let value = unescape(&value)?.to_string();

					if field.replace(value).is_some() {
						log::warn!("Multiple '{:?}' metadata entries", el);
					}
				}
			}
			Event::End(_) => {
				path.pop();
			}
			Event::Empty(e) if path.iter().any(|el| matches!(el, PackageElement::Metadata)) => {
				let el = PackageElement::from(e.name());

				if matches!(el, PackageElement::Meta) {
					let is_cover = e.attributes().any(|attr| {
						attr.inspect_err(|e| log::warn!("Meta attr error: {e}"))
							.is_ok_and(|attr| {
								attr.key.as_ref() == b"name" && attr.value.as_ref() == b"cover"
							})
					});
					if !is_cover {
						continue;
					}

					let content = e
						.attributes()
						.find_map(|attr| {
							let attr = attr.inspect_err(|e| log::warn!("Attr error: {e}")).ok()?;
							(attr.key.as_ref() == b"content").then(|| {
								attr.decode_and_unescape_value(reader.decoder())
									.inspect_err(|e| log::warn!("Attr value decode error: {e}"))
									.unwrap_or_default()
							})
						})
						.unwrap_or_default();
					let resource_id =
						resource_ids
							.get(content.as_ref())
							.cloned()
							.unwrap_or_else(|| {
								let id = ResourceId(Arc::new(content.to_string()));
								resource_ids.insert(id.clone());
								id
							});

					if metadata.cover.replace(resource_id).is_some() {
						log::warn!("Multiple '{:?}' metadata entries", el);
					}
				}
			}
			Event::Empty(e) if path.iter().any(|el| matches!(el, PackageElement::Manifest)) => {
				let el = PackageElement::from(e.name());
				if !matches!(el, PackageElement::Item) {
					continue;
				}

				let mut id = None;
				let mut mime = None;
				let mut href = None;
				let mut properties = None;

				for attr in e.attributes() {
					let Ok(attr) = attr.inspect_err(|e| log::warn!("Attr error: {e}")) else {
						continue;
					};
					let Ok(value) = attr
						.decode_and_unescape_value(reader.decoder())
						.inspect_err(|e| log::warn!("Attr value decode error: {e}"))
					else {
						continue;
					};

					match attr.key.as_ref() {
						b"id" => {
							let resource_id = resource_ids
								.get(value.as_ref())
								.cloned()
								.unwrap_or_else(|| {
									let id = ResourceId(Arc::new(value.to_string()));
									resource_ids.insert(id.clone());
									id
								});
							id = Some(resource_id)
						}
						b"href" => {
							let path = Path::new(value.as_ref());
							let value = if path.is_relative() {
								package_root.join(path).to_string_lossy().to_string()
							} else {
								value.to_string()
							};
							href = Some(value)
						}
						b"media-type" => mime = Some(value.to_string()),
						b"properties" => properties = Some(value.to_string()),
						_ => {}
					}
				}

				match (id, mime, href) {
					(Some(id), Some(mime), Some(href)) => {
						if properties
							.as_ref()
							.is_some_and(|p| p.split(" ").any(|p| p == "cover-image"))
							&& metadata.cover.is_none()
						{
							log::debug!("Upsert resource as cover: {id}");
							metadata.cover = Some(id.clone());
						}
						if properties
							.as_ref()
							.is_some_and(|p| p.split(" ").any(|p| p == "nav"))
							&& metadata.navigation.is_none()
						{
							log::debug!("Upsert resource as nav: {id}");
							metadata.navigation = Some(id.clone());
						}

						resources.insert(
							id.clone(),
							ResourceItem {
								id,
								href,
								mime,
								properties,
							},
						);
					}
					values => {
						log::error!("Skip resource because missing values {:?}", values);
					}
				}
			}
			Event::Empty(e) if path.iter().any(|el| matches!(el, PackageElement::Spine)) => {
				let el = PackageElement::from(e.name());
				if !matches!(el, PackageElement::ItemRef) {
					continue;
				}
				let mut idref = None;

				for attr in e.attributes() {
					let Ok(attr) = attr.inspect_err(|e| log::warn!("Attr error: {e}")) else {
						continue;
					};
					let Ok(value) = attr
						.decode_and_unescape_value(reader.decoder())
						.inspect_err(|e| log::warn!("Attr value decode error: {e}"))
					else {
						continue;
					};

					if attr.key.as_ref() == b"idref" {
						idref = Some(value)
					}
				}

				if let Some(idref) = idref {
					let resource_id =
						resource_ids
							.get(idref.as_ref())
							.cloned()
							.unwrap_or_else(|| {
								let id = ResourceId(Arc::new(idref.to_string()));
								resource_ids.insert(id.clone());
								id
							});
					spine.push(resource_id);
				}
			}
			Event::Eof => {
				break;
			}
			_ => {}
		}
	}
	debug_assert!(path.is_empty(), "Path should be empty");

	Ok(Package {
		package_root,
		metadata,
		manifest: resources,
		spine,
	})
}

#[derive(Debug)]
pub struct NavPoint {
	pub idref: ResourceId,
	pub title: Arc<String>,
	pub parent: Option<ResourceId>,
	pub spine: Option<u32>,
}

#[derive(Debug, Default)]
pub struct Navigation {
	pub doc_title: Option<String>,
	pub nav_points: Vec<NavPoint>,
}

struct NavEntry {
	idref: Option<ResourceId>,
	title: Option<String>,
	parent_index: Option<usize>,
}

#[derive(Debug, Default, Clone, Copy)]
enum NavElement {
	#[default]
	Unknown,

	NavToc,
	H1,
	Ol,
	Li,
	A,
}

impl NavElement {
	fn from(e: &BytesStart<'_>) -> Self {
		let name = e.name();
		let prefix = name.prefix();
		let local = name.local_name();

		if prefix.is_none() {
			match local.as_ref() {
				b"nav" => {
					let is_toc = e.attributes().any(|attr| {
						attr.inspect_err(|e| log::warn!("Attr error: {e}"))
							.is_ok_and(|attr| {
								attr.key.prefix().is_some_and(|p| p.as_ref() == b"epub")
									&& attr.key.local_name().as_ref() == b"type"
									&& attr.value.as_ref() == b"toc"
							})
					});
					if is_toc { Self::NavToc } else { Self::Unknown }
				}
				b"h1" => Self::H1,
				b"ol" => Self::Ol,
				b"li" => Self::Li,
				b"a" => Self::A,
				_ => Self::Unknown,
			}
		} else {
			Self::Unknown
		}
	}
}

pub fn parse_nav<R: BufRead>(
	package: &Package,
	mut reader: quick_xml::Reader<R>,
) -> Result<Option<Navigation>, quick_xml::Error> {
	use quick_xml::events::Event;

	let package_root = package.package_root.as_path();
	let path_lookup = package
		.manifest
		.values()
		.map(|r| (r.href.as_str(), r.id.clone()))
		.collect::<BTreeMap<_, _>>();

	let mut doc_title = None;
	let mut entries = Vec::new();
	let mut stack = Vec::new();

	let mut path = Vec::new();
	let mut txt_buf = Vec::new();
	let mut buf = Vec::new();

	let mut has_nav_toc_element = false;

	loop {
		match reader.read_event_into(&mut buf)? {
			Event::Start(e) => {
				let el = NavElement::from(&e);
				path.push(el);

				if !path.iter().any(|el| matches!(el, NavElement::NavToc)) {
					continue;
				}
				has_nav_toc_element = true;

				if matches!(el, NavElement::Li) {
					let idx = entries.len();
					entries.push(NavEntry {
						idref: None,
						title: None,
						parent_index: stack.last().cloned(),
					});
					stack.push(idx);
				} else if matches!(el, NavElement::A) {
					let key = b"href";
					let href = e.attributes().find_map(|attr| {
						let attr = attr.inspect_err(|e| log::warn!("Attr error: {e}")).ok()?;
						(attr.key.as_ref() == key).then(|| {
							attr.decode_and_unescape_value(reader.decoder())
								.inspect_err(|e| log::warn!("Attr value decode error: {e}"))
								.unwrap_or_default()
						})
					});
					if let Some(href) = href
						&& let Some(idx) = stack.last().cloned()
					{
						path.pop(); // Read text handles end

						let value = reader.read_text_into(e.name(), &mut txt_buf)?.decode()?;
						let value = unescape(&value)?.to_string();
						entries[idx].title = Some(value);

						let path = Path::new(href.as_ref());
						if path.is_relative() {
							let path = package_root.join(path);
							let idref = path_lookup.get(path.to_string_lossy().as_ref()).cloned();
							if idref.is_none() {
								log::warn!("Navigation target not found in manifest: {}", href);
							}
							entries[idx].idref = idref;
						} else {
							let idref = path_lookup.get(href.as_ref()).cloned();
							if idref.is_none() {
								log::warn!("Navigation target not found in manifest: {}", href);
							}
							entries[idx].idref = idref;
						};
					}
				} else if matches!(el, NavElement::H1) {
					path.pop(); // Read text handles end

					let value = reader.read_text_into(e.name(), &mut txt_buf)?.decode()?;
					let value = unescape(&value)?.to_string();
					doc_title = Some(value);
				}
			}
			Event::End(_) => {
				let el = path.pop();
				if let Some(el) = el
					&& path.iter().any(|el| matches!(el, NavElement::NavToc))
					&& matches!(el, NavElement::Li)
				{
					stack.pop();
				}
			}
			Event::Eof => break,
			_ => {}
		}
	}
	debug_assert!(stack.is_empty(), "Stack should be empty");
	debug_assert!(path.is_empty(), "Path should be empty");

	if !has_nav_toc_element {
		return Ok(None);
	}

	let spine_lookup = package
		.spine
		.iter()
		.enumerate()
		.map(|(index, id)| (id, index as u32))
		.collect::<BTreeMap<_, _>>();
	let idrefs = entries.iter().map(|e| e.idref.clone()).collect::<Vec<_>>();

	let mut nav_points = Vec::new();
	for entry in entries {
		let Some(idref) = entry.idref else {
			continue;
		};
		let Some(title) = entry.title else {
			continue;
		};
		let title = Arc::new(title);
		let parent = entry.parent_index.and_then(|idx| idrefs[idx].clone());
		let spine = spine_lookup.get(&idref).cloned();
		nav_points.push(NavPoint {
			idref,
			title,
			parent,
			spine,
		});
	}

	Ok(Some(Navigation {
		doc_title,
		nav_points,
	}))
}

#[derive(Debug, Default, Clone, Copy)]
enum NcxElement {
	#[default]
	Unknown,

	Ncx,
	Head,
	Meta,
	DocTitle,
	Text,
	NavMap,
	NavPoint,
	NavLabel,
	Content,
}

impl NcxElement {
	fn from(name: QName<'_>) -> Self {
		let prefix = name.prefix();
		let local = name.local_name();

		if prefix.is_none() {
			match local.as_ref() {
				b"ncx" => Self::Ncx,
				b"head" => Self::Head,
				b"meta" => Self::Meta,
				b"docTitle" => Self::DocTitle,
				b"text" => Self::Text,
				b"navMap" => Self::NavMap,
				b"navPoint" => Self::NavPoint,
				b"navLabel" => Self::NavLabel,
				b"content" => Self::Content,
				_ => Self::Unknown,
			}
		} else {
			Self::Unknown
		}
	}
}

pub fn parse_ncx<R: BufRead>(
	package: &Package,
	mut reader: quick_xml::Reader<R>,
) -> Result<Navigation, quick_xml::Error> {
	use quick_xml::events::Event;

	let package_root = package.package_root.as_path();
	let path_lookup = package
		.manifest
		.values()
		.map(|r| (r.href.as_str(), r.id.clone()))
		.collect::<BTreeMap<_, _>>();

	let mut doc_title = None;
	let mut entries = Vec::new();
	let mut stack = Vec::new();

	let mut buf = Vec::new();
	let mut txt_buf = Vec::new();
	let mut path = Vec::new();

	loop {
		match reader.read_event_into(&mut buf)? {
			Event::Start(e) => {
				let el = NcxElement::from(e.name());

				if matches!(el, NcxElement::NavPoint) {
					let idx = entries.len();
					entries.push(NavEntry {
						idref: None,
						title: None,
						parent_index: stack.last().cloned(),
					});
					stack.push(idx);
				}

				if matches!(el, NcxElement::Text) {
					let value = reader.read_text_into(e.name(), &mut txt_buf)?.decode()?;
					let value = unescape(&value)?;

					if matches!(path.last(), Some(NcxElement::DocTitle)) {
						doc_title = Some(value.to_string());
					} else if matches!(path.last(), Some(NcxElement::NavLabel))
						&& let Some(idx) = stack.last().cloned()
					{
						entries[idx].title = Some(value.to_string());
					}
				} else {
					path.push(el);
				}
			}
			Event::End(_) => {
				let el = path.pop();
				if let Some(el) = el
					&& matches!(el, NcxElement::NavPoint)
				{
					stack.pop();
				}
			}
			Event::Empty(e) => {
				let el = NcxElement::from(e.name());
				if matches!(el, NcxElement::Content) {
					let src = e.attributes().find_map(|attr| {
						let attr = attr.inspect_err(|e| log::warn!("Attr error: {e}")).ok()?;
						(attr.key.as_ref() == b"src").then(|| {
							attr.decode_and_unescape_value(reader.decoder())
								.inspect_err(|e| log::warn!("Attr value decode error: {e}"))
								.unwrap_or_default()
						})
					});
					if let Some(src) = src
						&& let Some(idx) = stack.last().cloned()
					{
						let path = Path::new(src.as_ref());
						if path.is_relative() {
							let path = package_root.join(path);
							let idref = path_lookup.get(path.to_string_lossy().as_ref()).cloned();
							if idref.is_none() {
								log::warn!("Navigation target not found in manifest: {}", src);
							}
							entries[idx].idref = idref;
						} else {
							let idref = path_lookup.get(src.as_ref()).cloned();
							if idref.is_none() {
								log::warn!("Navigation target not found in manifest: {}", src);
							}
							entries[idx].idref = idref;
						};
					}
				}
			}
			Event::Eof => break,
			_ => {}
		}
	}
	debug_assert!(stack.is_empty(), "Stack should be empty");
	debug_assert!(path.is_empty(), "Path should be empty");

	let spine_lookup = package
		.spine
		.iter()
		.enumerate()
		.map(|(index, id)| (id, index as u32))
		.collect::<BTreeMap<_, _>>();
	let idrefs = entries.iter().map(|e| e.idref.clone()).collect::<Vec<_>>();

	let mut nav_points = Vec::new();
	for entry in entries {
		let Some(idref) = entry.idref else {
			continue;
		};
		let Some(title) = entry.title else {
			continue;
		};
		let title = Arc::new(title);
		let parent = entry.parent_index.and_then(|idx| idrefs[idx].clone());
		let spine = spine_lookup.get(&idref).cloned();
		nav_points.push(NavPoint {
			idref,
			title,
			parent,
			spine,
		});
	}

	Ok(Navigation {
		doc_title,
		nav_points,
	})
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	use crate::epub::parse_container;
	use crate::epub::parse_nav;
	use crate::epub::parse_ncx;
	use crate::epub::parse_package;

	#[test]
	fn test_container_parse() -> Result<(), quick_xml::de::DeError> {
		let input = r##"
<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml" />
  </rootfiles>
</container>
            "##;

		let reader = quick_xml::Reader::from_str(input);
		let path = parse_container(reader)?.expect("Failed to parse root path");

		assert_eq!(path.as_path(), Path::new("OEBPS/content.opf"));

		Ok(())
	}

	#[test]
	fn test_opf_v3_parse() -> Result<(), quick_xml::de::DeError> {
		let input = r##"
<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="epub-id-1">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"
            xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="epub-id-1">urn:uuid:b3003f46-8d2c-4646-8402-6e65185543dd</dc:identifier>
    <dc:title>Amelia Thornheart</dc:title>
    <dc:language>en</dc:language>
    <dc:creator id="epub-creator-0">Keene</dc:creator>
    <meta refines="#epub-creator-0" property="role" scheme="marc:relators">aut</meta>
    <meta property="dcterms:modified">2026-03-29T07:44:27Z</meta>
    <meta name="cover" content="cover-image"/>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item media-type="application/xhtml+xml" id="cover.xhtml" href="cover.xhtml"/>
    <item media-type="text/css" id="stylesheet.css" href="stylesheet.css"/>
    <item media-type="image/jpeg" properties="cover-image" id="cover-image" href="amelia-thornheart-aaaatp90qhu.jpg"/>
  </manifest>
  <spine>
  	<itemref idref="nav.xhtml"/>
  </spine>
  <guide>
    <reference type="toc" title="Table Of Contents" href="nav.xhtml"/>
    <reference type="cover" title="Cover" href="cover.xhtml"/>
  </guide>
</package>
"##;

		let reader = quick_xml::Reader::from_str(input);
		let result = parse_package(Path::new("OEBPS"), reader).expect("Parse failed");

		assert_eq!(
			result.metadata.identifier.as_deref(),
			Some("urn:uuid:b3003f46-8d2c-4646-8402-6e65185543dd"),
			"Identifier missmatch"
		);
		assert_eq!(
			result.metadata.title.as_deref(),
			Some("Amelia Thornheart"),
			"Title missmatch"
		);
		assert_eq!(
			result.metadata.language.as_deref(),
			Some("en"),
			"Language missmatch"
		);
		assert_eq!(
			result.metadata.creator.as_deref(),
			Some("Keene"),
			"Creator missmatch"
		);
		assert_eq!(
			result.metadata.cover.as_deref(),
			Some("cover-image"),
			"Cover missmatch"
		);
		assert!(result.metadata.publisher.is_none(), "Publisher missmatch");
		assert!(result.metadata.date.is_none(), "Date missmatch");

		assert_eq!(
			result.manifest.len(),
			5,
			"Unexpected resource count, resources {:?}",
			result.manifest.keys().collect::<Vec<_>>()
		);
		assert_eq!(result.spine.len(), 1, "Unexpected spine count");

		let cover_id = result.metadata.cover.as_ref().unwrap();
		let cover = result
			.manifest
			.get(cover_id)
			.expect("Cover not in manifest");

		assert_eq!(&cover.id, cover_id, "Unexpected cover.id");
		assert_eq!(&cover.mime, "image/jpeg", "Unexpected cover.mime");
		assert_eq!(
			&cover.href, "OEBPS/amelia-thornheart-aaaatp90qhu.jpg",
			"Unexpected cover.href"
		);
		assert_eq!(
			cover.properties.as_deref(),
			Some("cover-image"),
			"Unexpected cover.properties"
		);

		Ok(())
	}

	#[test]
	fn test_opf_v2_parse() -> Result<(), quick_xml::de::DeError> {
		let input = r##"
<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="epub-id-1">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"
            xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="epub-id-1">urn:uuid:2ce9fc1f-1168-499e-8389-0d2e1ccbf77e</dc:identifier>
    <dc:title>Amelia Thornheart</dc:title>
    <dc:language>en</dc:language>
    <dc:creator opf:role="aut">Keene</dc:creator>
    <meta name="cover" content="cover-image"/>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/>
    <item media-type="application/xhtml+xml" id="cover.xhtml" href="cover.xhtml"/>
    <item media-type="text/css" id="stylesheet.css" href="stylesheet.css"/>
    <item media-type="image/jpeg" id="cover-image" href="amelia-thornheart-aaaatp90qhu.jpg"/>
  </manifest>
  <spine>
  	<itemref idref="nav.xhtml" />
  </spine>
  <guide>
    <reference type="toc" title="Table Of Contents" href="nav.xhtml"/>
    <reference type="cover" title="Cover" href="cover.xhtml"/>
  </guide>
</package>
"##;

		let reader = quick_xml::Reader::from_str(input);
		let result = parse_package(Path::new("OEBPS"), reader).expect("Parse failed");

		assert_eq!(
			result.metadata.identifier.as_deref(),
			Some("urn:uuid:2ce9fc1f-1168-499e-8389-0d2e1ccbf77e"),
			"Identifier missmatch"
		);
		assert_eq!(
			result.metadata.title.as_deref(),
			Some("Amelia Thornheart"),
			"Title missmatch"
		);
		assert_eq!(
			result.metadata.language.as_deref(),
			Some("en"),
			"Language missmatch"
		);
		assert_eq!(
			result.metadata.creator.as_deref(),
			Some("Keene"),
			"Creator missmatch"
		);
		assert_eq!(
			result.metadata.cover.as_deref(),
			Some("cover-image"),
			"Cover missmatch"
		);
		assert!(result.metadata.publisher.is_none(), "Publisher missmatch");
		assert!(result.metadata.date.is_none(), "Date missmatch");

		assert_eq!(
			result.manifest.len(),
			5,
			"Unexpected resource count, resources {:?}",
			result.manifest.keys().collect::<Vec<_>>()
		);
		assert_eq!(result.spine.len(), 1, "Unexpected spine count");

		let cover_id = result.metadata.cover.as_ref().unwrap();
		let cover = result
			.manifest
			.get(cover_id)
			.expect("Cover not in manifest");

		assert_eq!(&cover.id, cover_id, "Unexpected cover.id");
		assert_eq!(&cover.mime, "image/jpeg", "Unexpected cover.mime");
		assert_eq!(
			&cover.href, "OEBPS/amelia-thornheart-aaaatp90qhu.jpg",
			"Unexpected cover.href"
		);

		Ok(())
	}

	#[test]
	fn test_ncx_parse() -> Result<(), quick_xml::de::DeError> {
		let input = r##"
<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="epub-id-1">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"
            xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="epub-id-1">urn:uuid:2ce9fc1f-1168-499e-8389-0d2e1ccbf77e</dc:identifier>
    <dc:title>Amelia Thornheart</dc:title>
    <dc:language>en</dc:language>
    <dc:creator opf:role="aut">Keene</dc:creator>
    <meta name="cover" content="cover-image"/>
  </metadata>
  <manifest>
    <item media-type="application/xhtml+xml" id="a" href="a.xhtml"/>
    <item media-type="application/xhtml+xml" id="b" href="b.xhtml"/>
    <item media-type="application/xhtml+xml" id="c" href="c.xhtml"/>
  </manifest>
  <spine>
  	<itemref idref="a" />
  	<itemref idref="b" />
  	<itemref idref="c" />
  </spine>
</package>
"##;

		let reader = quick_xml::Reader::from_str(input);
		let package = parse_package(Path::new("OEBPS"), reader).expect("Parse failed");

		let input = r#"
<?xml version="1.0" encoding="UTF-8"?>
<ncx version="2005-1" xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <head>
    <meta name="dtb:depth" content="1" />
    <meta name="dtb:totalPageCount" content="0" />
    <meta name="dtb:maxPageNumber" content="0" />
  </head>
  <docTitle>
    <text>Table Of Contents</text>
  </docTitle>
  <navMap>
    <navPoint id="navPoint-1">
      <navLabel>
       <text>Cover</text>
      </navLabel>
      <content src="a.xhtml"/>
      <navPoint id="navPoint-2">
        <navLabel>
         <text>Cover Sub</text>
        </navLabel>
        <content src="b.xhtml"/>
      </navPoint>
    </navPoint>
    <navPoint id="navPoint-3">
      <navLabel>
       <text>Cover</text>
      </navLabel>
      <content src="c.xhtml"/>
    </navPoint>
  </navMap>
</ncx>"#;

		let reader = quick_xml::Reader::from_str(input);
		let result = parse_ncx(&package, reader).expect("Parse failed");

		assert_eq!(result.doc_title.as_deref(), Some("Table Of Contents"));
		assert_eq!(result.nav_points.len(), 3);

		println!("Nav points: {:?}", result.nav_points);

		assert!(
			result.nav_points.iter().any(|n| &*n.idref == "a"),
			"Missing id a"
		);
		assert!(
			result.nav_points.iter().any(|n| &*n.idref == "b"),
			"Missing id b"
		);
		assert!(
			result.nav_points.iter().any(|n| &*n.idref == "c"),
			"Missing id c"
		);
		assert!(
			result
				.nav_points
				.iter()
				.any(|n| &*n.idref == "b" && n.parent.is_some()),
			"Expected parent on b"
		);
		assert!(
			result.nav_points.iter().all(|n| n.spine.is_some()),
			"Expected spine on all nav points"
		);

		Ok(())
	}

	#[test]
	fn test_nav_parse() -> Result<(), quick_xml::de::DeError> {
		let input = r##"
<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="epub-id-1">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"
            xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="epub-id-1">urn:uuid:2ce9fc1f-1168-499e-8389-0d2e1ccbf77e</dc:identifier>
    <dc:title>Amelia Thornheart</dc:title>
    <dc:language>en</dc:language>
    <dc:creator opf:role="aut">Keene</dc:creator>
    <meta name="cover" content="cover-image"/>
  </metadata>
  <manifest>
    <item media-type="application/xhtml+xml" id="a" href="a.xhtml"/>
    <item media-type="application/xhtml+xml" id="b" href="b.xhtml"/>
    <item media-type="application/xhtml+xml" id="c" href="c.xhtml"/>
  </manifest>
  <spine>
  	<itemref idref="a" />
  	<itemref idref="b" />
  	<itemref idref="c" />
  </spine>
</package>
"##;

		let reader = quick_xml::Reader::from_str(input);
		let package = parse_package(Path::new("OEBPS"), reader).expect("Parse failed");

		let input = r#"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head>
  <meta charset = "utf-8" />
  <meta name="generator" content="Rust EPUB library" />
  <title>Table Of Contents</title>
  <link rel="stylesheet" type="text/css" href="stylesheet.css" />
</head>
<body>
  <nav epub:type = "toc" id="toc">
    <h1 id="toc-title">Table Of Contents</h1>
    <ol>
      <li><a href="a.xhtml">Cover</a></li>
      <li>
        <a href="b.xhtml">Cover</a>
        <ol>
          <li><a href="c.xhtml">Cover</a></li>
        </ol>
      </li>
    </ol>
  </nav>
  <nav epub:type = "landmarks">
    <ol>
      <li><a epub:type="cover" href="cover.xhtml">Cover</a></li>
    </ol>
  </nav>
</body>
</html>
"#;

		let reader = quick_xml::Reader::from_str(input);
		let result = parse_nav(&package, reader)
			.expect("Parse failed")
			.expect("Expected xhtml navigation");

		assert_eq!(result.doc_title.as_deref(), Some("Table Of Contents"));
		assert_eq!(result.nav_points.len(), 3);

		println!("Nav points: {:?}", result.nav_points);

		assert!(
			result.nav_points.iter().any(|n| &*n.idref == "a"),
			"Missing id a"
		);
		assert!(
			result.nav_points.iter().any(|n| &*n.idref == "b"),
			"Missing id b"
		);
		assert!(
			result.nav_points.iter().any(|n| &*n.idref == "c"),
			"Missing id c"
		);
		assert!(
			result
				.nav_points
				.iter()
				.any(|n| &*n.idref == "c" && n.parent.is_some()),
			"Expected parent on c"
		);
		assert!(
			result.nav_points.iter().all(|n| n.spine.is_some()),
			"Expected spine on all nav points"
		);

		Ok(())
	}
}
