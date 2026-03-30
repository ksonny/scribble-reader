mod cache;
mod html_parser;
mod layout;
mod svg;

use std::fs;
use std::io;
use std::io::Cursor;
use std::ops::Range;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::RwLock;
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
use scribe::library;
use scribe::library::Location;
use sculpter::SculpterFonts;
use sculpter::SculpterOptions;
use sculpter::TextBlock;
use zip::ZipArchive;

use crate::cache::PageContentCache;
use crate::html_parser::NodeTreeBuilder;
use crate::html_parser::TreeBuilderError;
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

pub struct IllustratorHandle {
	req_tx: Sender<Request>,
	#[allow(unused)]
	handle: JoinHandle<Result<(), IllustratorWorkerError>>,
	navigation: Arc<RwLock<Arc<epub::Navigation>>>,
	location: Arc<RwLock<Location>>,
	cache: Arc<Mutex<PageContentCache>>,
}

#[derive(Debug, thiserror::Error)]
pub enum IllustratorRequestError {
	#[error("Illustrator not running")]
	NotRunning,
}

impl IllustratorHandle {
	pub fn location(&self) -> Location {
		*self.location.read().unwrap()
	}

	pub fn navigation(&self) -> Arc<epub::Navigation> {
		self.navigation.read().unwrap().clone()
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

struct Worker {
	config: ScribeConfig,
	fonts: Arc<SculpterFonts>,
	cache: Arc<Mutex<PageContentCache>>,
	shared_navigation: Arc<RwLock<Arc<epub::Navigation>>>,
	shared_location: Arc<RwLock<Location>>,
	records: scribe::record_keeper::RecordKeeper,
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
	#[error(transparent)]
	TreeBuilder(#[from] TreeBuilderError),
	#[error("Missing resource {0}")]
	MissingResource(epub::ResourceId),
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

		let bytes = SharedVec(Arc::new(fs::read(&book.path)?));

		let mut archive = ZipArchive::new(Cursor::new(bytes.clone()))?;
		let (package, navigation) = {
			let mut epub = epub::EpubMetadata::new(&mut archive);
			let package = epub.package()?;
			let navigation = epub.navigation()?;
			(package, navigation)
		};
		let spine = count_spine_elements(package.clone(), &mut archive)?;
		*self.shared_navigation.write().unwrap() = navigation.into();

		self.cache.lock().unwrap().clear();
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

		let illustrator_config = self.config.illustrator()?;
		let sculpter = sculpter::create_sculpter(
			&self.fonts,
			&[
				&into_font_options(&illustrator_config.font_regular),
				&into_font_options(&illustrator_config.font_bold),
				&into_font_options(&illustrator_config.font_italic),
			],
			SculpterOptions {
				atlas_sub_pixel_mask: I26F6::from_bits(!0b1),
			},
		)?;
		let mut layouter = PageLayouter::new(sculpter);
		let mut clear_cache = false;

		loop {
			let req = match req_rx.try_recv() {
				Ok(req) => req,
				Err(TryRecvError::Empty) => {
					let item = &spine[current_loc.spine as usize];
					let resource = package
						.manifest
						.get(&item.idref)
						.expect("Unexpected missing resource");

					if clear_cache || !self.cache.lock().unwrap().is_cached(item) {
						let settings = StyleSettings::new(&illustrator_config, &params);
						let l = layouter.load(
							&mut archive,
							package.package_root.as_path(),
							resource.as_path(),
							&settings,
						)?;
						let (mut l, pages) = l.layout(&settings)?;
						{
							let mut cache = self.cache.lock().unwrap();

							if clear_cache {
								cache.clear();
								clear_cache = false;
							}

							cache.insert(item, pages);
							l.write_glyph_atlas(cache.atlas_mut())?;
						}
						layouter = l;
					}

					log::debug!("Save location {current_loc} in {}", book.id);
					*self.shared_location.write().unwrap() = current_loc;
					self.records.record_book_state(book.id, Some(current_loc))?;
					bell.content_ready(book.id, current_loc);

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
					current_loc = self.cache.lock().unwrap().next_page(&spine, current_loc);
				}
				Request::PreviousPage => {
					current_loc = self
						.cache
						.lock()
						.unwrap()
						.previous_page(&spine, current_loc);
				}
				Request::Goto(loc) => {
					current_loc = loc;
				}
				Request::Resize { width, height } => {
					clear_cache = true;
					params.page_width = width;
					params.page_height = height;
				}
				Request::Rescale { scale } => {
					clear_cache = true;
					params.scale = scale;
				}
			}
		}

		Ok(())
	}
}

#[derive(Debug, thiserror::Error)]
pub enum IllustratorSpawnError {
	#[error(transparent)]
	RecordKeeper(#[from] scribe::record_keeper::RecordKeeperError),
}

#[must_use = "Must track handle or illustrator dies"]
pub fn create_illustrator(
	config: ScribeConfig,
	fonts: Arc<SculpterFonts>,
	bell: impl Bell + Send + 'static,
	book_id: library::BookId,
) -> Result<IllustratorHandle, IllustratorSpawnError> {
	let state_path = config.paths().data_path.join("state.db");
	let records = scribe::record_keeper::create(&state_path)?;

	log::info!("Open book {book_id}");
	let book = records.fetch_book(book_id)?;

	let config = config.clone();
	let fonts = fonts.clone();

	let cache = Arc::new(Mutex::new(PageContentCache::default()));
	let navigation = Arc::new(RwLock::new(Arc::new(epub::Navigation::default())));
	let location = Arc::new(RwLock::new(book.location()));

	let (req_tx, req_rx) = channel();

	let worker = Worker {
		config,
		fonts,
		records,
		cache: cache.clone(),
		shared_navigation: navigation.clone(),
		shared_location: location.clone(),
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

	Ok(IllustratorHandle {
		req_tx,
		handle,
		navigation,
		location,
		cache,
	})
}

#[derive(Debug)]
struct BookSpineItem {
	pub(crate) index: u32,
	pub(crate) idref: epub::ResourceId,
	pub(crate) elements: Range<U26F6>,
}

fn count_spine_elements<R: io::Seek + io::Read>(
	package: Arc<epub::Package>,
	archive: &mut ZipArchive<R>,
) -> Result<Vec<BookSpineItem>, IllustratorWorkerError> {
	let start = Instant::now();

	let spine = {
		let mut builder = NodeTreeBuilder::new();
		let mut items = Vec::new();
		for (index, item) in package.spine.iter().enumerate() {
			let resource = package
				.manifest
				.get(item)
				.ok_or_else(|| IllustratorWorkerError::MissingResource(item.clone()))?;
			let file = archive.by_path(resource.as_path())?;
			let tree = builder.read_from(file)?;
			let node_count = tree.tree.node_count();
			items.push(BookSpineItem {
				index: index as u32,
				idref: item.clone(),
				elements: U26F6::ZERO..U26F6::from_num(node_count),
			});
			builder = tree.into_builder();
		}
		items
	};
	let dur = Instant::now().duration_since(start);

	log::info!(
		"Opened {:?} in {}",
		package.metadata.title.as_ref(),
		dur.as_secs_f64()
	);

	Ok(spine)
}
