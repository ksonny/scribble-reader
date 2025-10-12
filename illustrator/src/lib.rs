#![feature(mapped_lock_guards)]
mod error;
mod html_parser;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::io::Cursor;
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
use std::sync::mpsc::channel;
use std::thread::JoinHandle;
use std::time::Instant;

use cosmic_text::Attrs;
use cosmic_text::Buffer;
use cosmic_text::FontSystem;
use cosmic_text::Shaping;
use epub::doc::EpubDoc;
use epub::doc::NavPoint;
use html5ever::local_name;
use scribe::library;
use scribe::library::Location;
use taffy::prelude::*;
use zip::ZipArchive;
use zip::read::ZipFile;
use bitflags::bitflags;

use crate::error::IllustratorError;
use crate::error::IllustratorRenderError;
use crate::error::IllustratorRequestError;
use crate::error::IllustratorSpawnError;
use crate::html_parser::EdgeRef;
use crate::html_parser::Element;
use crate::html_parser::NodeTreeBuilder;
use crate::html_parser::NodeTreeIter;
use crate::html_parser::Text;
use crate::html_parser::TextStyle;

#[derive(Debug, Default)]
pub struct PageContentCache {
	pages: Vec<PageContent>,
}

impl PageContentCache {
	pub fn page(&self, loc: Location) -> Option<&PageContent> {
		match loc {
			Location::Spine { spine, element } => self
				.pages
				.iter()
				.find(|p| p.spine_index == spine && p.elements.contains(&element)),
		}
	}

	fn next_page(&self, book_meta: &BookMeta, loc: Location) -> Result<Location, IllustratorError> {
		let page = self
			.page(loc)
			.ok_or(IllustratorError::ImpossibleMissingCache)?;
		let Location::Spine { spine, .. } = loc;
		if page.position.contains(PagePosition::Last) {
			let item = book_meta.spine.get(spine as usize + 1).ok_or_else(|| {
				IllustratorError::SpinelessBook(Location::Spine {
					spine: spine.saturating_sub(1),
					element: u64::MAX,
				})
			})?;
			Ok(Location::Spine {
				spine: item.index,
				element: item.elements.start,
			})
		} else {
			debug_assert!(
				self.pages
					.iter()
					.filter(|p| p.spine_index == spine)
					.is_sorted_by_key(|p| p.elements.start),
				"Cache not sorted"
			);
			self.pages
				.iter()
				.find(|p| p.spine_index == spine && p.elements.start > page.elements.start)
				.map(|p| Location::Spine {
					spine,
					element: p.elements.start,
				})
				.ok_or(IllustratorError::ImpossibleMissingCache)
		}
	}

	fn previous_page(
		&self,
		book_meta: &BookMeta,
		loc: Location,
	) -> Result<Location, IllustratorError> {
		let page = self
			.page(loc)
			.ok_or(IllustratorError::ImpossibleMissingCache)?;
		let Location::Spine { spine, .. } = loc;
		if page.position.contains(PagePosition::First) {
			let item = book_meta
				.spine
				.get(spine.saturating_sub(1) as usize)
				.ok_or_else(|| {
					IllustratorError::SpinelessBook(Location::Spine {
						spine: spine.saturating_sub(1),
						element: u64::MAX,
					})
				})?;
			Ok(Location::Spine {
				spine: item.index,
				element: item.elements.end.saturating_sub(1),
			})
		} else {
			debug_assert!(
				self.pages
					.iter()
					.filter(|p| p.spine_index == spine)
					.is_sorted_by_key(|p| p.elements.start),
				"Cache not sorted"
			);
			self.pages
				.iter()
				.rfind(|p| p.spine_index == spine && p.elements.start < page.elements.start)
				.map(|p| Location::Spine {
					spine,
					element: p.elements.start,
				})
				.ok_or(IllustratorError::ImpossibleMissingCache)
		}
	}

	fn is_cached(&self, spine_item: &BookSpineItem) -> bool {
		self.pages.iter().any(|p| p.spine_index == spine_item.index)
	}

	fn insert(
		&mut self,
		_spine_item: &BookSpineItem,
		pages: impl IntoIterator<Item = PageContent>,
	) {
		self.pages.extend(pages)
	}

	pub fn clear(&mut self) {
		self.pages.clear()
	}
}

pub struct IllustratorHandle {
	req_tx: Sender<Request>,
	#[allow(unused)]
	handle: JoinHandle<Result<(), IllustratorError>>,
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

pub struct Illustrator {
	state_db_path: PathBuf,
	font_system: Arc<Mutex<FontSystem>>,
	settings: Arc<RwLock<RenderSettings>>,
	cache: Arc<RwLock<PageContentCache>>,
	req_tx: Option<Sender<Request>>,
	settings_changed: bool,
}

impl Illustrator {
	pub fn new(state_path: PathBuf, settings: RenderSettings) -> Self {
		Self {
			state_db_path: state_path,
			font_system: Arc::new(Mutex::new(FontSystem::new())),
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
		log::info!("Resize event");
		let mut settings = self.settings.write().unwrap();
		settings.page_width = width;
		settings.page_height = height;
		self.settings_changed = true;
	}

	pub fn rescale(&mut self, scale: f32) {
		log::info!("Rescale event");
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

#[allow(unused)]
#[derive(Debug)]
struct BookToCItem {
	label: String,
	content: PathBuf,
	play_order: usize,
}

#[allow(unused)]
#[derive(Debug)]
struct BookSpineItem {
	index: u64,
	idref: String,
	elements: Range<u64>,
}

#[allow(unused)]
struct BookMeta {
	resources: BTreeMap<String, BookResource>,
	spine: Vec<BookSpineItem>,
	toc: Vec<BookToCItem>,
	toc_title: String,
	cover_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum FromEpubError {}

impl BookMeta {
	fn create<R: io::Seek + io::Read>(
		epub: EpubDoc<R>,
		archive: &mut ZipArchive<R>,
	) -> Result<Self, IllustratorError> {
		let start = Instant::now();
		let EpubDoc {
			resources,
			spine,
			toc,
			toc_title,
			cover_id,
			metadata,
			..
		} = epub;
		let resources = resources
			.into_iter()
			.map(|(key, (path, mime))| (key, BookResource::new(path, &mime)))
			.collect::<BTreeMap<_, _>>();
		let toc = {
			let mut items = Vec::new();
			convert_toc(&mut items, toc.into_iter());
			items
		};

		let spine = {
			let mut builder = NodeTreeBuilder::new()?;
			let mut items = Vec::new();
			let mut elements = 0;
			for (index, item) in spine.into_iter().enumerate() {
				let res = resources
					.get(&item.idref)
					.ok_or_else(|| IllustratorError::MissingResource(item.idref.clone()))?;
				let file = archive.by_path(&res.path)?;
				let tree = builder.read_from(file)?;
				if let Some(body_iter) = tree.body_nodes() {
					let start = elements;
					elements += body_iter
						.filter(|n| matches!(n, EdgeRef::OpenElement(..)))
						.count() as u64;
					items.push(BookSpineItem {
						index: index as u64,
						idref: item.idref,
						elements: start..elements,
					});
				}
				builder = tree.into_builder()?;
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
			toc_title,
			toc,
			cover_id,
		})
	}

	fn spine_resource(&self, loc: Location) -> Option<(&BookSpineItem, &Path)> {
		let Location::Spine { spine, element } = loc;
		self.spine
			.get(spine as usize)
			.filter(|s| s.elements.contains(&element))
			.and_then(|s| self.resources.get(&s.idref).map(|r| (s, r.path.as_path())))
	}
}

fn convert_toc(toc_items: &mut Vec<BookToCItem>, iter: impl Iterator<Item = NavPoint>) {
	for item in iter {
		let NavPoint {
			label,
			content,
			children,
			play_order,
		} = item;
		toc_items.push(BookToCItem {
			label,
			content,
			play_order,
		});
		convert_toc(toc_items, children.into_iter());
	}
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
	pub spine_index: u64,
	pub position: PagePosition,
	pub elements: Range<u64>,
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

	pub body_text: RenderTextSettings,
	pub h1_text: RenderTextSettings,
	pub h2_text: RenderTextSettings,
}

#[allow(unused)]
impl RenderSettings {
	fn body_text(&self) -> (cosmic_text::Metrics, &Attrs<'static>) {
		(self.body_text.metrics(self.scale), &self.body_text.attrs)
	}
	fn h1_text(&self) -> (cosmic_text::Metrics, &Attrs<'static>) {
		(self.h1_text.metrics(self.scale), &self.h1_text.attrs)
	}
	fn h2_text(&self) -> (cosmic_text::Metrics, &Attrs<'static>) {
		(self.h2_text.metrics(self.scale), &self.h2_text.attrs)
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

	let handle = std::thread::spawn(move || -> Result<(), IllustratorError> {
		log::trace!("Launching illustrator");
		let bytes = SharedVec(Arc::new(
			fs::read(&book.path).inspect_err(|e| log::error!("Error: {e}"))?,
		));
		let doc = EpubDoc::from_reader(Cursor::new(bytes.clone()))
			.inspect_err(|e| log::error!("Error: {e}"))?;
		let mut archive =
			ZipArchive::new(Cursor::new(bytes)).inspect_err(|e| log::error!("Error: {e}"))?;
		let book_meta =
			BookMeta::create(doc, &mut archive).inspect_err(|e| log::error!("Error: {e}"))?;

		log::info!(
			"Opened book with {} resources, {} spine items",
			book_meta.resources.len(),
			book_meta.spine.len()
		);
		records
			.record_book_state(id, None)
			.inspect_err(|e| log::error!("Error: {e}"))?;

		cache.write().unwrap().clear();

		let mut builder = NodeTreeBuilder::new().unwrap();
		let mut current_loc = Location::Spine {
			spine: book.spine.unwrap_or(0),
			element: book.element.unwrap_or(0),
		};
		if let Some((item, path)) = book_meta.spine_resource(current_loc) {
			builder = assure_cached(
				&settings,
				&font_system,
				&cache,
				&mut archive,
				builder,
				item,
				path,
			)
			.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
			bell.content_ready(id, current_loc);
		} else {
			log::error!("Invalid book location, reset to first page");
			current_loc = Location::Spine {
				spine: 0,
				element: 0,
			};
			let (item, path) = book_meta
				.spine_resource(current_loc)
				.ok_or(IllustratorError::SpinelessBook(current_loc))
				.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
			builder = assure_cached(
				&settings,
				&font_system,
				&cache,
				&mut archive,
				builder,
				item,
				path,
			)
			.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
			records
				.record_book_state(id, Some(current_loc))
				.inspect_err(|e| log::error!("Error: {e}"))?;
			bell.content_ready(id, current_loc);
		}

		for req in req_rx.iter() {
			match req {
				Request::NextPage => {
					log::info!("NextPage {current_loc}");
					let loc = cache
						.read()
						.unwrap()
						.next_page(&book_meta, current_loc)
						.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					log::info!("Goto next page at {loc}");
					if let Some((item, path)) = book_meta.spine_resource(loc) {
						builder = assure_cached(
							&settings,
							&font_system,
							&cache,
							&mut archive,
							builder,
							item,
							path,
						)
						.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
						current_loc = loc;
						log::info!("Record loc {current_loc}");
						records
							.record_book_state(id, Some(current_loc))
							.inspect_err(|e| log::error!("Error: {e}"))?;
						bell.content_ready(id, current_loc);
					} else {
						log::info!("At end of book: {loc}")
					}
				}
				Request::PreviousPage => {
					log::info!("PreviousPage {current_loc}");
					let loc = cache
						.read()
						.unwrap()
						.previous_page(&book_meta, current_loc)
						.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					let (item, path) = book_meta
						.spine_resource(loc)
						.ok_or(IllustratorError::SpinelessBook(loc))
						.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					builder = assure_cached(
						&settings,
						&font_system,
						&cache,
						&mut archive,
						builder,
						item,
						path,
					)
					.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					current_loc = loc;
					log::info!("Record loc {current_loc}");
					records
						.record_book_state(id, Some(current_loc))
						.inspect_err(|e| log::error!("Error: {e}"))?;
					bell.content_ready(id, current_loc);
				}
				Request::Goto(loc) => {
					log::info!("Goto {loc}");
					let (item, path) = book_meta
						.spine_resource(loc)
						.ok_or(IllustratorError::SpinelessBook(loc))
						.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					builder = assure_cached(
						&settings,
						&font_system,
						&cache,
						&mut archive,
						builder,
						item,
						path,
					)
					.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					current_loc = loc;
					records
						.record_book_state(id, Some(current_loc))
						.inspect_err(|e| log::error!("Error: {e}"))?;
					bell.content_ready(id, current_loc);
				}
				Request::RefreshCache => {
					log::info!("Refresh cache");
					cache.write().unwrap().clear();
					let (item, path) = book_meta
						.spine_resource(current_loc)
						.ok_or(IllustratorError::SpinelessBook(current_loc))
						.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					builder = assure_cached(
						&settings,
						&font_system,
						&cache,
						&mut archive,
						builder,
						item,
						path,
					)
					.inspect_err(|e| log::error!("Illustrator error: {e}"))?;
					bell.content_ready(id, current_loc);
				}
			}
		}

		log::info!("Illustrator worker terminated");
		Ok(())
	});

	Ok(IllustratorHandle { req_tx, handle })
}

fn assure_cached<R: io::Seek + io::Read>(
	settings: &RwLock<RenderSettings>,
	font_system: &Mutex<FontSystem>,
	cache: &RwLock<PageContentCache>,
	archive: &mut ZipArchive<R>,
	builder: NodeTreeBuilder,
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
			&mut font_system.lock().unwrap(),
			file,
			item,
			builder,
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

struct MeasureContext<'a> {
	font_system: &'a mut FontSystem,
	settings: &'a RenderSettings,
	buffers: HashMap<NodeId, Buffer>,
}

impl<'a> MeasureContext<'a> {
	fn new(font_system: &'a mut FontSystem, settings: &'a RenderSettings) -> Self {
		Self {
			font_system,
			settings,
			buffers: HashMap::new(),
		}
	}

	fn measure_text(
		&mut self,
		known_dimensions: Size<Option<f32>>,
		available_space: Size<taffy::AvailableSpace>,
		node_id: taffy::NodeId,
		_style: &taffy::Style,
		text: &mut Text,
	) -> Size<f32> {
		let Text { style, t } = text;
		let width_constraint = known_dimensions.width.or(match available_space.width {
			AvailableSpace::MinContent => Some(0.0),
			AvailableSpace::MaxContent => None,
			AvailableSpace::Definite(width) => Some(width),
		});

		let (metrics, attrs) = match style {
			TextStyle::Body => self.settings.body_text(),
			TextStyle::H1 => self.settings.h1_text(),
			TextStyle::H2 => self.settings.h1_text(),
		};
		let buffer = self.buffers.entry(node_id).or_insert_with(|| {
			let mut buffer = Buffer::new(self.font_system, metrics);
			buffer.set_wrap(self.font_system, cosmic_text::Wrap::WordOrGlyph);
			buffer.set_size(self.font_system, width_constraint, None);
			buffer.set_text(self.font_system, t, attrs, Shaping::Advanced);
			buffer.shape_until_scroll(self.font_system, false);
			buffer
		});

		let (width, total_lines) = buffer
			.layout_runs()
			.fold((0.0, 0usize), |(width, total_lines), run| {
				(run.line_w.max(width), total_lines + 1)
			});
		let height = total_lines as f32 * metrics.line_height;

		Size { width, height }
	}

	fn measure_element(
		&mut self,
		_known_dimensions: Size<Option<f32>>,
		_available_space: Size<taffy::AvailableSpace>,
		_node_id: taffy::NodeId,
		_style: &taffy::Style,
		_element: &mut Element,
	) -> Size<f32> {
		Size::ZERO
	}
}

fn render_resource<R: io::Seek + io::Read>(
	settings: &RenderSettings,
	font_system: &mut FontSystem,
	file: ZipFile<R>,
	item: &BookSpineItem,
	builder: NodeTreeBuilder,
) -> Result<(Vec<PageContent>, NodeTreeBuilder), IllustratorRenderError> {
	let mut node_tree = builder.read_from(file)?;
	let body = node_tree
		.nodes()
		.find_map(|n| match n {
			EdgeRef::OpenElement(el) if matches!(el.local_name(), &local_name!("body")) => {
				Some(el.id)
			}
			_ => None,
		})
		.ok_or(IllustratorRenderError::MissingBodyElement)?;
	let mut buffers = {
		let tree = &mut node_tree.tree;
		let mut measurer = MeasureContext::new(font_system, settings);
		tree.set_style(
			body,
			Style {
				size: taffy::Size {
					width: length(settings.page_width_padded()),
					height: auto(),
				},
				..Default::default()
			},
		)?;
		tree.compute_layout_with_measure(
			body,
			Size::MAX_CONTENT,
			|known_dimensions, available_space, node_id, node_context, style| match node_context {
				Some(html_parser::Node::Text(text)) => {
					measurer.measure_text(known_dimensions, available_space, node_id, style, text)
				}
				Some(html_parser::Node::Element(element)) => measurer.measure_element(
					known_dimensions,
					available_space,
					node_id,
					style,
					element,
				),
				None => Size::ZERO,
			},
		)?;
		measurer.buffers
	};

	// print_tree(&node_tree, &buffers)?;

	let page_height = settings.page_height_padded();
	let padding_top = settings.padding_top_em * settings.em();
	let padding_left = settings.padding_left_em * settings.em();

	let mut page_end = 0.;
	let mut offset = taffy::Point::<f32>::zero();
	let mut pages = Vec::new();
	let mut page = PageContent {
		spine_index: item.index,
		position: PagePosition::First,
		elements: item.elements.start..item.elements.start,
		items: Vec::new(),
	};
	for edge in NodeTreeIter::new(&node_tree.tree, body) {
		match edge {
			EdgeRef::OpenElement(el) => {
				page.elements.end += 1;
				let l = node_tree.tree.layout(el.id)?;
				offset = taffy::Point {
					x: offset.x + l.location.x,
					y: offset.y + l.location.y,
				};
			}
			EdgeRef::CloseElement(id) => {
				let l = node_tree.tree.layout(id)?;
				offset = taffy::Point {
					x: offset.x - l.location.x,
					y: offset.y - l.location.y,
				};
			}
			EdgeRef::Text(text) => {
				// TODO: Break large content into pieces
				let l = node_tree.tree.layout(text.id)?;
				let content_end = offset.y + l.location.y + l.size.height;
				if content_end - page_end > page_height {
					log::info!("Break page at {page_end} {:?}", page.elements);
					let elements_end = page.elements.end;
					pages.push(page);
					page = PageContent {
						spine_index: item.index,
						position: PagePosition::empty(),
						elements: elements_end..elements_end,
						items: Vec::new(),
					};
					page_end = offset.y + l.location.y;
				}
				let b = buffers
					.remove(&text.id)
					.ok_or(IllustratorRenderError::NoTextBuffer(text.id))?;
				page.items.push(DisplayItem::Text(DisplayText {
					pos: taffy::Point {
						x: padding_left + offset.x + l.location.x,
						y: padding_top + offset.y + l.location.y - page_end,
					},
					size: l.size,
					buffer: b,
				}))
			}
		}
	}
	if !page.items.is_empty() {
		log::info!("Last page {:?}", page.elements);
		page.position.set(PagePosition::Last, true);
		pages.push(page);
	} else if let Some(last) = pages.last_mut() {
		last.position.set(PagePosition::Last, true);
	}
	log::trace!("Generated {} pages", pages.len());
	debug_assert!(
		pages
			.last()
			.is_none_or(|p| p.elements.end == item.elements.end),
		"Element count missmatch"
	);

	Ok((pages, node_tree.into_builder()?))
}

#[allow(dead_code)]
fn print_tree(
	node_tree: &html_parser::NodeTree,
	buffers: &HashMap<NodeId, Buffer>,
) -> Result<(), IllustratorRenderError> {
	let mut indent = Vec::new();
	for edge in node_tree.nodes() {
		match edge {
			EdgeRef::OpenElement(el) => {
				let layout = node_tree.tree.layout(el.id)?;
				println!(
					"{:i$}<{} left={} top={}>",
					"",
					el.name().local,
					layout.location.x,
					layout.location.y,
					i = indent.len()
				);
				indent.push(el.name().local.clone());
			}
			EdgeRef::CloseElement(_) => {
				if let Some(name) = indent.pop() {
					println!("{:i$}</{name}>", "", i = indent.len());
				}
			}
			EdgeRef::Text(text) => {
				if let Some(buffer) = buffers.get(&text.id) {
					for run in buffer.layout_runs() {
						let start = run.glyphs.first().map(|g| g.start).unwrap_or(0);
						let end = run.glyphs.last().map(|g| g.end).unwrap_or(start);
						let t = &run
							.text
							.chars()
							.skip(start)
							.take(end - start)
							.collect::<String>();
						println!("{:i$} {}", "", t.trim(), i = indent.len());
					}
				} else {
					println!("{:i$} {}", "", text.text(), i = indent.len());
				}
			}
		}
	}
	Ok(())
}
