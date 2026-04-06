mod cache;
mod html_parser;
mod layout;
mod svg;

use std::io;
use std::io::Cursor;
use std::ops::Range;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::sync::mpsc::TryRecvError;
use std::sync::mpsc::channel;
use std::thread::JoinHandle;
use std::time::Instant;

use bitflags::bitflags;
use fixed::types::I26F6;
use fixed::types::U26F6;
use scribe::ScribeConfig;
use scribe::epub;
use scribe::epub::Package;
use scribe::library;
use scribe::library::Location;
use scribe::record_keeper::RecordKeeper;
use sculpter::SculpterFonts;
use sculpter::SculpterOptions;
use sculpter::TextBlock;
use wrangler::DocumentId;
use wrangler::content::ContentWranglerAssistant;
use zip::ZipArchive;

use crate::cache::NavigateError;
use crate::cache::PageContentCache;
use crate::layout::IllustratorLayoutError;
use crate::layout::PageLayouter;
use crate::layout::StyleSettings;
use crate::layout::into_font_options;

#[derive(Debug)]
pub enum Request {
	Goto(Location),
	NextPage,
	PreviousPage,
	Resize { width: u32, height: u32 },
	Rescale { scale: f32 },
}

pub struct IllustratorAssistant {
	req_tx: Sender<Request>,
	#[allow(unused)]
	handle: JoinHandle<Result<(), IllustratorWorkerError>>,
	working: Arc<AtomicBool>,
	navigation: Arc<Mutex<Option<Arc<epub::Navigation>>>>,
	state: Arc<Mutex<BookState>>,
	cache: Arc<Mutex<PageContentCache>>,
}

#[derive(Debug, thiserror::Error)]
pub enum IllustratorRequestError {
	#[error("Illustrator not running")]
	NotRunning,
}

impl IllustratorAssistant {
	pub fn working(&self) -> bool {
		self.working.load(Ordering::Acquire)
	}

	pub fn state(&self) -> BookState {
		self.state.lock().unwrap().clone()
	}

	pub fn navigation(&self) -> Option<Arc<epub::Navigation>> {
		self.navigation.lock().unwrap().clone()
	}

	pub fn cache<'a>(&'a self) -> MutexGuard<'a, PageContentCache> {
		self.cache.lock().unwrap()
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

	pub fn rescale(&self, scale: f32) -> Result<(), IllustratorRequestError> {
		self.req_tx
			.send(Request::Rescale { scale })
			.map_err(|_| IllustratorRequestError::NotRunning)
	}

	pub fn resize(&self, width: u32, height: u32) -> Result<(), IllustratorRequestError> {
		self.req_tx
			.send(Request::Resize { width, height })
			.map_err(|_| IllustratorRequestError::NotRunning)
	}
}

#[derive(Debug)]
pub struct Position {
	pub x: f32,
	pub y: f32,
}

impl From<taffy::Point<f32>> for Position {
	fn from(value: taffy::Point<f32>) -> Self {
		Self {
			x: value.x,
			y: value.y,
		}
	}
}

#[derive(Debug)]
pub struct Size {
	pub width: f32,
	pub height: f32,
}

impl From<taffy::Size<f32>> for Size {
	fn from(value: taffy::Size<f32>) -> Self {
		Self {
			width: value.width,
			height: value.height,
		}
	}
}

#[derive(Debug)]
pub struct DisplayPixmap {
	pub pixmap_width: u32,
	pub pixmap_height: u32,
	pub pixmap_rgba: Vec<u8>,
}

#[derive(Debug)]
pub enum DisplayContent {
	Text(TextBlock),
	Pixmap(DisplayPixmap),
}

impl From<TextBlock> for DisplayContent {
	fn from(value: TextBlock) -> Self {
		DisplayContent::Text(value)
	}
}

impl From<DisplayPixmap> for DisplayContent {
	fn from(value: DisplayPixmap) -> Self {
		DisplayContent::Pixmap(value)
	}
}

#[derive(Debug)]
pub struct DisplayItem {
	pub pos: Position,
	pub size: Size,
	pub content: DisplayContent,
}

bitflags! {
	#[derive(Debug)]
	pub struct PageFlags: u8 {
		const First = 1;
		const Last  = 1 << 1;
	}
}

#[derive(Debug)]
pub struct PageContent {
	pub flags: PageFlags,
	pub elements: Range<U26F6>,
	pub items: Vec<DisplayItem>,
}

#[derive(Debug, Clone)]
pub struct Params {
	page_width: u32,
	page_height: u32,
	scale: f32,
}

pub trait Bell {
	fn content_ready(&self, id: library::BookId, loc: Location);
}

#[derive(Clone)]
struct SharedVec(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedVec {
	fn as_ref(&self) -> &[u8] {
		let Self(data) = self;
		data
	}
}

#[derive(Debug, Clone)]
pub struct BookState {
	pub location: Location,
	pub percent_read: u32,
}

struct Worker {
	config: ScribeConfig,
	fonts: Arc<SculpterFonts>,
	cache: Arc<Mutex<PageContentCache>>,
	navigation: Arc<Mutex<Option<Arc<epub::Navigation>>>>,
	state: Arc<Mutex<BookState>>,
	working: Arc<AtomicBool>,
	record_keeper: RecordKeeper,
	content: ContentWranglerAssistant,
}

#[derive(Debug, thiserror::Error)]
pub enum IllustratorWorkerError {
	#[error("record keeper error: {0}")]
	RecordKeeper(#[from] scribe::record_keeper::RecordKeeperError),
	#[error("zip error: {0}")]
	Zip(#[from] zip::result::ZipError),
	#[error("io error at {1}: {0}")]
	Io(std::io::Error, &'static std::panic::Location<'static>),
	#[error("config error: {0}")]
	Config(#[from] config::ConfigError),
	#[error("layout error: {0}")]
	Layout(#[from] IllustratorLayoutError),
	#[error("create sculpter error: {0}")]
	SculpterCreate(#[from] sculpter::SculpterCreateError),
	#[error("sculpter print error: {0}")]
	SculpterPrinter(#[from] sculpter::SculpterPrinterError),
	#[error("epub error: {0}")]
	Epub(#[from] epub::EpubError),
}

impl From<std::io::Error> for IllustratorWorkerError {
	#[track_caller]
	fn from(err: std::io::Error) -> Self {
		Self::Io(err, std::panic::Location::caller())
	}
}

impl Worker {
	fn launch(
		self,
		bell: impl Bell + Send + 'static,
		req_rx: Receiver<Request>,
		book: library::Book,
	) -> Result<(), IllustratorWorkerError> {
		let mut params = Params {
			page_width: 800,
			page_height: 600,
			scale: 1.0,
		};

		let start = Instant::now();
		let document = DocumentId::new(book.path.to_string_lossy().to_string());
		let (bytes, _) = pollster::block_on(self.content.load(document))?;
		let bytes = SharedVec(Arc::new(bytes));
		log::debug!(
			"Loaded content in {}",
			Instant::now().duration_since(start).as_secs_f64()
		);

		let start = Instant::now();
		let mut archive = ZipArchive::new(Cursor::new(bytes.clone()))?;
		let package = {
			let mut epub = epub::EpubMetadata::new(&mut archive);
			let package = epub.package()?;
			let navigation = Arc::new(epub.navigation()?);
			*self.navigation.lock().unwrap() = Some(navigation);
			package
		};
		log::debug!(
			"Loaded epub metadata in {}",
			Instant::now().duration_since(start).as_secs_f64()
		);

		let start = Instant::now();
		let mut spine_bytes = Vec::new();
		for resource in package.spine.iter().map(|id| package.manifest.get(id)) {
			if let Some(resource) = resource {
				let file = archive.by_path(resource.as_path())?;
				spine_bytes.push(file.size());
			} else {
				spine_bytes.push(0);
			}
		}
		log::debug!(
			"Loaded spine byte sizes in {}",
			Instant::now().duration_since(start).as_secs_f64()
		);

		let book_loc = book.location();
		let mut current_loc = if package.spine.get(book_loc.spine as usize).is_some() {
			book_loc
		} else {
			log::warn!("Invalid book location {book_loc:?}, reset to first page");
			Location {
				spine: 0,
				element: U26F6::ZERO,
			}
		};

		let start = Instant::now();
		let settings = self.config.illustrator()?;
		let sculpter = sculpter::create_sculpter(
			&self.fonts,
			&[
				&into_font_options(&settings.font_regular),
				&into_font_options(&settings.font_bold),
				&into_font_options(&settings.font_italic),
			],
			SculpterOptions {
				atlas_sub_pixel_mask: I26F6::from_bits(!0b1),
			},
		)?;
		log::debug!(
			"Created sculpter in {}",
			Instant::now().duration_since(start).as_secs_f64()
		);

		let mut reusable_layouter = PageLayouter::new(sculpter);
		let mut clear_cache = true;

		let records = self.record_keeper.assistant()?;

		loop {
			let req = match req_rx.try_recv() {
				Ok(req) => req,
				Err(TryRecvError::Empty) => {
					if clear_cache || !self.cache.lock().unwrap().is_cached(current_loc) {
						let start = Instant::now();

						if clear_cache {
							self.cache.lock().unwrap().clear();
							clear_cache = false;
						}

						let settings = StyleSettings::new(&settings, &params);
						reusable_layouter = self.load_chapter_to_cache(
							reusable_layouter,
							&mut archive,
							&package,
							&settings,
							current_loc.spine,
						)?;

						log::debug!(
							"Render current chapter {} in {}",
							current_loc.spine,
							Instant::now().duration_since(start).as_secs_f64()
						);
					}

					let percent_read = self.estimate_percent_read(&spine_bytes, current_loc);

					*self.state.lock().unwrap() = BookState {
						location: current_loc,
						percent_read,
					};
					bell.content_ready(book.id, current_loc);
					records.record_book_state(book.id, current_loc, percent_read)?;
					self.working.store(false, Ordering::Release);

					let start = Instant::now();
					let (load_next, load_prev) = {
						let cache = self.cache.lock().unwrap();
						let load_next = current_loc.spine as usize + 1 < package.spine.len()
							&& matches!(
								cache.next_page(current_loc),
								Err(NavigateError::LoadNextChapter)
							);
						let load_prev = matches!(
							cache.previous_page(current_loc),
							Err(NavigateError::LoadPreviousChapter)
						);
						(load_next, load_prev)
					};
					if load_next {
						let next_spine = current_loc.spine + 1;
						log::debug!("Load chapter {next_spine} into cache");
						let settings = StyleSettings::new(&settings, &params);
						reusable_layouter = self.load_chapter_to_cache(
							reusable_layouter,
							&mut archive,
							&package,
							&settings,
							next_spine,
						)?;
					}
					if load_prev {
						let prev_spine = current_loc.spine.saturating_sub(1);
						log::debug!("Load chapter {prev_spine} into cache");
						let settings = StyleSettings::new(&settings, &params);
						reusable_layouter = self.load_chapter_to_cache(
							reusable_layouter,
							&mut archive,
							&package,
							&settings,
							prev_spine,
						)?;
					}
					if load_prev || load_next {
						log::debug!(
							"Completed pre-render in {}",
							Instant::now().duration_since(start).as_secs_f64()
						);
					}

					match req_rx.recv() {
						Ok(req) => {
							self.working.store(true, Ordering::Release);
							req
						}
						Err(_) => break,
					}
				}
				Err(TryRecvError::Disconnected) => {
					break;
				}
			};

			log::trace!("{req:?} {current_loc}");

			match req {
				Request::Resize { width, height } => {
					clear_cache = true;
					params.page_width = width;
					params.page_height = height;
				}
				Request::Rescale { scale } => {
					clear_cache = true;
					params.scale = scale;
				}
				Request::NextPage => {
					// Assume next chapter is loaded into cache if needed
					current_loc = self
						.cache
						.lock()
						.unwrap()
						.next_page(current_loc)
						.unwrap_or(current_loc);
				}
				Request::PreviousPage => {
					// Assume previous chapter is loaded into cache if needed
					current_loc = self
						.cache
						.lock()
						.unwrap()
						.previous_page(current_loc)
						.unwrap_or(current_loc);
				}
				Request::Goto(loc) => {
					current_loc = loc;
				}
			}
		}

		Ok(())
	}

	fn estimate_percent_read(&self, spine_bytes: &[u64], current_loc: Location) -> u32 {
		let mut bytes_total = 0;
		let mut bytes_read = 0;
		for (idx, b) in spine_bytes.iter().enumerate() {
			if idx < current_loc.spine as usize {
				bytes_read += b;
			} else if idx == current_loc.spine as usize
				&& let Some((_, meta)) = self.cache.lock().unwrap().page(current_loc)
			{
				bytes_read += (*b * meta.page) / meta.pages;
			}
			bytes_total += b;
		}
		(100 * bytes_read / bytes_total) as u32
	}

	fn load_chapter_to_cache<'layout, 'settings, R: io::Seek + io::Read + Send + Sync>(
		&self,
		layouter: PageLayouter<'layout>,
		archive: &mut ZipArchive<R>,
		package: &Package,
		settings: &StyleSettings<'settings>,
		spine_index: u32,
	) -> Result<PageLayouter<'layout>, IllustratorWorkerError> {
		let resource = package
			.metadata_by_spine(spine_index as usize)
			.expect("Unexpected missing resource");
		let layouter = layouter.load(
			archive,
			package.package_root.as_path(),
			resource.as_path(),
			settings,
		)?;
		let (mut layouter, pages) = layouter.layout(settings)?;

		let mut cache = self.cache.lock().unwrap();
		cache.insert(spine_index, pages);
		layouter.write_glyph_atlas(cache.atlas_mut())?;
		drop(cache);

		Ok(layouter)
	}
}

#[derive(Debug, thiserror::Error)]
pub enum IllustratorCreateError {
	#[error(transparent)]
	RecordKeeper(#[from] scribe::record_keeper::RecordKeeperError),
}

#[must_use = "Must track handle or illustrator dies"]
pub fn create_illustrator(
	config: ScribeConfig,
	record_keeper: RecordKeeper,
	content: ContentWranglerAssistant,
	fonts: Arc<SculpterFonts>,
	bell: impl Bell + Send + 'static,
	book_id: library::BookId,
) -> Result<IllustratorAssistant, IllustratorCreateError> {
	log::debug!("Open book {book_id}");

	let records = record_keeper.assistant()?;
	let book = records.fetch_book(book_id)?;

	let config = config.clone();
	let fonts = fonts.clone();

	let cache = Arc::new(Mutex::new(PageContentCache::default()));
	let navigation = Arc::new(Mutex::new(None));
	let state = Arc::new(Mutex::new(BookState {
		location: book.location(),
		percent_read: book.percent_read.unwrap_or(0),
	}));
	let working = Arc::new(AtomicBool::new(true));

	let (req_tx, req_rx) = channel();

	let worker = Worker {
		config,
		fonts,
		cache: cache.clone(),
		navigation: navigation.clone(),
		state: state.clone(),
		working: working.clone(),
		record_keeper,
		content,
	};

	let handle = std::thread::spawn(move || -> Result<(), IllustratorWorkerError> {
		log::trace!("Launching illustrator");
		match worker.launch(bell, req_rx, book) {
			Ok(()) => {
				log::info!("Illustrator worker terminated");
				Ok(())
			}
			Err(err) => {
				log::error!("Error in illustrator: {err}");
				Err(err)
			}
		}
	});

	Ok(IllustratorAssistant {
		req_tx,
		handle,
		working,
		navigation,
		state,
		cache,
	})
}
