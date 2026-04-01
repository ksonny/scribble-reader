pub mod epub;
pub mod library;
pub mod record_keeper;
pub mod settings;

use std::collections::BTreeMap;
use std::collections::BinaryHeap;
use std::fs;
use std::io;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::DateTime;
use chrono::SubsecRound;
use chrono::Utc;
use image::ImageReader;
use image::codecs::png;
use image::codecs::png::PngEncoder;
use wrangler::Discovery;
use wrangler::DocumentId;
use wrangler::FileContent;
use wrangler::Ticket;
use wrangler::Wrangler;
use wrangler::WranglerCommand;
use wrangler::WranglerResult;
use wrangler::WranglerSystem;
use zip::ZipArchive;

use crate::epub::EPUB_CONTAINER_PATH;
use crate::epub::parse_container;
use crate::epub::parse_package;
use crate::library::Book;
use crate::library::BookId;
use crate::record_keeper::InsertBook;
use crate::record_keeper::RecordKeeperAssistant;
use crate::record_keeper::RecordKeeperError;
use crate::record_keeper::UpdateBook;
use crate::settings::DEFAULT_SCRIBE_CONFIG;
use crate::settings::Paths;
use crate::settings::Settings;

#[derive(Debug, thiserror::Error)]
pub enum ConfigEditError {
	#[error("at {1}: {0}")]
	Io(io::Error, &'static std::panic::Location<'static>),
	#[error(transparent)]
	TomlDe(#[from] toml::de::Error),
	#[error(transparent)]
	TomlSer(#[from] toml::ser::Error),
}

impl From<io::Error> for ConfigEditError {
	#[track_caller]
	fn from(err: std::io::Error) -> Self {
		Self::Io(err, std::panic::Location::caller())
	}
}

#[derive(Clone)]
pub struct ScribeConfig {
	paths: Arc<settings::Paths>,
	config_builder: Arc<config::ConfigBuilder<config::builder::DefaultState>>,
}

impl ScribeConfig {
	pub fn new(paths: Arc<settings::Paths>) -> Self {
		let config_path = paths.config_path.join("config.toml");
		let config_builder = config::Config::builder()
			.add_source(config::File::from_str(
				settings::DEFAULT_SCRIBE_CONFIG,
				config::FileFormat::Toml,
			))
			.add_source(config::File::from(config_path.as_path()).required(false))
			.add_source(config::Environment::with_prefix("SCRAPE").separator("_"));
		let config_builder = Arc::new(config_builder);

		Self {
			paths,
			config_builder,
		}
	}

	pub fn paths(&self) -> &settings::Paths {
		&self.paths
	}

	pub fn set_library(self, library: settings::Library) -> Result<ScribeConfig, ConfigEditError> {
		let config_path = self.paths.config_path.join("config.toml");
		let mut settings: Settings = if config_path.try_exists()? {
			toml::from_slice(&fs::read(&config_path)?)?
		} else {
			toml::from_slice(DEFAULT_SCRIBE_CONFIG.as_bytes())?
		};

		settings.library = library;

		log::info!("Write config: {}", config_path.display());
		fs::write(&config_path, &toml::to_string(&settings)?)?;

		Ok(Self::new(self.paths))
	}

	pub fn library(&self) -> Result<settings::Library, config::ConfigError> {
		let config = self.config_builder.build_cloned()?;
		config.get("library")
	}

	pub fn illustrator(&self) -> Result<settings::Illustrator, config::ConfigError> {
		let config = self.config_builder.build_cloned()?;
		config.get("illustrator")
	}
}

pub trait Bell {
	fn book_updated(&self, book_id: BookId);
}

#[derive(Debug)]
enum LibraryTask {
	#[allow(unused)]
	Discover,
	Read(BookId),
}

pub struct Scribe<B: Bell> {
	system: WranglerSystem,
	records: RecordKeeperAssistant,
	bell: B,
	thumbnail_path: PathBuf,
	tasks: Arc<Mutex<BTreeMap<Ticket, LibraryTask>>>,
	discovery_ticket: Option<Ticket>,
	library_books: BTreeMap<PathBuf, Book>,
	stale_books: BinaryHeap<(SystemTime, BookId, DocumentId)>,
	buffer: Vec<u8>,
}

#[derive(Clone)]
pub struct ScribeAssistant {
	system: WranglerSystem,
	tasks: Arc<Mutex<BTreeMap<Ticket, LibraryTask>>>,
}

impl<B: Bell + Send + 'static> Scribe<B> {
	/// Create and register a wrangler instance.
	///
	/// Should be called once during init.
	/// returns `LibraryWranglerAssistant` which can be cheaply cloned.
	pub fn create(
		system: WranglerSystem,
		bell: B,
		records: RecordKeeperAssistant,
		paths: &Paths,
	) -> ScribeAssistant {
		let thumbnail_path = paths.cache_path.join("thumbnails");

		let tasks = Arc::new(Mutex::new(BTreeMap::new()));

		log::info!("Register wrangler");
		system.register(Box::new(Self {
			system: system.clone(),
			records,
			bell,
			thumbnail_path,
			tasks: tasks.clone(),
			discovery_ticket: None,
			library_books: BTreeMap::new(),
			stale_books: BinaryHeap::new(),
			buffer: Vec::new(),
		}));

		ScribeAssistant { system, tasks }
	}
}

impl ScribeAssistant {
	pub fn scan(&self) {
		let ticket = Ticket::take();
		self.tasks
			.lock()
			.unwrap()
			.insert(ticket, LibraryTask::Discover);
		self.system.send(WranglerCommand::ExploreTree(ticket));
	}
}

#[derive(Debug, thiserror::Error)]
enum ProcessError {
	#[error(transparent)]
	RecordKeeper(#[from] RecordKeeperError),
}

impl<B: Bell> Scribe<B> {
	fn process(&mut self, doc: &wrangler::DiscoveryDocument<'_>) -> Result<(), ProcessError> {
		if Path::new(doc.file_name)
			.extension()
			.is_none_or(|e| e != "epub")
		{
			log::debug!("Ignoring non-epub file: {}", doc.file_name);
			return Ok(());
		}

		let path = doc.document.path();
		let file_name = doc.file_name;
		let modified_at: DateTime<Utc> = <DateTime<Utc>>::from(doc.timestamp).trunc_subsecs(0);

		if let Some(book) = self.library_books.remove(path) {
			if book.modified_at < modified_at {
				log::debug!(
					"Book stale by timestamp: {file_name} {} vs {}",
					book.modified_at,
					modified_at
				);
				self.stale_books
					.push((doc.timestamp, book.id, doc.document.clone()));
			} else if book.size < doc.size {
				log::debug!("Book stale by size: {file_name}");
				self.stale_books
					.push((doc.timestamp, book.id, doc.document.clone()));
			} else {
				log::trace!("Book is fresh: {file_name}");
				self.stale_books
					.push((doc.timestamp, book.id, doc.document.clone()));
			}
		} else {
			let book = InsertBook {
				path: path.to_path_buf(),
				title: None,
				author: None,
				size: doc.size,
				modified_at,
				added_at: Utc::now(),
			};
			let book_id = self.records.upsert_book(book)?;
			self.stale_books
				.push((doc.timestamp, book_id, doc.document.clone()));
		}
		Ok(())
	}

	fn finish(&mut self) -> Result<(), ProcessError> {
		let unexist_ids = self
			.library_books
			.values()
			.map(|book| book.id)
			.collect::<Vec<_>>();
		log::debug!("Got {} unexist books", unexist_ids.len());
		self.records.unexist_books(&unexist_ids)?;
		for book_id in unexist_ids {
			self.bell.book_updated(book_id);
		}
		Ok(())
	}

	fn send_stale_batch(&mut self) {
		log::debug!("Request 5 more stale files");
		for _ in 0..5 {
			if let Some((_, id, doc)) = self.stale_books.pop() {
				let ticket = Ticket::take();
				self.tasks
					.lock()
					.unwrap()
					.insert(ticket, LibraryTask::Read(id));
				self.system.send(WranglerCommand::Document(ticket, doc));
			}
		}
	}
}

#[derive(Debug, thiserror::Error)]
enum ProcessFileError {
	#[error(transparent)]
	RecordKeeper(#[from] RecordKeeperError),
	#[error("at {1}: {0}")]
	Zip(
		zip::result::ZipError,
		&'static std::panic::Location<'static>,
	),
	#[error(transparent)]
	QuickXml(#[from] quick_xml::Error),
	#[error(transparent)]
	CreateThumbnail(#[from] CreateThumbnailError),
	#[error("No epub package root file in zip")]
	NoEpubRootFile,
	#[error("at {1}: {0}")]
	Io(io::Error, &'static std::panic::Location<'static>),
}

impl From<zip::result::ZipError> for ProcessFileError {
	#[track_caller]
	fn from(err: zip::result::ZipError) -> Self {
		Self::Zip(err, std::panic::Location::caller())
	}
}

impl From<io::Error> for ProcessFileError {
	#[track_caller]
	fn from(err: std::io::Error) -> Self {
		Self::Io(err, std::panic::Location::caller())
	}
}

impl<B: Bell> Scribe<B> {
	fn process_file(
		&mut self,
		book_id: BookId,
		content: &FileContent<'_>,
	) -> Result<(), ProcessFileError> {
		let mut archive = ZipArchive::new(content.file)?;

		let file = archive.by_path(Path::new(EPUB_CONTAINER_PATH))?;
		let root_path = parse_container(quick_xml::Reader::from_reader(io::BufReader::new(file)))?;
		let Some(root_path) = root_path else {
			return Err(ProcessFileError::NoEpubRootFile);
		};
		let root_dir = root_path.as_path().parent().unwrap_or(Path::new(""));
		let file = archive.by_path(&root_path)?;
		let package = parse_package(
			root_dir,
			quick_xml::Reader::from_reader(io::BufReader::new(file)),
		)?;

		log::debug!(
			"Found and parsed epub, title: {}",
			package.metadata.title.as_deref().unwrap_or_default()
		);

		let modified_at: DateTime<Utc> = <DateTime<Utc>>::from(content.timestamp).trunc_subsecs(0);
		let book = UpdateBook {
			book_id: book_id.value(),
			title: package.metadata.title.as_deref(),
			author: package.metadata.creator.as_deref(),
			modified_at,
		};
		self.records.update_book(book)?;
		self.bell.book_updated(book_id);

		if let Some(cover) = package
			.metadata
			.cover
			.as_ref()
			.and_then(|id| package.manifest.get(id))
		{
			let thumbnail_path = self
				.thumbnail_path
				.join(format!("thumbnail_{}.png", book_id.value()));
			if !thumbnail_path.try_exists()? {
				self.buffer.clear();
				archive
					.by_path(cover.as_path())?
					.read_to_end(&mut self.buffer)?;

				create_thumbnail(&thumbnail_path, &self.buffer)?;

				self.records
					.record_thumbnail(book_id, Some(&thumbnail_path))?;
				self.bell.book_updated(book_id);
			} else {
				log::debug!("Thumbnail for {book_id} exists");
			}
		}

		Ok(())
	}
}

impl<B: Bell + Sized + Send> Wrangler for Scribe<B> {
	fn discover(
		&mut self,
		ticket: wrangler::Ticket,
		discovery: Discovery,
	) -> wrangler::WranglerResult {
		if self.discovery_ticket.is_some_and(|t| t == ticket) {
			match discovery {
				Discovery::Begin => unreachable!("Unexpected message, must have been sent twice"),
				Discovery::Document(doc) => {
					let file_name = doc.file_name;
					match self.process(doc) {
						Ok(()) => {}
						Err(e) => {
							log::error!("Failed to process {file_name}: {e}");
						}
					};
				}
				Discovery::End => {
					match self.finish() {
						Ok(()) => {}
						Err(e) => {
							log::error!("Failed to send discovery end: {e}");
						}
					}
					self.send_stale_batch();
					self.tasks.lock().unwrap().remove(&ticket);
				}
			}

			WranglerResult::Handled
		} else if self.tasks.lock().unwrap().contains_key(&ticket) {
			debug_assert!(
				matches!(discovery, Discovery::Begin),
				"Unexpected first discovery message"
			);
			// TODO: Make specific fetch for this
			self.library_books = self
				.records
				.fetch_books()
				.unwrap_or_default()
				.into_values()
				.map(|book| (book.path.clone(), book))
				.collect();

			self.discovery_ticket = Some(ticket);
			self.stale_books.clear();

			WranglerResult::Handled
		} else {
			WranglerResult::SomebodyElsesProblem
		}
	}

	fn file<'a>(
		&mut self,
		ticket: Ticket,
		result: &Result<wrangler::FileContent<'a>, std::io::Error>,
	) -> wrangler::WranglerResult {
		let (task, len) = {
			let mut tasks = self.tasks.lock().unwrap();
			let task = tasks.remove(&ticket);
			let len = tasks.len();
			(task, len)
		};
		if let Some(LibraryTask::Read(book_id)) = task {
			match result {
				Ok(content) => match self.process_file(book_id, content) {
					Ok(_) => {}
					Err(e) => {
						log::error!("Failed to process file for {book_id}: {e}");
					}
				},
				Err(e) => {
					log::error!("Failed to process file for {book_id}: {e}");
				}
			}

			if len == 0 {
				self.send_stale_batch();
			}

			WranglerResult::Handled
		} else {
			WranglerResult::SomebodyElsesProblem
		}
	}
}

#[derive(Debug, thiserror::Error)]
pub enum CreateThumbnailError {
	#[error("at {1}: {0}")]
	Io(std::io::Error, &'static std::panic::Location<'static>),
	#[error(transparent)]
	Image(#[from] image::ImageError),
}

impl From<std::io::Error> for CreateThumbnailError {
	#[track_caller]
	fn from(err: std::io::Error) -> Self {
		Self::Io(err, std::panic::Location::caller())
	}
}

fn create_thumbnail(path: &Path, bytes: &[u8]) -> Result<(), CreateThumbnailError> {
	const THUMBNAIL_SIZE: u32 = 320;

	let img = ImageReader::new(io::Cursor::new(bytes))
		.with_guessed_format()?
		.decode()?;
	let img = img.resize(
		THUMBNAIL_SIZE,
		THUMBNAIL_SIZE,
		image::imageops::FilterType::CatmullRom,
	);

	log::debug!("Save thumbnail as {}", path.display());
	let mut file = match fs::File::create(path) {
		Ok(file) => file,
		Err(e) if e.kind() == io::ErrorKind::NotFound => {
			if let Some(parent) = path.parent() {
				fs::create_dir_all(parent)?;
			}
			fs::File::create(path)?
		}
		Err(e) => {
			return Err(e.into());
		}
	};

	let encoder = PngEncoder::new_with_quality(
		&mut file,
		png::CompressionType::Fast,
		png::FilterType::default(),
	);
	img.write_with_encoder(encoder)?;

	Ok(())
}
