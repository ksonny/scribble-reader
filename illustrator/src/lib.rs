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
use scribe::library;
use scribe::library::Location;
use taffy::prelude::*;
use zip::ZipArchive;
use zip::read::ZipFile;

use crate::error::IllustratorError;
use crate::error::IllustratorRenderError;
use crate::error::IllustratorRequestError;
use crate::error::IllustratorSpawnError;
use crate::html_parser::EdgeRef;
use crate::html_parser::Element;
use crate::html_parser::NodeTreeBuilder;
use crate::html_parser::Text;

#[derive(Debug, Default)]
pub struct PageContentCache {
	pages: Vec<PageContent>,
}

impl PageContentCache {
	pub fn page(&self, loc: Location) -> Option<&PageContent> {
		match loc {
			Location::Word(word) => self.pages.iter().find(|p| p.words.contains(&word)),
		}
	}

	pub fn extend(&mut self, pages: impl IntoIterator<Item = PageContent>) {
		self.pages.extend(pages)
	}

	pub fn clear(&mut self) {
		self.pages.clear()
	}

	pub fn drain(&mut self, words: Range<u64>) {
		self.pages.retain(|p| !words.contains(&p.words.start));
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
		let mut settings = self.settings.write().unwrap();
		settings.page_width = width;
		settings.page_height = height;
		self.settings_changed = true;
	}

	pub fn rescale(&mut self, scale: f32) {
		let mut settings = self.settings.write().unwrap();
		settings.scale = scale;
		self.settings_changed = true;
	}

	pub fn refresh_if_needed(&mut self) {
		if self.settings_changed
			&& let Some(req_tx) = &self.req_tx
		{
			match req_tx.send(Request::RefreshCache) {
				Ok(_) => {}
				Err(e) => {
					log::info!("Error on illustrator channel, close: {e}");
					self.req_tx = None;
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

#[derive(Debug)]
struct BookSpineItem {
	idref: String,
	words: Range<u64>,
}

#[allow(unused)]
struct BookMeta {
	resources: BTreeMap<String, BookResource>,
	spine: Vec<BookSpineItem>,
	toc: Vec<BookToCItem>,
	toc_title: String,
	cover_id: Option<String>,
	total_words: u64,
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
			let mut words = 0;
			for item in spine {
				let res = resources
					.get(&item.idref)
					.ok_or_else(|| IllustratorError::MissingResource(item.idref.clone()))?;
				let tree = builder.read_from(archive.by_path(&res.path)?)?;
				let start = words;
				words += tree
					.nodes()
					.map(|n| {
						if let EdgeRef::Text(text) = n {
							count_words(text.text())
						} else {
							0
						}
					})
					.sum::<u64>();
				builder = tree.into_builder()?;

				items.push(BookSpineItem {
					idref: item.idref,
					words: start..words,
				});
			}
			items
		};
		let total_words = spine.last().map(|s| s.words.end).unwrap_or(0);
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
			total_words,
		})
	}

	fn spine_resource(&self, loc: Location) -> Option<(&BookSpineItem, &Path)> {
		let Location::Word(word) = loc;
		self.spine
			.iter()
			.find(|s| s.words.contains(&word))
			.and_then(|s| self.resources.get(&s.idref).map(|r| (s, r.path.as_path())))
	}

	fn to_word(&self, loc: Location) -> u64 {
		let Location::Word(word) = loc;
		word
	}
}

fn count_words(input: &str) -> u64 {
	let mut words = 0;
	let mut in_word = false;
	for b in input.as_bytes() {
		if matches!(*b, b'\t' | b'\n' | b' ') {
			if in_word {
				in_word = false;
				words += 1;
			}
		} else if !in_word {
			in_word = true;
		}
	}
	words
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

#[derive(Debug)]
pub struct PageContent {
	pub words: Range<u64>,
	pub items: Vec<DisplayItem>,
}

pub struct RenderTextSettings {
	pub font_size: f32,
	pub line_height: f32,
	pub attrs: cosmic_text::Attrs<'static>,
}

pub struct RenderSettings {
	pub version: u32,

	pub page_width: u32,
	pub page_height: u32,
	pub scale: f32,

	pub padding_top_em: u32,
	pub padding_left_em: u32,
	pub padding_right_em: u32,
	pub padding_bottom_em: u32,

	pub body_text: RenderTextSettings,
}

#[allow(unused)]
impl RenderSettings {
	fn body_text(&self) -> (cosmic_text::Metrics, &Attrs<'static>) {
		(
			cosmic_text::Metrics::new(
				self.body_text.font_size * self.scale,
				self.body_text.line_height * self.scale,
			),
			&self.body_text.attrs,
		)
	}

	fn page_height_padded(&self) -> f32 {
		self.page_height as f32 - self.padding_top() - self.padding_bottom()
	}

	fn page_width_padded(&self) -> f32 {
		self.page_width as f32 - self.padding_left() - self.padding_right()
	}

	fn padding_top(&self) -> f32 {
		self.padding_top_em as f32 * self.body_text.font_size * self.scale
	}

	fn padding_left(&self) -> f32 {
		self.padding_left_em as f32 * self.body_text.font_size * self.scale
	}
	fn padding_right(&self) -> f32 {
		self.padding_right_em as f32 * self.body_text.font_size * self.scale
	}
	fn padding_bottom(&self) -> f32 {
		self.padding_bottom_em as f32 * self.body_text.font_size * self.scale
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
	let word_position = book.words_position.unwrap_or(0);

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
			"Opened book with {} words, {} resources, {} spine items",
			book_meta.total_words,
			book_meta.resources.len(),
			book_meta.spine.len()
		);
		records
			.record_book_state(id, book_meta.total_words, None)
			.inspect_err(|e| log::error!("Error: {e}"))?;

		cache.write().unwrap().clear();

		let mut builder = NodeTreeBuilder::new().unwrap();
		let mut current_loc = Location::Word(word_position);
		match assure_cached(
			&settings,
			&font_system,
			&cache,
			&book_meta,
			&mut archive,
			builder,
			current_loc,
		) {
			Ok(b) => builder = b,
			Err(e) => {
				log::error!("Illustrator error: {e}");
				builder = NodeTreeBuilder::new().unwrap();
			}
		};
		bell.content_ready(id, current_loc);

		for req in req_rx.iter() {
			match req {
				Request::NextPage => {
					log::info!("NextPage {current_loc}");
					let loc = next_page_loc(&cache, &book_meta, current_loc);
					if let Some(loc) = loc {
						current_loc = loc;
						match assure_cached(
							&settings,
							&font_system,
							&cache,
							&book_meta,
							&mut archive,
							builder,
							current_loc,
						) {
							Ok(b) => builder = b,
							Err(e) => {
								log::error!("Illustrator error: {e}");
								builder = NodeTreeBuilder::new().unwrap();
								continue;
							}
						};
						log::info!("Record loc {current_loc}");
						records
							.record_book_state(
								id,
								book_meta.total_words,
								Some(book_meta.to_word(current_loc)),
							)
							.inspect_err(|e| log::error!("Error: {e}"))?;
						bell.content_ready(id, current_loc);
					} else {
						log::info!("At end {current_loc}");
					}
				}
				Request::PreviousPage => {
					log::info!("PreviousPage {current_loc}");
					current_loc = previous_page_loc(&cache, &book_meta, current_loc);
					match assure_cached(
						&settings,
						&font_system,
						&cache,
						&book_meta,
						&mut archive,
						builder,
						current_loc,
					) {
						Ok(b) => builder = b,
						Err(e) => {
							log::error!("Illustrator error: {e}");
							builder = NodeTreeBuilder::new().unwrap();
							continue;
						}
					};
					log::info!("Record loc {current_loc}");
					records
						.record_book_state(
							id,
							book_meta.total_words,
							Some(book_meta.to_word(current_loc)),
						)
						.inspect_err(|e| log::error!("Error: {e}"))?;
					bell.content_ready(id, current_loc);
				}
				Request::Goto(loc) => {
					log::info!("Goto {loc}");
					current_loc = loc;
					match assure_cached(
						&settings,
						&font_system,
						&cache,
						&book_meta,
						&mut archive,
						builder,
						current_loc,
					) {
						Ok(b) => builder = b,
						Err(e) => {
							log::error!("Illustrator error: {e}");
							builder = NodeTreeBuilder::new().unwrap();
							continue;
						}
					};
					records
						.record_book_state(
							id,
							book_meta.total_words,
							Some(book_meta.to_word(current_loc)),
						)
						.inspect_err(|e| log::error!("Error: {e}"))?;
					bell.content_ready(id, current_loc);
				}
				Request::RefreshCache => {
					log::info!("Refresh cache");
					cache.write().unwrap().clear();
					match assure_cached(
						&settings,
						&font_system,
						&cache,
						&book_meta,
						&mut archive,
						builder,
						current_loc,
					) {
						Ok(b) => builder = b,
						Err(e) => {
							log::error!("Illustrator error: {e}");
							builder = NodeTreeBuilder::new().unwrap();
						}
					};
					bell.content_ready(id, current_loc);
				}
			}
		}

		log::info!("Illustrator worker terminated");
		Ok(())
	});

	Ok(IllustratorHandle { req_tx, handle })
}

fn next_page_loc(
	cache: &RwLock<PageContentCache>,
	book_meta: &BookMeta,
	loc: Location,
) -> Option<Location> {
	let word = book_meta.to_word(loc);
	let next_start = cache.read().unwrap().page(loc).map(|p| p.words.end);
	if let Some(end) = next_start {
		Some(Location::Word(end))
	} else {
		book_meta
			.spine
			.iter()
			.find(|s| s.words.start > word)
			.map(|s| Location::Word(s.words.start))
	}
}

fn previous_page_loc(
	cache: &RwLock<PageContentCache>,
	book_meta: &BookMeta,
	loc: Location,
) -> Location {
	let word = book_meta.to_word(loc);
	let prev_start = {
		let cache = cache.read().unwrap();
		if let Some(start) = cache.page(loc).map(|p| p.words.start) {
			cache
				.page(Location::Word(start.saturating_sub(1)))
				.map(|p| p.words.start)
		} else {
			None
		}
	};
	if let Some(prev_start) = prev_start {
		Location::Word(prev_start)
	} else {
		book_meta
			.spine
			.iter()
			.rfind(|s| s.words.end < word)
			.map(|s| Location::Word(s.words.start.saturating_sub(1)))
			.unwrap_or(Location::Word(0))
	}
}

fn assure_cached<R: io::Seek + io::Read>(
	settings: &RwLock<RenderSettings>,
	font_system: &Mutex<FontSystem>,
	cache: &RwLock<PageContentCache>,
	book_meta: &BookMeta,
	archive: &mut ZipArchive<R>,
	builder: NodeTreeBuilder,
	loc: Location,
) -> Result<NodeTreeBuilder, IllustratorError> {
	if cache.read().unwrap().page(loc).is_some() {
		log::trace!("Already cached {loc}");
		Ok(builder)
	} else if let Some((item, path)) = book_meta.spine_resource(loc) {
		log::info!("Refresh cache for {}", path.display());
		let start = Instant::now();
		let file = archive.by_path(path)?;
		let (pages, b) = render_resource(
			&settings.read().unwrap(),
			&mut font_system.lock().unwrap(),
			file,
			builder,
			item.words.start,
		)?;
		let mut cache = cache.write().unwrap();
		cache.drain(item.words.clone());
		cache.extend(pages);
		drop(cache);
		let dur = Instant::now().duration_since(start);
		log::info!(
			"Render {} complete in {}",
			path.display(),
			dur.as_secs_f64()
		);
		Ok(b)
	} else {
		log::error!("No spine item associated with location {loc}");
		Ok(builder)
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
		let Text { t } = text;
		let width_constraint = known_dimensions.width.or(match available_space.width {
			AvailableSpace::MinContent => Some(0.0),
			AvailableSpace::MaxContent => None,
			AvailableSpace::Definite(width) => Some(width),
		});

		let (metrics, attrs) = self.settings.body_text();
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
	builder: NodeTreeBuilder,
	word_offset: u64,
) -> Result<(Vec<PageContent>, NodeTreeBuilder), IllustratorRenderError> {
	let mut node_tree = builder.read_from(file)?;
	let mut buffers = {
		let root = node_tree.root;
		let tree = &mut node_tree.tree;
		let mut measurer = MeasureContext::new(font_system, settings);
		tree.set_style(
			root,
			Style {
				size: taffy::Size {
					width: length(settings.page_width_padded()),
					height: auto(),
				},
				..Default::default()
			},
		)?;
		tree.compute_layout_with_measure(
			root,
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
	let padding_top = settings.padding_top();
	let padding_left = settings.padding_left();

	let mut page_end = 0.;
	let mut offset = taffy::Point::<f32>::zero();
	let mut pages = Vec::new();
	let mut page = PageContent {
		words: word_offset..word_offset,
		items: Vec::new(),
	};
	for edge in node_tree.nodes() {
		match edge {
			EdgeRef::OpenElement(el) => {
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
					log::info!("Break page at {page_end}, {:?}", page.words);
					let words_end = page.words.end;
					pages.push(page);
					page = PageContent {
						words: words_end..words_end,
						items: Vec::new(),
					};
					page_end = offset.y + l.location.y;
				}
				let b = buffers
					.remove(&text.id)
					.ok_or(IllustratorRenderError::NoTextBuffer(text.id))?;
				page.words.end += count_words(text.text());
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
		pages.push(page);
	}
	log::info!("Generated {} pages", pages.len());

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
