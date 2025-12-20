#![feature(new_range_api)]
#![feature(mapped_lock_guards)]
mod error;
mod html_parser;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::io::Cursor;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::range::Range;
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
use scribe::library;
use scribe::library::Location;
use serde::Deserialize;
use taffy::prelude::*;
use zip::ZipArchive;
use zip::read::ZipFile;

use crate::error::IllustratorError;
use crate::error::IllustratorRenderError;
use crate::error::IllustratorRequestError;
use crate::error::IllustratorSpawnError;
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
			elements: spine_item.elements,
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
	pub location: Arc<RwLock<Location>>,
}

impl IllustratorHandle {
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

pub struct Illustrator {
	state_db_path: PathBuf,
	font_system: Arc<Mutex<FontSystem>>,
	settings: Arc<RwLock<RenderSettings>>,
	cache: Arc<RwLock<PageContentCache>>,
	req_tx: Option<Sender<Request>>,
	settings_changed: bool,
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
	pub fn new(state_path: PathBuf, settings: RenderSettings) -> Self {
		#[cfg(target_os = "android")]
		let font_system = create_font_system();
		#[cfg(not(target_os = "android"))]
		let font_system = FontSystem::new();
		Self {
			state_db_path: state_path,
			font_system: Arc::new(Mutex::new(font_system)),
			settings: Arc::new(RwLock::new(settings)),
			cache: Arc::new(RwLock::new(PageContentCache::default())),
			req_tx: None,
			settings_changed: false,
		}
	}

	pub fn font_system(&self) -> LockResult<MutexGuard<'_, FontSystem>> {
		self.font_system.lock()
	}

	pub fn state(&self) -> LockResult<RwLockReadGuard<'_, PageContentCache>> {
		self.cache.read()
	}

	pub fn resize(&mut self, width: u32, height: u32) {
		log::debug!("Resize event {width}/{height}");
		let mut settings = self.settings.write().unwrap();
		settings.page_width = width;
		settings.page_height = height;
		self.settings_changed = true;
	}

	pub fn rescale(&mut self, scale: f32) {
		log::debug!("Rescale event {scale}");
		let mut settings = self.settings.write().unwrap();
		settings.scale = scale;
		self.settings_changed = true;
	}

	pub fn refresh_if_needed(&mut self) {
		if self.settings_changed {
			if let Some(req_tx) = &self.req_tx {
				match req_tx.send(Request::RefreshCache) {
					Ok(_) => {}
					Err(e) => {
						log::info!("Error on illustrator channel, close: {e}");
						self.req_tx = None;
					}
				}
			}
			self.settings_changed = false;
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
	RefreshCache,
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
			cover_id,
			metadata,
			..
		} = epub;
		let resources = resources
			.into_iter()
			.map(|(key, (path, mime))| (key, BookResource::new(path, &mime)))
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
					elements: Range::from(0..node_count),
				});
				builder = tree.into_builder();
			}
			items
		};
		let dur = Instant::now().duration_since(start);
		log::info!(
			"Opened {:?} in {}",
			metadata.get("title"),
			dur.as_secs_f64()
		);

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
pub struct DisplayText {
	pub pos: taffy::Point<f32>,
	pub size: taffy::Size<f32>,
	pub buffer: Buffer,
}

#[derive(Debug)]
pub enum DisplayItem {
	Text(DisplayText),
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

pub struct RenderTextSettings {
	pub font_size: f32,
	pub line_height: f32,
	pub attrs: cosmic_text::Attrs<'static>,
}

impl RenderTextSettings {
	fn metrics(&self, scale: f32) -> cosmic_text::Metrics {
		cosmic_text::Metrics::new(self.font_size * scale, self.line_height * scale)
	}
}

pub struct RenderSettings {
	pub page_width: u32,
	pub page_height: u32,
	pub scale: f32,

	pub padding_top_em: f32,
	pub padding_left_em: f32,
	pub padding_right_em: f32,
	pub padding_bottom_em: f32,
	pub padding_paragraph_em: f32,

	pub body_text: RenderTextSettings,
	pub h1_text: RenderTextSettings,
	pub h2_text: RenderTextSettings,
}

#[allow(unused)]
impl RenderSettings {
	fn body_text(&self) -> (cosmic_text::Metrics, Attrs<'static>) {
		(
			self.body_text.metrics(self.scale),
			self.body_text.attrs.clone(),
		)
	}
	fn h1_text(&self) -> (cosmic_text::Metrics, Attrs<'static>) {
		(self.h1_text.metrics(self.scale), self.h1_text.attrs.clone())
	}
	fn h2_text(&self) -> (cosmic_text::Metrics, Attrs<'static>) {
		(self.h2_text.metrics(self.scale), self.h2_text.attrs.clone())
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
		self.body_text.font_size * self.scale
	}
}

#[must_use = "Must track handle or illustrator dies"]
pub fn spawn_illustrator(
	illustrator: &mut Illustrator,
	bell: impl Bell + Send + 'static,
	id: library::BookId,
) -> Result<IllustratorHandle, IllustratorSpawnError> {
	let records = scribe::record_keeper::create(&illustrator.state_db_path)?;

	log::info!("Open book {id:?}");
	let book = records.fetch_book(id)?;

	let (req_tx, req_rx) = channel();
	let font_system = illustrator.font_system.clone();
	let settings = illustrator.settings.clone();
	let cache = illustrator.cache.clone();

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
		let mut archive = ZipArchive::new(Cursor::new(bytes.clone()))
			.inspect_err(|e| log::error!("Error: {e}"))?;
		let (book_meta, toc) = read_book_meta(bytes, &mut archive)?;
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
					let (item, path) = book_meta
						.spine_resource(current_loc)
						.expect("On location without spine");
					builder = assure_cached(
						&settings,
						&font_system,
						&cache,
						&mut archive,
						builder,
						&mut taffy_tree,
						item,
						path,
					)
					.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					log::info!("Record loc {current_loc}");
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
				Request::RefreshCache => {
					cache.write().unwrap().clear();
				}
			}
		}

		log::info!("Illustrator worker terminated");
		Ok(())
	});

	Ok(IllustratorHandle {
		req_tx,
		handle,
		toc: handle_toc,
		location: handle_loc,
	})
}

#[allow(clippy::too_many_arguments)]
fn assure_cached<R: io::Seek + io::Read>(
	settings: &RwLock<RenderSettings>,
	font_system: &Mutex<FontSystem>,
	cache: &RwLock<PageContentCache>,
	archive: &mut ZipArchive<R>,
	builder: NodeTreeBuilder,
	taffy_tree: &mut taffy::TaffyTree<html_parser::NodeId>,
	item: &BookSpineItem,
	path: &Path,
) -> Result<NodeTreeBuilder, IllustratorError> {
	if cache.read().unwrap().is_cached(item) {
		log::trace!("Already cached {item:?}");
		Ok(builder)
	} else {
		log::info!("Refresh cache for {}", path.display());
		let start = Instant::now();
		let file = archive.by_path(path)?;
		let (pages, b) = render_resource(
			&settings.read().unwrap(),
			font_system,
			file,
			builder,
			taffy_tree,
		)?;
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

fn render_resource<R: io::Seek + io::Read>(
	settings: &RenderSettings,
	font_system: &Mutex<FontSystem>,
	file: ZipFile<R>,
	builder: NodeTreeBuilder,
	tree: &mut taffy::TaffyTree<html_parser::NodeId>,
) -> Result<(Vec<PageContent>, NodeTreeBuilder), IllustratorRenderError> {
	let node_tree = builder.read_from(file)?;

	let body: NodeId = tree.new_leaf(Style {
		size: taffy::Size {
			width: length(settings.page_width_padded()),
			height: auto(),
		},
		..Default::default()
	})?;
	let mut buffers = HashMap::new();
	let mut current = body;
	let mut text_styles = Vec::new();
	let mut texts = Vec::new();
	for edge in node_tree
		.body_iter()
		.ok_or(IllustratorRenderError::MissingBody)?
	{
		match edge {
			EdgeRef::OpenElement(el) => {
				if has_text_style(el.local_name()) {
					text_styles.push(text_style(settings, text_styles.last(), el.local_name()));
				}
				if !is_non_block(el.local_name()) {
					if !texts.is_empty() {
						buffers.entry(current).or_insert_with(|| {
							create_text(
								settings,
								text_styles.last(),
								&mut font_system.lock().unwrap(),
								texts.drain(..),
							)
						});
					}
					let node = tree
						.new_leaf_with_context(element_style(settings, el.local_name()), el.id)?;
					tree.add_child(current, node)?;
					current = node;
				}
			}
			EdgeRef::CloseElement(_id, name) => {
				if has_text_style(&name) {
					text_styles.pop();
				}
				if !is_non_block(&name) {
					if !texts.is_empty() {
						buffers.entry(current).or_insert_with(|| {
							create_text(
								settings,
								text_styles.last(),
								&mut font_system.lock().unwrap(),
								texts.drain(..),
							)
						});
					}
					current = tree
						.parent(current)
						.ok_or(IllustratorRenderError::UnexpectedExtraClose)?;
				}
			}
			EdgeRef::Text(TextWrapper { t: Text { t }, .. }) => {
				let attr = text_styles
					.last()
					.map(|(_, attrs)| attrs)
					.cloned()
					.unwrap_or(settings.body_text().1);
				texts.push((t, attr));
			}
		}
	}

	debug_assert!(texts.is_empty());
	debug_assert!(text_styles.is_empty());
	drop(texts);
	drop(text_styles);

	tree.compute_layout_with_measure(
		body,
		Size::MAX_CONTENT,
		|known_dimensions, available_space, node_id, _node_context, _style| match buffers
			.get_mut(&node_id)
		{
			Some(buffer) => measure_text(
				&mut font_system.lock().unwrap(),
				buffer,
				known_dimensions,
				available_space,
			),
			None => Size::ZERO,
		},
	)?;

	let page_height = settings.page_height_padded();
	let padding_top = settings.padding_top_em * settings.em();
	let padding_left = settings.padding_left_em * settings.em();

	let mut page_end = 0.;
	let mut offset = taffy::Point::<f32>::zero();
	let mut pages = Vec::new();
	let mut page = PageContent {
		position: PagePosition::First,
		index: 0,
		elements: Range::from(0..0),
		items: Vec::new(),
	};
	for edge in TaffyTreeIter::new(tree, body) {
		match edge {
			Edge::Open(id) => {
				if let Some(id) = tree.get_node_context(id) {
					page.elements.end = id.value();
				}
				let l = tree.layout(id)?;
				offset = taffy::Point {
					x: offset.x + l.location.x,
					y: offset.y + l.location.y,
				};
				if let Some(buffer) = buffers.remove(&id) {
					let content_end = offset.y + l.size.height;
					if content_end - page_end > page_height {
						let index = page.index + 1;
						let elements_end = page.elements.end;
						pages.push(page);
						page = PageContent {
							position: PagePosition::empty(),
							index,
							elements: Range::from(elements_end..elements_end),
							items: Vec::new(),
						};
						page_end = offset.y;
					}
					page.items.push(DisplayItem::Text(DisplayText {
						pos: taffy::Point {
							x: padding_left + offset.x,
							y: padding_top + offset.y - page_end,
						},
						size: l.size,
						buffer,
					}));
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

fn measure_text(
	font_system: &mut FontSystem,
	buffer: &mut Buffer,
	known_dimensions: Size<Option<f32>>,
	available_space: Size<taffy::AvailableSpace>,
) -> Size<f32> {
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

	Size { width, height }
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

fn create_text<'a>(
	settings: &'a RenderSettings,
	base: Option<&(cosmic_text::Metrics, cosmic_text::Attrs<'static>)>,
	font_system: &mut FontSystem,
	texts: impl IntoIterator<Item = (&'a str, Attrs<'a>)>,
) -> Buffer {
	let (metrics, attrs) = base.cloned().unwrap_or_else(|| settings.body_text());
	let mut buffer = Buffer::new(font_system, metrics);
	buffer.set_rich_text(font_system, texts, &attrs, Shaping::Advanced, None);
	buffer
}

fn text_style(
	settings: &RenderSettings,
	base: Option<&(cosmic_text::Metrics, cosmic_text::Attrs<'static>)>,
	name: &LocalName,
) -> (cosmic_text::Metrics, cosmic_text::Attrs<'static>) {
	let (metrics, attrs) = base.cloned().unwrap_or_else(|| settings.body_text());
	match *name {
		local_name!("b") | local_name!("strong") => {
			(metrics, attrs.weight(cosmic_text::Weight::BOLD))
		}
		local_name!("i") | local_name!("em") => (metrics, attrs.style(cosmic_text::Style::Italic)),
		local_name!("h1") => settings.h1_text(),
		local_name!("h2") => settings.h2_text(),
		local_name!("h3") => settings.h2_text(),
		local_name!("h4") => settings.h2_text(),
		_ => settings.body_text(),
	}
}

fn has_text_style(name: &LocalName) -> bool {
	name == &local_name!("h1")
		|| name == &local_name!("h2")
		|| name == &local_name!("h3")
		|| name == &local_name!("h4")
		|| name == &local_name!("strong")
		|| name == &local_name!("b")
		|| name == &local_name!("em")
		|| name == &local_name!("i")
}

fn is_non_block(name: &LocalName) -> bool {
	name == &local_name!("strong")
		|| name == &local_name!("b")
		|| name == &local_name!("em")
		|| name == &local_name!("i")
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
