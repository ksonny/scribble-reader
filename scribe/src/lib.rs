pub mod library;
pub mod record_keeper;
mod secret_storage;
pub mod settings;

use std::collections::BTreeMap;
use std::collections::BinaryHeap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::RecvError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread;
use std::thread::JoinHandle;

#[cfg(not(target_os = "android"))]
use expand_tilde::expand_tilde_owned;

use crate::library::BookId;
use crate::library::SortDirection;
use crate::library::SortField;
use crate::library::SortOrder;

#[derive(Debug, thiserror::Error)]
pub enum ScribeCreateError {
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Config(#[from] config::ConfigError),
	#[error("Library path is not directory: {0}")]
	LibraryPathNotDir(PathBuf),
	#[error("Cache path is not directory: {0}")]
	CachePathNotDir(PathBuf),
	#[error("Data path is not directory: {0}")]
	DataPathNotDir(PathBuf),
	#[error(transparent)]
	SecretStorage(#[from] secret_storage::SecretStorageError),
	#[error(transparent)]
	SecretReords(#[from] record_keeper::RecordKeeperError),
	#[error(transparent)]
	ExpandTilde(#[from] expand_tilde::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ScribeError {
	#[error(transparent)]
	Recv(#[from] RecvError),
	#[error(transparent)]
	SecretStorage(#[from] secret_storage::SecretStorageError),
	#[error(transparent)]
	SecretReords(#[from] record_keeper::RecordKeeperError),
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
pub struct ScribeTicket(usize);

pub trait Bell {
	fn library_updated(&self, book_id: Option<BookId>);
}

#[derive(Debug)]
pub enum ScribeRequest {
	Scan,
	Show(BookId),
	#[allow(dead_code)]
	Sort(SortOrder),
}

#[derive(Clone)]
pub struct ScribeConfig {
	paths: Arc<settings::Paths>,
	config_builder: Arc<config::ConfigBuilder<config::builder::DefaultState>>,
}

impl ScribeConfig {
	pub fn new(paths: settings::Paths) -> Self {
		let config_path = paths.config_path.join("config.toml");
		let config_builder = config::Config::builder()
			.add_source(config::File::from_str(
				settings::DEFAULT_SCRIBE_CONFIG,
				config::FileFormat::Toml,
			))
			.add_source(config::File::from(config_path.as_path()).required(false))
			.add_source(config::Environment::with_prefix("SCRAPE").separator("_"));
		let paths = Arc::new(paths);
		let config_builder = Arc::new(config_builder);

		Self {
			paths,
			config_builder,
		}
	}

	pub fn paths(&self) -> &settings::Paths {
		&self.paths
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

pub struct Scribe {
	lib: library::Library,
	order_tx: Sender<ScribeRequest>,
	handle: JoinHandle<Result<(), ScribeError>>,
}

pub struct ScribeAssistant {
	lib: library::Library,
	order_tx: Sender<ScribeRequest>,
}

impl ScribeAssistant {
	pub fn send(&self, order: ScribeRequest) -> ScribeState {
		match self.order_tx.send(order) {
			Ok(_) => ScribeState::Working,
			Err(e) => {
				log::info!("Error sending to scribe: {e}");
				todo!();
			}
		}
	}

	pub fn library(&self) -> &library::Library {
		&self.lib
	}
}

#[derive(Debug, Default, Copy, Clone)]
pub enum ScribeState {
	#[default]
	Idle,
	Working,
}

impl Scribe {
	pub fn create(
		bell: impl Bell + Send + 'static,
		config: ScribeConfig,
	) -> Result<Self, ScribeCreateError> {
		let paths = config.paths();
		let library_settings = config.library()?;

		log::info!("Create scribe with {:?}", paths);
		#[cfg(target_os = "android")]
		let lib_path = library_settings.path;
		#[cfg(not(target_os = "android"))]
		let lib_path = expand_tilde_owned(library_settings.path)?;
		if !lib_path.try_exists()? {
			fs::create_dir_all(&lib_path)?;
		}
		if !lib_path.is_dir() {
			return Err(ScribeCreateError::LibraryPathNotDir(lib_path.to_path_buf()));
		}
		if !paths.cache_path.try_exists()? {
			fs::create_dir_all(&paths.cache_path)?;
		}
		if !paths.cache_path.is_dir() {
			return Err(ScribeCreateError::CachePathNotDir(paths.cache_path.clone()));
		}
		if !paths.data_path.try_exists()? {
			fs::create_dir_all(&paths.data_path)?;
		}
		if !paths.data_path.is_dir() {
			return Err(ScribeCreateError::DataPathNotDir(paths.data_path.clone()));
		}
		let state_db_path = paths.data_path.join("state.db");

		let lib = library::Library::default();
		let records = record_keeper::create(&state_db_path)?;
		let storage = secret_storage::create(&paths.cache_path)?;
		let (order_tx, order_rx) = channel();
		let handle = spawn_scribe(bell, lib_path, lib.clone(), records, storage, order_rx);

		Ok(Scribe {
			lib,
			order_tx,
			handle,
		})
	}

	pub fn assistant(&self) -> ScribeAssistant {
		ScribeAssistant {
			lib: self.lib.clone(),
			order_tx: self.order_tx.clone(),
		}
	}

	pub fn quit(self) -> Result<(), ScribeError> {
		drop(self.order_tx);
		self.handle.join().unwrap()?;
		Ok(())
	}
}

fn spawn_scribe(
	bell: impl Bell + Send + 'static,
	lib_path: PathBuf,
	worker_lib: library::Library,
	mut records: record_keeper::RecordKeeper,
	storage: secret_storage::SecretStorage,
	order_rx: std::sync::mpsc::Receiver<ScribeRequest>,
) -> JoinHandle<Result<(), ScribeError>> {
	thread::spawn(move || -> Result<(), ScribeError> {
		let lib = worker_lib;
		log::info!("Started scribe worker");

		let books = records.fetch_books().inspect_err(|e| log::error!("{e}"))?;
		let books_len = books.len();
		let SortOrder(field, dir) = {
			let lib = lib.read().unwrap();
			lib.order
		};
		let sorted = sort_books(&books, field, dir);
		{
			let mut lib = lib.write().unwrap();
			lib.books = books;
			lib.sorted = sorted;
		}
		log::info!("Library loaded with {books_len} books");
		bell.library_updated(None);

		loop {
			let request = order_rx.recv();
			log::trace!("Request received: {request:?}");
			match request {
				Ok(ScribeRequest::Scan) => {
					log::info!("Scan library at {}", lib_path.display());
					match storage.scan(&mut records, &lib_path) {
						Ok(_) => {}
						Err(e) => {
							log::error!("Failed to scan library: {e}");
						}
					}
					match records.fetch_books() {
						Ok(books) => {
							let books_len = books.len();
							let SortOrder(field, dir) = {
								let lib = lib.read().unwrap();
								lib.order
							};
							let sorted = sort_books(&books, field, dir);
							{
								let mut lib = lib.write().unwrap();
								lib.books = books;
								lib.sorted = sorted;
							}
							log::info!("Library loaded with {books_len} books");
							bell.library_updated(None);
						}
						Err(e) => {
							log::error!("Failed to load library: {e}");
						}
					};
				}
				Ok(ScribeRequest::Show(id)) => {
					let has_thumbnail = lib.read().unwrap().thumbnails.contains_key(&id);
					if !has_thumbnail {
						match storage.load_thumbnail(&mut records, id) {
							Ok(tn) => {
								let tn = if let Some(bytes) = tn {
									log::trace!("Got thumbnail for {:?}", id);
									library::Thumbnail::Bytes {
										bytes: bytes.into(),
									}
								} else {
									log::trace!("No thumbnail for {:?}", id);
									library::Thumbnail::None
								};
								let mut lib = lib.write().unwrap();
								lib.thumbnails.insert(id, tn);
								bell.library_updated(Some(id));
							}
							Err(e) => {
								log::error!("Failed to get thumbnail for {id:?}: {e}");
							}
						};
					}
				}
				Ok(ScribeRequest::Sort(order)) => {
					let SortOrder(field, dir) = order;
					let sorted = {
						let lib = lib.read().unwrap();
						sort_books(&lib.books, field, dir)
					};
					{
						let mut lib = lib.write().unwrap();
						lib.order = order;
						lib.sorted = sorted;
					}
					bell.library_updated(None);
				}
				Err(RecvError) => {
					log::info!("Scribe worker terminated");
					break Ok(());
				}
			}
		}
	})
}

fn sort_books(
	books: &BTreeMap<BookId, library::Book>,
	field: SortField,
	dir: SortDirection,
) -> Vec<BookId> {
	let mut sorted: Vec<BookId> = match field {
		SortField::Added => books
			.values()
			.map(|book| (book.added_at, book.id))
			.collect::<BinaryHeap<_>>()
			.into_iter()
			.map(|(_, id)| id)
			.collect(),
		SortField::Modified => books
			.values()
			.map(|book| (book.modified_at, book.id))
			.collect::<BinaryHeap<_>>()
			.into_iter()
			.map(|(_, id)| id)
			.collect(),
		SortField::Title => books
			.values()
			.map(|book| (book.title.as_deref(), book.id))
			.collect::<BinaryHeap<_>>()
			.into_iter()
			.map(|(_, id)| id)
			.collect(),
	};
	if matches!(dir, SortDirection::Descending) {
		sorted.reverse();
	}
	sorted
}
