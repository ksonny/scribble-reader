mod error;
mod html_parser;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Write;
use std::fs;
use std::io;
use std::io::Cursor;
use std::io::Read;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LockResult;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::RwLock;
use std::sync::RwLockReadGuard;
use std::sync::mpsc::Sender;
use std::sync::mpsc::TryRecvError;
use std::sync::mpsc::channel;
use std::thread::JoinHandle;
use std::time::Instant;

use bitflags::bitflags;
use cosmic_text::Attrs;
use cosmic_text::Buffer;
use cosmic_text::FontSystem;
use cosmic_text::Shaping;
use epub::doc::EpubDoc;
use html5ever::LocalName;
use html5ever::local_name;
use resvg::tiny_skia;
use resvg::usvg;
use scribe::ScribeConfig;
use scribe::library;
use scribe::library::Location;
use sculpter::FontOptions;
use sculpter::Sculpter;
use sculpter::error::SculpterLoadError;
use serde::Deserialize;
use taffy::prelude::*;
use zip::ZipArchive;

use crate::error::IllustratorError;
use crate::error::IllustratorRenderError;
use crate::error::IllustratorRequestError;
use crate::error::IllustratorSpawnError;
use crate::error::IllustratorSvgError;
use crate::html_parser::EdgeRef;
use crate::html_parser::NodeTreeBuilder;
use crate::html_parser::Text;
use crate::html_parser::TextWrapper;

#[derive(Debug, Default)]
struct PageCacheEntry {
	spine: u32,
	elements: Range<u32>,
	pages: Vec<PageContent>,
}

const CACHE_CHAPTERS: usize = 5;

#[derive(Debug)]
pub struct PageContentCache {
	index: usize,
	entries: [Option<PageCacheEntry>; CACHE_CHAPTERS],
}

impl Default for PageContentCache {
	fn default() -> Self {
		Self {
			index: 0,
			entries: [const { None }; CACHE_CHAPTERS],
		}
	}
}

impl PageContentCache {
	fn entry(&self, loc: Location) -> Option<(&PageCacheEntry, &PageContent)> {
		let entry = self
			.entries
			.iter()
			.flatten()
			.find(|e| e.spine == loc.spine)?;

		if loc.element == entry.elements.start {
			Some((entry, entry.pages.first()?))
		} else if loc.element >= entry.elements.end {
			Some((entry, entry.pages.last()?))
		} else {
			let page = entry
				.pages
				.iter()
				.find(|p| p.elements.contains(&loc.element))?;
			Some((entry, page))
		}
	}

	pub fn page(&self, loc: Location) -> Option<&PageContent> {
		self.entry(loc).map(|(_, page)| page)
	}

	fn next_page(&self, book_meta: &BookMeta, loc: Location) -> Location {
		self.entry(loc)
			.map(|(entry, page)| {
				if page.position.contains(PagePosition::Last) {
					book_meta
						.spine
						.get(entry.spine as usize + 1)
						.map(|item| Location {
							spine: item.index,
							element: item.elements.start,
						})
						// End of book
						.unwrap_or(loc)
				} else {
					entry
						.pages
						.iter()
						.find(|p| p.elements.start > page.elements.start)
						.or(entry.pages.last())
						.map(|p| Location {
							spine: entry.spine,
							element: p.elements.start,
						})
						.expect("Programmer error, not last page but nothing after")
				}
			})
			.unwrap_or(loc)
	}

	fn previous_page(&self, book_meta: &BookMeta, loc: Location) -> Location {
		self.entry(loc)
			.map(|(entry, page)| {
				if page.position.contains(PagePosition::First) {
					book_meta
						.spine
						.get(entry.spine.saturating_sub(1) as usize)
						.map(|item| Location {
							spine: item.index,
							element: item.elements.end,
						})
						// Start of book
						.unwrap_or(loc)
				} else {
					entry
						.pages
						.iter()
						.rfind(|p| p.elements.start < page.elements.start)
						.map(|p| Location {
							spine: entry.spine,
							element: p.elements.start,
						})
						.expect("Programmer error, not first page but nothing before")
				}
			})
			.unwrap_or(loc)
	}

	fn is_cached(&self, spine_item: &BookSpineItem) -> bool {
		self.entries
			.iter()
			.flatten()
			.any(|e| e.spine == spine_item.index)
	}

	fn insert(&mut self, spine_item: &BookSpineItem, pages: Vec<PageContent>) {
		debug_assert!(
			pages.iter().is_sorted_by_key(|p| p.elements.start),
			"Pages not sorted"
		);

		self.entries[self.index % CACHE_CHAPTERS] = Some(PageCacheEntry {
			spine: spine_item.index,
			elements: spine_item.elements.clone(),
			pages,
		});
		self.index += 1;
	}

	pub fn clear(&mut self) {
		self.entries = [const { None }; CACHE_CHAPTERS];
	}
}

pub struct IllustratorHandle {
	req_tx: Sender<Request>,
	#[allow(unused)]
	handle: JoinHandle<Result<(), IllustratorError>>,
	pub toc: Arc<RwLock<IllustratorToC>>,
	location: Arc<RwLock<Location>>,
	pub font_system: Arc<Mutex<FontSystem>>,
	cache: Arc<RwLock<PageContentCache>>,
}

impl IllustratorHandle {
	pub fn location(&self) -> Location {
		*self.location.read().unwrap()
	}

	pub fn cache<'a>(&'a self) -> RwLockReadGuard<'a, PageContentCache> {
		self.cache.read().unwrap()
	}

	pub fn goto(&mut self, loc: Location) -> Result<(), IllustratorRequestError> {
		self.req_tx
			.send(Request::Goto(loc))
			.map_err(|_| IllustratorRequestError::NotRunning)
	}

	pub fn next_page(&mut self) -> Result<(), IllustratorRequestError> {
		self.req_tx
			.send(Request::NextPage)
			.map_err(|_| IllustratorRequestError::NotRunning)
	}

	pub fn previous_page(&mut self) -> Result<(), IllustratorRequestError> {
		self.req_tx
			.send(Request::PreviousPage)
			.map_err(|_| IllustratorRequestError::NotRunning)
	}
}

pub struct IllustratorToCItem {
	pub title: Arc<String>,
	pub location: Location,
}

#[derive(Default)]
pub struct IllustratorToC {
	pub items: Vec<IllustratorToCItem>,
}

#[derive(Debug, Clone)]
pub struct Params {
	page_width: u32,
	page_height: u32,
	scale: f32,
}

pub struct Illustrator {
	params: Params,
	config: ScribeConfig,
	sculpter: Arc<Sculpter>,
	font_system: Arc<Mutex<FontSystem>>,
	cache: Arc<RwLock<PageContentCache>>,
	req_tx: Option<Sender<Request>>,
}

#[cfg(target_os = "android")]
fn create_font_system() -> FontSystem {
	let mut font_system = FontSystem::new();
	font_system.db_mut().load_fonts_dir("/system/fonts");
	font_system.db_mut().set_sans_serif_family("Roboto");
	font_system.db_mut().set_serif_family("Noto Serif");
	font_system.db_mut().set_monospace_family("Droid Sans Mono"); // Cutive Mono looks more printer-like
	font_system.db_mut().set_cursive_family("Dancing Script");
	font_system.db_mut().set_fantasy_family("Dancing Script");
	font_system
}

impl Illustrator {
	pub fn create(config: ScribeConfig) -> Result<Self, SculpterLoadError> {
		#[cfg(target_os = "android")]
		let font_system = Arc::new(Mutex::new(create_font_system()));
		#[cfg(not(target_os = "android"))]
		let font_system = Arc::new(Mutex::new(FontSystem::new()));
		let params = Params {
			page_height: 800,
			page_width: 600,
			scale: 1.0,
		};
		let sculpter = {
			let mut sculpter = Sculpter::new();
			sculpter.load_builtin_fonts()?;
			Arc::new(sculpter)
		};

		let cache = Arc::new(RwLock::new(PageContentCache::default()));

		Ok(Self {
			params,
			config,
			sculpter,
			font_system,
			cache,
			req_tx: None,
		})
	}

	pub fn font_system(&self) -> LockResult<MutexGuard<'_, FontSystem>> {
		self.font_system.lock()
	}

	pub fn state(&self) -> LockResult<RwLockReadGuard<'_, PageContentCache>> {
		self.cache.read()
	}

	pub fn resize(&mut self, width: u32, height: u32) {
		log::debug!("Resize event {width}/{height}");
		self.params.page_width = width;
		self.params.page_height = height;

		if let Some(req_tx) = &self.req_tx {
			match req_tx.send(Request::Resize { width, height }) {
				Ok(_) => {}
				Err(e) => {
					log::info!("Error on illustrator channel, close: {e}");
					self.req_tx = None;
				}
			}
		}
	}

	pub fn rescale(&mut self, scale: f32) {
		log::debug!("Rescale event {scale}");
		self.params.scale = scale;

		if let Some(req_tx) = &self.req_tx {
			match req_tx.send(Request::Rescale { scale }) {
				Ok(_) => {}
				Err(e) => {
					log::info!("Error on illustrator channel, close: {e}");
					self.req_tx = None;
				}
			}
		}
	}
}

pub trait Bell {
	fn content_ready(&self, id: library::BookId, loc: Location);
}

#[derive(Debug)]
pub enum Request {
	Goto(Location),
	NextPage,
	PreviousPage,
	Resize { width: u32, height: u32 },
	Rescale { scale: f32 },
}

#[derive(Clone)]
struct SharedVec(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedVec {
	fn as_ref(&self) -> &[u8] {
		let Self(data) = self;
		data
	}
}

#[derive(Debug)]
struct BookResource {
	path: PathBuf,
	#[allow(unused)]
	mime: mime::Mime,
}

impl BookResource {
	fn new(path: PathBuf, mime: &str) -> Self {
		Self {
			path,
			mime: mime.parse().unwrap_or(mime::TEXT_HTML),
		}
	}
}

#[derive(Debug, Deserialize)]
pub(crate) struct NavLabel {
	text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Content {
	#[serde(rename = "@src")]
	src: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NavPoint {
	#[serde(rename = "@id")]
	id: String,
	#[serde(rename = "navLabel")]
	nav_label: NavLabel,
	#[serde(rename = "content")]
	content: Content,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NavMap {
	#[serde(rename = "navPoint")]
	nav_points: Vec<NavPoint>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Navigation {
	#[serde(rename = "navMap")]
	nav_map: NavMap,
}

fn read_navigation(s: &str) -> Result<Navigation, quick_xml::de::DeError> {
	quick_xml::de::from_str(s)
}

impl Navigation {
	fn into_toc(self, spine: &[BookSpineItem]) -> IllustratorToC {
		let spine_lookup = spine
			.iter()
			.enumerate()
			.map(|(i, s)| (&s.idref, i))
			.collect::<BTreeMap<_, _>>();
		let mut items = Vec::new();
		for nav_point in &self.nav_map.nav_points {
			if let Some(spine) = spine_lookup.get(&nav_point.content.src) {
				items.push(IllustratorToCItem {
					title: Arc::new(nav_point.nav_label.text.clone()),
					location: Location {
						spine: *spine as u32,
						element: 0,
					},
				});
			} else {
				log::warn!(
					"Failed to find {} {} in spine",
					nav_point.id,
					nav_point.nav_label.text
				);
			}
		}
		IllustratorToC { items }
	}
}

#[derive(Debug)]
struct BookSpineItem {
	index: u32,
	idref: String,
	elements: Range<u32>,
}

#[allow(unused)]
#[derive(Debug)]
struct BookMeta {
	resources: BTreeMap<String, BookResource>,
	spine: Vec<BookSpineItem>,
	cover_id: Option<String>,
}

impl BookMeta {
	fn create<R: io::Seek + io::Read>(
		epub: EpubDoc<R>,
		archive: &mut ZipArchive<R>,
	) -> Result<Self, IllustratorError> {
		let start = Instant::now();
		let EpubDoc {
			resources,
			spine,
			metadata,
			..
		} = epub;
		let cover_id = metadata
			.iter()
			.find(|data| data.property == "cover")
			.map(|m| m.value.clone());
		let title = metadata
			.iter()
			.find(|data| data.property == "title")
			.map(|m| m.value.clone());

		let resources = resources
			.into_iter()
			.map(|(key, res)| (key, BookResource::new(res.path, &res.mime)))
			.collect::<BTreeMap<_, _>>();
		let spine = {
			let mut builder = NodeTreeBuilder::new();
			let mut items = Vec::new();
			for (index, item) in spine.into_iter().enumerate() {
				let res = resources
					.get(&item.idref)
					.ok_or_else(|| IllustratorError::MissingResource(item.idref.clone()))?;
				let file = archive.by_path(&res.path)?;
				let tree = builder.read_from(file)?;
				let node_count = tree.tree.node_count();
				items.push(BookSpineItem {
					index: index as u32,
					idref: item.idref,
					elements: 0..node_count,
				});
				builder = tree.into_builder();
			}
			items
		};
		let dur = Instant::now().duration_since(start);
		log::info!("Opened {:?} in {}", title, dur.as_secs_f64());

		Ok(Self {
			resources,
			spine,
			cover_id,
		})
	}

	fn spine_resource(&self, loc: Location) -> Option<(&BookSpineItem, &Path)> {
		self.spine
			.get(loc.spine as usize)
			.and_then(|s| self.resources.get(&s.idref).map(|r| (s, r.path.as_path())))
	}
}

fn read_book_meta(
	bytes: SharedVec,
	archive: &mut ZipArchive<Cursor<SharedVec>>,
) -> Result<(BookMeta, Option<IllustratorToC>), IllustratorError> {
	let doc = EpubDoc::from_reader(Cursor::new(bytes.clone()))
		.inspect_err(|e| log::error!("Error: {e}"))?;
	let book_meta = BookMeta::create(doc, archive)?;
	let toc = book_meta.resources.get("ncx").and_then(|res| {
		let mut buf = String::new();
		archive
			.by_path(&res.path)
			.inspect_err(|e| log::error!("Error: {e}"))
			.ok()?
			.read_to_string(&mut buf)
			.inspect_err(|e| log::error!("Error: {e}"))
			.ok()?;
		let nav = read_navigation(&buf)
			.inspect_err(|e| log::error!("Error: {e}"))
			.ok()?;
		Some(nav.into_toc(&book_meta.spine))
	});
	Ok((book_meta, toc))
}

#[derive(Debug)]
pub struct Position {
	pub x: u32,
	pub y: u32,
}

impl From<taffy::Point<f32>> for Position {
	fn from(value: taffy::Point<f32>) -> Self {
		Self {
			x: value.x.round() as u32,
			y: value.y.round() as u32,
		}
	}
}

#[derive(Debug)]
pub struct Size {
	pub width: u32,
	pub height: u32,
}

impl From<taffy::Size<f32>> for Size {
	fn from(value: taffy::Size<f32>) -> Self {
		Self {
			width: value.width.round() as u32,
			height: value.height.round() as u32,
		}
	}
}

#[derive(Debug)]
pub struct DisplayText {
	pub buffer: Buffer,
}

#[derive(Debug)]
pub struct DisplayPixmap {
	pub pixmap_width: u32,
	pub pixmap_height: u32,
	pub pixmap_rgba: Vec<u8>,
}

#[derive(Debug)]
pub enum DisplayContent {
	Text(DisplayText),
	Pixmap(DisplayPixmap),
}

#[derive(Debug)]
pub struct DisplayItem {
	pub pos: Position,
	pub size: Size,
	pub content: DisplayContent,
}

bitflags! {
	#[derive(Debug)]
	pub struct PagePosition: u8 {
		const First = 1;
		const Last  = 1 << 1;
	}
}

#[derive(Debug)]
pub struct PageContent {
	pub position: PagePosition,
	pub index: u32,
	pub elements: Range<u32>,
	pub items: Vec<DisplayItem>,
}

struct RenderSettings<'a> {
	page_width: u32,
	page_height: u32,
	scale: f32,

	padding_top_em: f32,
	padding_left_em: f32,
	padding_right_em: f32,
	padding_bottom_em: f32,
	padding_paragraph_em: f32,

	body_metrics: cosmic_text::Metrics,
	body: cosmic_text::Attrs<'a>,
	bold: cosmic_text::Attrs<'a>,
	italic: cosmic_text::Attrs<'a>,
	h1: cosmic_text::Attrs<'a>,
	h2: cosmic_text::Attrs<'a>,
	h3: cosmic_text::Attrs<'a>,
	h4: cosmic_text::Attrs<'a>,
	h5: cosmic_text::Attrs<'a>,
}

#[derive(Debug, Default, Clone, Copy)]
enum TextStyle {
	#[default]
	Body,
	Bold,
	Italic,
	H1,
	H2,
	H3,
	H4,
	H5,
}

impl<'a> RenderSettings<'a> {
	fn new(params: &Params, config: &'a scribe::settings::Illustrator) -> Self {
		use cosmic_text::Attrs;
		use cosmic_text::Family;
		use cosmic_text::Metrics;
		use cosmic_text::Style;
		use cosmic_text::Weight;

		let family = match config.font_regular.family.as_str() {
			"serif" => Family::Serif,
			"sans-serif" => Family::SansSerif,
			"fantasy" => Family::Fantasy,
			"cursive" => Family::Cursive,
			"monospace" => Family::Monospace,
			family => Family::Name(family),
		};
		let size = config.font_size_px * params.scale;
		let line = config.line_height;

		let body_metrics = Metrics::relative(size, line);
		let body = Attrs::new().family(family);
		let bold = body.clone().weight(Weight::BOLD);
		let italic = body.clone().style(Style::Italic);

		let h1 = body
			.clone()
			.metrics(Metrics::relative(size * config.h1.font_size_em, line));
		let h2 = body
			.clone()
			.metrics(Metrics::relative(size * config.h2.font_size_em, line));
		let h3 = body
			.clone()
			.metrics(Metrics::relative(size * config.h3.font_size_em, line));
		let h4 = body
			.clone()
			.metrics(Metrics::relative(size * config.h4.font_size_em, line));
		let h5 = body
			.clone()
			.metrics(Metrics::relative(size * config.h5.font_size_em, line));

		let padding_top_em = config.padding.top_em;
		let padding_left_em = config.padding.left_em;
		let padding_right_em = config.padding.right_em;
		let padding_bottom_em = config.padding.bottom_em;
		let padding_paragraph_em = config.padding.paragraph_em;

		RenderSettings {
			page_width: params.page_width,
			page_height: params.page_height,
			scale: params.scale,

			padding_top_em,
			padding_left_em,
			padding_right_em,
			padding_bottom_em,
			padding_paragraph_em,

			body_metrics,
			body,
			bold,
			italic,
			h1,
			h2,
			h3,
			h4,
			h5,
		}
	}

	fn text_attrs(&self, style: TextStyle) -> &Attrs<'a> {
		match style {
			TextStyle::Body => &self.body,
			TextStyle::Bold => &self.bold,
			TextStyle::Italic => &self.italic,
			TextStyle::H1 => &self.h1,
			TextStyle::H2 => &self.h2,
			TextStyle::H3 => &self.h3,
			TextStyle::H4 => &self.h4,
			TextStyle::H5 => &self.h5,
		}
	}

	fn page_height_padded(&self) -> f32 {
		self.page_height as f32
			- self.padding_top_em * self.em()
			- self.padding_bottom_em * self.em()
	}
	fn page_width_padded(&self) -> f32 {
		self.page_width as f32
			- self.padding_left_em * self.em()
			- self.padding_right_em * self.em()
	}

	fn paragraph_padding(&self) -> f32 {
		self.padding_paragraph_em * self.em()
	}

	fn em(&self) -> f32 {
		self.body_metrics.font_size * self.scale
	}
}

#[must_use = "Must track handle or illustrator dies"]
pub fn spawn_illustrator(
	illustrator: &mut Illustrator,
	bell: impl Bell + Send + 'static,
	id: library::BookId,
) -> Result<IllustratorHandle, IllustratorSpawnError> {
	let state_path = illustrator.config.paths().data_path.join("state.db");
	let records = scribe::record_keeper::create(&state_path)?;

	log::info!("Open book {id}");
	let book = records.fetch_book(id)?;

	let mut params = illustrator.params.clone();
	let config = illustrator.config.clone();

	let sculpter = illustrator.sculpter.clone();
	let font_system = illustrator.font_system.clone();
	let cache = illustrator.cache.clone();

	let (req_tx, req_rx) = channel();
	illustrator.req_tx = Some(req_tx.clone());

	let handle_toc = Arc::new(RwLock::new(IllustratorToC::default()));
	let handle_loc = Arc::new(RwLock::new(book.location()));

	let shared_toc = handle_toc.clone();
	let shared_loc = handle_loc.clone();

	let handle = std::thread::spawn(move || -> Result<(), IllustratorError> {
		log::trace!("Launching illustrator");
		let bytes = SharedVec(Arc::new(
			fs::read(&book.path).inspect_err(|e| log::error!("Error: {e}"))?,
		));
		let illustrator_config = config
			.illustrator()
			.inspect_err(|e| log::error!("Config error: {e}"))?;

		let font_opts = vec![
			FontOptions::new(sculpter::Family::SansSerif),
			FontOptions::new(sculpter::Family::SansSerif).with_variations(
				[ttf_parser::Variation {
					axis: ttf_parser::Tag::from_bytes(b"wght"),
					value: 500.0,
				}]
				.into_iter(),
			),
		];
		let (style_refs, _shaper, _printer) = sculpter.create_shaper(params.scale, &font_opts)?;
		log::info!("Created style refs {style_refs:?}");

		let (archive, book_meta, toc) = {
			let mut archive = ZipArchive::new(Cursor::new(bytes.clone()))
				.inspect_err(|e| log::error!("Zip error: {e}"))?;
			let (meta, toc) = read_book_meta(bytes, &mut archive)?;
			(Mutex::new(archive), meta, toc)
		};
		if let Some(toc) = toc {
			*shared_toc.write().unwrap() = toc;
		}

		log::info!(
			"Opened book with {} resources, {} spine items",
			book_meta.resources.len(),
			book_meta.spine.len()
		);

		cache.write().unwrap().clear();

		let book_loc = book.location();
		let mut current_loc = if book_meta.spine_resource(book_loc).is_some() {
			book_loc
		} else {
			log::error!("Invalid book location {book_loc:?}, reset to first page");
			Location {
				spine: 0,
				element: 0,
			}
		};

		let mut builder = NodeTreeBuilder::new();
		let mut taffy_tree = taffy::TaffyTree::new();
		loop {
			let req = match req_rx.try_recv() {
				Ok(req) => req,
				Err(TryRecvError::Empty) => {
					let settings = RenderSettings::new(&params, &illustrator_config);
					let (item, path) = book_meta
						.spine_resource(current_loc)
						.expect("On location without spine");
					builder = assure_cached(
						&settings,
						&font_system,
						&archive,
						&cache,
						builder,
						&mut taffy_tree,
						item,
						path,
					)
					.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					log::debug!("Save location {current_loc} in {}", book.id);
					*shared_loc.write().unwrap() = current_loc;
					records
						.record_book_state(id, Some(current_loc))
						.inspect_err(|e| log::error!("Error: {e}"))?;
					bell.content_ready(id, current_loc);

					match req_rx.recv() {
						Ok(req) => req,
						Err(_) => break,
					}
				}
				Err(TryRecvError::Disconnected) => {
					break;
				}
			};
			log::trace!("{req:?} {current_loc}");
			match req {
				Request::NextPage => {
					current_loc = cache.read().unwrap().next_page(&book_meta, current_loc);
				}
				Request::PreviousPage => {
					current_loc = cache.read().unwrap().previous_page(&book_meta, current_loc);
				}
				Request::Goto(loc) => {
					current_loc = loc;
				}
				Request::Resize { width, height } => {
					cache.write().unwrap().clear();
					params.page_width = width;
					params.page_height = height;
				}
				Request::Rescale { scale } => {
					cache.write().unwrap().clear();
					params.scale = scale;
				}
			}
		}

		log::info!("Illustrator worker terminated");
		Ok(())
	});

	let font_system = illustrator.font_system.clone();
	let cache = illustrator.cache.clone();
	Ok(IllustratorHandle {
		req_tx,
		handle,
		toc: handle_toc,
		location: handle_loc,
		font_system,
		cache,
	})
}

#[allow(clippy::too_many_arguments)]
fn assure_cached<R: io::Seek + io::Read + Sync + Send>(
	settings: &RenderSettings,
	font_system: &Mutex<FontSystem>,
	archive: &Mutex<ZipArchive<R>>,
	cache: &RwLock<PageContentCache>,
	builder: NodeTreeBuilder,
	taffy_tree: &mut taffy::TaffyTree<NodeContext>,
	item: &BookSpineItem,
	path: &Path,
) -> Result<NodeTreeBuilder, IllustratorError> {
	if cache.read().unwrap().is_cached(item) {
		log::trace!("Already cached {item:?}");
		Ok(builder)
	} else {
		log::info!("Refresh cache for {}", path.display());
		let start = Instant::now();
		let (pages, b) =
			render_resource(settings, font_system, archive, path, builder, taffy_tree)?;
		let mut cache = cache.write().unwrap();
		cache.insert(item, pages);
		drop(cache);
		let dur = Instant::now().duration_since(start);
		log::info!(
			"Render {} complete in {}",
			path.display(),
			dur.as_secs_f64()
		);
		Ok(b)
	}
}

pub enum Edge {
	Open(NodeId),
	Close(NodeId),
}

pub struct TaffyTreeIter<'a, C = ()> {
	tree: &'a TaffyTree<C>,
	stack: Vec<Edge>,
}

impl<'a, C> TaffyTreeIter<'a, C> {
	pub fn new(tree: &'a TaffyTree<C>, id: NodeId) -> Self {
		let stack = tree
			.children(id)
			.unwrap_or_default()
			.into_iter()
			.rev()
			.map(Edge::Open)
			.collect();
		Self { tree, stack }
	}
}

impl<'a, C> Iterator for TaffyTreeIter<'a, C> {
	type Item = Edge;

	fn next(&mut self) -> Option<Self::Item> {
		let node = self.stack.pop()?;
		if let Edge::Open(id) = node {
			self.stack.push(Edge::Close(id));
			let children = self
				.tree
				.children(id)
				.unwrap_or_default()
				.into_iter()
				.rev()
				.map(Edge::Open);
			self.stack.extend(children);
		}
		Some(node)
	}
}

#[derive(Debug)]
enum NodeContent {
	Block,
	Text,
	Svg { scale: f32 },
}

#[derive(Debug)]
#[allow(unused)]
struct NodeContext {
	element: u32,
	content: NodeContent,
}

impl NodeContext {
	fn block(element: u32) -> Self {
		Self {
			element,
			content: NodeContent::Block,
		}
	}

	fn text(element: u32) -> Self {
		Self {
			element,
			content: NodeContent::Text,
		}
	}

	fn svg(element: u32, scale: f32) -> Self {
		Self {
			element,
			content: NodeContent::Svg { scale },
		}
	}
}

fn render_resource<R: io::Seek + io::Read + Sync + Send>(
	settings: &RenderSettings,
	font_system: &Mutex<FontSystem>,
	archive: &Mutex<ZipArchive<R>>,
	path: &Path,
	builder: NodeTreeBuilder,
	tree: &mut taffy::TaffyTree<NodeContext>,
) -> Result<(Vec<PageContent>, NodeTreeBuilder), IllustratorRenderError> {
	let node_tree = {
		let mut archive = archive.lock().unwrap();
		let file = archive.by_path(path)?;
		builder.read_from(file)?
	};

	let body: NodeId = tree.new_leaf(Style {
		size: taffy::Size {
			width: length(settings.page_width_padded()),
			height: auto(),
		},
		..Default::default()
	})?;
	let options = svg_options(archive, path.parent().unwrap_or(Path::new("OEBPS/")));

	let mut current = body;
	let mut buffers = HashMap::new();
	let mut text_styles = Vec::new();
	let mut texts = Vec::new();
	let mut svgs = HashMap::new();
	let mut svg_buf = String::new();

	let page_height = settings.page_height_padded();
	let page_width = settings.page_width_padded();

	let mut node_iter = node_tree
		.body_iter()
		.ok_or(IllustratorRenderError::MissingBody)?;
	while let Some(edge) = node_iter.next() {
		match edge {
			EdgeRef::OpenElement(el) if el.local_name() == &local_name!("svg") => {
				let svg = read_svg(&mut svg_buf, &el, &mut node_iter, &options)?;
				let size = svg.size();
				let scale = scale_to_fit(size.width(), size.height(), page_width, page_height);
				let style = Style {
					size: taffy::Size::from_lengths(size.width() * scale, size.height() * scale),
					..Default::default()
				};
				let node =
					tree.new_leaf_with_context(style, NodeContext::svg(el.id.value(), scale))?;
				svgs.insert(node, svg);
				tree.add_child(current, node)?;
			}
			EdgeRef::OpenElement(el) if is_inline(el.local_name()) => {
				if let Some(text_style) = text_style(el.local_name()) {
					text_styles.push((el.id, text_style))
				}
			}
			EdgeRef::OpenElement(el) => {
				if let Some(text_style) = text_style(el.local_name()) {
					text_styles.push((el.id, text_style))
				}
				if !texts.is_empty() {
					let node = tree.new_leaf_with_context(
						Style::default(),
						NodeContext::text(el.id.value()),
					)?;
					buffers.entry(node).or_insert_with(|| {
						create_text(
							settings,
							&mut font_system.lock().unwrap(),
							std::mem::take(&mut texts),
						)
					});
					tree.add_child(current, node)?;
				}
				let node = tree.new_leaf_with_context(
					element_style(settings, el.local_name()),
					NodeContext::block(el.id.value()),
				)?;
				tree.add_child(current, node)?;
				current = node;
			}
			EdgeRef::CloseElement(id, name) if is_inline(&name.local) => {
				if text_styles.last().is_some_and(|(el_id, _)| *el_id == id) {
					text_styles.pop();
				}
			}
			EdgeRef::CloseElement(id, _name) => {
				if text_styles.last().is_some_and(|(el_id, _)| *el_id == id) {
					text_styles.pop();
				}
				if !texts.is_empty() {
					let node = tree
						.new_leaf_with_context(Style::default(), NodeContext::text(id.value()))?;
					buffers.entry(node).or_insert_with(|| {
						create_text(
							settings,
							&mut font_system.lock().unwrap(),
							std::mem::take(&mut texts),
						)
					});
					tree.add_child(current, node)?;
				}
				current = tree
					.parent(current)
					.ok_or(IllustratorRenderError::UnexpectedExtraClose)?;
			}
			EdgeRef::Text(TextWrapper { t: Text { t }, .. }) => {
				let text_style = text_styles.last().map(|(_, s)| *s).unwrap_or_default();
				texts.push((t, text_style));
			}
		}
	}

	debug_assert!(texts.is_empty());
	debug_assert!(text_styles.is_empty());
	drop(texts);
	drop(text_styles);

	tree.compute_layout_with_measure(
		body,
		taffy::Size::MAX_CONTENT,
		|known_dimensions, available_space, node_id, _node_context, _style| match buffers
			.get_mut(&node_id)
		{
			Some(buffer) => measure_text(
				&mut font_system.lock().unwrap(),
				buffer,
				known_dimensions,
				available_space,
			),
			None => taffy::Size::ZERO,
		},
	)?;

	let padding_top = settings.padding_top_em * settings.em();
	let padding_left = settings.padding_left_em * settings.em();

	let mut page_end = 0.;
	let mut offset = taffy::Point::<f32>::zero();
	let mut pages = Vec::new();
	let mut page = PageContent {
		position: PagePosition::First,
		index: 0,
		elements: 0..0,
		items: Vec::new(),
	};
	for edge in TaffyTreeIter::new(tree, body) {
		match edge {
			Edge::Open(id) => {
				let l = tree.layout(id)?;
				offset = taffy::Point {
					x: offset.x + l.location.x,
					y: offset.y + l.location.y,
				};

				if let Some(ctx) = tree.get_node_context(id) {
					page.elements.end = ctx.element;

					if let Some(content) = create_display_item(&mut buffers, &mut svgs, id, ctx)? {
						let content_end = offset.y + l.size.height;
						if content_end - page_end > page_height {
							log::debug!("Page break at el {}", page.elements.end,);

							let index = page.index + 1;
							let elements_end = page.elements.end;
							pages.push(page);
							page = PageContent {
								position: PagePosition::empty(),
								index,
								elements: elements_end..elements_end,
								items: Vec::new(),
							};
							page_end = offset.y;
						}

						page.items.push(DisplayItem {
							pos: taffy::Point {
								x: padding_left + offset.x,
								y: padding_top + offset.y - page_end,
							}
							.into(),
							size: l.size.into(),
							content,
						});
					}
				}
			}
			Edge::Close(id) => {
				let l = tree.layout(id)?;
				offset = taffy::Point {
					x: offset.x - l.location.x,
					y: offset.y - l.location.y,
				};
			}
		}
	}
	if !page.items.is_empty() || pages.is_empty() {
		page.position.set(PagePosition::Last, true);
		pages.push(page);
	} else if let Some(last) = pages.last_mut() {
		last.position.set(PagePosition::Last, true);
	}
	log::trace!("Generated {} pages", pages.len());
	debug_assert!(!pages.is_empty(), "Must have at least one page per chapter");

	Ok((pages, node_tree.into_builder()))
}

fn create_display_item(
	buffers: &mut HashMap<NodeId, Buffer>,
	svgs: &mut HashMap<NodeId, usvg::Tree>,
	id: NodeId,
	ctx: &NodeContext,
) -> Result<Option<DisplayContent>, IllustratorRenderError> {
	Ok(match ctx.content {
		NodeContent::Block => None,
		NodeContent::Text => buffers
			.remove(&id)
			.map(|buffer| DisplayContent::Text(DisplayText { buffer })),
		NodeContent::Svg { scale } => {
			if let Some(tree) = svgs.remove(&id) {
				let pixmap_size = tree
					.size()
					.to_int_size()
					.scale_by(scale)
					.ok_or(IllustratorRenderError::ScaleSvgFailed)?;
				let transform = tiny_skia::Transform::from_scale(scale, scale);
				let mut pixmap =
					tiny_skia::Pixmap::new(pixmap_size.width(), pixmap_size.height()).unwrap();
				resvg::render(&tree, transform, &mut pixmap.as_mut());

				Some(DisplayContent::Pixmap(DisplayPixmap {
					pixmap_width: pixmap.width(),
					pixmap_height: pixmap.height(),
					pixmap_rgba: pixmap.take(),
				}))
			} else {
				None
			}
		}
	})
}

fn scale_to_fit(width: f32, height: f32, max_width: f32, max_height: f32) -> f32 {
	if width < max_width && height < max_height {
		1.0
	} else {
		let ws = max_width / width;
		let hs = max_height / height;
		ws.min(hs)
	}
}

fn svg_options<'a, R: io::Seek + io::Read + Sync + Send>(
	archive: &'a Mutex<ZipArchive<R>>,
	base_path: &'a Path,
) -> usvg::Options<'a> {
	usvg::Options {
		image_href_resolver: usvg::ImageHrefResolver {
			resolve_string: Box::new(move |href: &str, opts: &usvg::Options| {
				let path = Path::new(href);
				let data = {
					let mut archive = archive.lock().unwrap();
					let file = if path.is_absolute() {
						archive.by_path(path)
					} else {
						archive.by_path(base_path.join(path))
					};
					match file {
						Ok(mut f) => {
							let mut content = Vec::new();
							match f.read_to_end(&mut content) {
								Ok(_) => Some(content),
								Err(e) => {
									log::warn!("Failed to load '{href}': {e}");
									None
								}
							}
						}
						Err(e) => {
							log::warn!("Failed to load '{href}': {e}");
							None
						}
					}
				};

				if let Some(data) = data {
					let ext = path.extension().and_then(|e| e.to_str())?.to_lowercase();
					if ext == "svg" || ext == "svgz" {
						loab_sub_svg(data.as_slice(), opts)
					} else {
						match imagesize::image_type(&data) {
							Ok(imagesize::ImageType::Gif) => {
								Some(usvg::ImageKind::GIF(Arc::new(data)))
							}
							Ok(imagesize::ImageType::Png) => {
								Some(usvg::ImageKind::PNG(Arc::new(data)))
							}
							Ok(imagesize::ImageType::Jpeg) => {
								Some(usvg::ImageKind::JPEG(Arc::new(data)))
							}
							Ok(imagesize::ImageType::Webp) => {
								Some(usvg::ImageKind::WEBP(Arc::new(data)))
							}
							Ok(image_type) => {
								log::warn!("unknown image type of '{href}': {image_type:?}");
								None
							}
							Err(e) => {
								log::warn!("error decoding image type of '{href}': {e}");
								None
							}
						}
					}
				} else {
					log::warn!("Not an image file '{href}'");
					None
				}
			}),
			..Default::default()
		},
		..Default::default()
	}
}

// Extracted from usvg/src/parser/image.rs and modified to fit
fn loab_sub_svg(data: &[u8], opts: &usvg::Options<'_>) -> Option<usvg::ImageKind> {
	let sub_opts = usvg::Options {
		resources_dir: None,
		dpi: opts.dpi,
		font_size: opts.font_size,
		shape_rendering: opts.shape_rendering,
		text_rendering: opts.text_rendering,
		image_rendering: opts.image_rendering,
		default_size: opts.default_size,
		// The referenced SVG image cannot have any 'image' elements by itself.
		// Not only recursive. Any. Don't know why.
		image_href_resolver: usvg::ImageHrefResolver {
			resolve_data: Box::new(|_, _, _| None),
			resolve_string: Box::new(|_, _| None),
		},
		..Default::default()
	};

	let tree = usvg::Tree::from_data(data, &sub_opts);
	let tree = match tree {
		Ok(tree) => tree,
		Err(e) => {
			log::warn!("Failed to load subsvg image: {e}");
			return None;
		}
	};

	Some(usvg::ImageKind::SVG(tree))
}

fn read_svg(
	buf: &mut String,
	el: &html_parser::ElementWrapper<'_>,
	node_iter: &mut html_parser::NodeTreeIter<'_>,
	options: &usvg::Options,
) -> Result<usvg::Tree, IllustratorSvgError> {
	buf.clear();
	write_begin_node(buf, el.el)?;
	for edge in node_iter.by_ref() {
		match edge {
			EdgeRef::CloseElement(id, _name) if id == el.id => {
				break;
			}
			EdgeRef::OpenElement(el) => write_begin_node(buf, el.el)?,
			EdgeRef::CloseElement(_id, name) => {
				write_end_node(buf, &name)?;
			}
			EdgeRef::Text(TextWrapper { t, .. }) => {
				write!(buf, "{}", t.t)?;
			}
		}
	}
	write_end_node(buf, el.name())?;
	log::debug!("Svg node '''\n{buf}\n'''");

	let svg = usvg::Tree::from_str(buf.as_str(), options)?;
	Ok(svg)
}

fn write_begin_node<W: fmt::Write>(w: &mut W, el: &html_parser::Element) -> Result<(), fmt::Error> {
	write!(w, "<")?;
	if let Some(prefix) = &el.name.prefix
		&& !prefix.is_empty()
	{
		write!(w, "{}:", prefix)?;
	}
	write!(w, "{}", el.name.local)?;

	for (name, value) in &el.attrs {
		write!(w, " ")?;
		if let Some(prefix) = &name.prefix {
			write!(w, "{}:", prefix)?;
		}
		write!(w, r#"{}="{}""#, name.local, value)?;
	}

	write!(w, ">")
}

fn write_end_node<W: fmt::Write>(w: &mut W, name: &html5ever::QualName) -> Result<(), fmt::Error> {
	write!(w, "</")?;
	if let Some(prefix) = &name.prefix
		&& !prefix.is_empty()
	{
		write!(w, "{}:", prefix)?;
	}
	write!(w, "{}", name.local)?;
	write!(w, ">")
}

fn measure_text(
	font_system: &mut FontSystem,
	buffer: &mut Buffer,
	known_dimensions: taffy::Size<Option<f32>>,
	available_space: taffy::Size<taffy::AvailableSpace>,
) -> taffy::Size<f32> {
	let width_constraint = known_dimensions.width.or(match available_space.width {
		AvailableSpace::MinContent => Some(0.0),
		AvailableSpace::MaxContent => None,
		AvailableSpace::Definite(width) => Some(width),
	});

	buffer.set_wrap(font_system, cosmic_text::Wrap::WordOrGlyph);
	buffer.set_size(font_system, width_constraint, None);
	buffer.shape_until_scroll(font_system, false);

	let (width, total_lines) = buffer
		.layout_runs()
		.fold((0.0, 0usize), |(width, total_lines), run| {
			(run.line_w.max(width), total_lines + 1)
		});
	let metrics = buffer.metrics();
	let height = total_lines as f32 * metrics.line_height;

	taffy::Size { width, height }
}

fn element_style(settings: &RenderSettings, name: &LocalName) -> Style {
	match *name {
		local_name!("p") => Style {
			padding: Rect {
				top: zero(),
				bottom: length(settings.paragraph_padding()),
				left: zero(),
				right: zero(),
			},
			..Style::default()
		},
		_ => Style::default(),
	}
}

fn create_text(
	settings: &RenderSettings,
	font_system: &mut FontSystem,
	texts: Vec<(&str, TextStyle)>,
) -> Buffer {
	let text_style = texts.first().map(|(_, s)| *s).unwrap_or_default();
	let attrs = settings.text_attrs(text_style);
	let metrics = attrs
		.metrics_opt
		.map(|m| m.into())
		.unwrap_or(settings.body_metrics);
	let texts = texts
		.into_iter()
		.map(|(t, s)| (t, settings.text_attrs(s).clone()));

	let mut buffer = Buffer::new(font_system, metrics);
	buffer.set_rich_text(font_system, texts, attrs, Shaping::Advanced, None);
	buffer
}

fn text_style(name: &LocalName) -> Option<TextStyle> {
	match *name {
		local_name!("b") | local_name!("strong") => Some(TextStyle::Bold),
		local_name!("i") | local_name!("em") => Some(TextStyle::Italic),
		local_name!("h1") => Some(TextStyle::H1),
		local_name!("h2") => Some(TextStyle::H2),
		local_name!("h3") => Some(TextStyle::H3),
		local_name!("h4") => Some(TextStyle::H4),
		local_name!("h5") => Some(TextStyle::H5),
		_ => None,
	}
}

fn is_inline(name: &LocalName) -> bool {
	name == &local_name!("strong")
		|| name == &local_name!("b")
		|| name == &local_name!("em")
		|| name == &local_name!("i")
		|| name == &local_name!("span")
}

#[cfg(test)]
mod tests {
	use quick_xml::de::from_str;

	#[test]
	fn test_parse_basic() -> Result<(), quick_xml::de::DeError> {
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
      <content src="cover.xhtml"/>
    </navPoint>
  </navMap>
</ncx>"#;

		let _nav_map: super::Navigation = from_str(input)?;
		Ok(())
	}
}
