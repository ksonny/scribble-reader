#![allow(dead_code)]
pub mod library;
mod secret_storage;
pub mod settings;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::collections::BinaryHeap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::RecvError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread;
use std::thread::JoinHandle;

#[cfg(not(target_os = "android"))]
use expand_tilde::expand_tilde_owned;

pub use crate::scribe::library::BookId;
use crate::scribe::library::SortDirection;
use crate::scribe::library::SortField;
use crate::scribe::library::SortOrder;

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
	ExpandTilde(#[from] expand_tilde::Error),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ScribeError {
	#[error(transparent)]
	Recv(#[from] RecvError),
	#[error(transparent)]
	SecretStorage(#[from] secret_storage::SecretStorageError),
}

#[derive(Debug, Clone, Copy)]
pub struct ScribeTicket(usize);

pub(crate) trait ScribeBell {
	fn library_loaded(&self);

	fn library_sorted(&self);

	fn book_updated(&self, id: BookId);

	fn fail(&self, error: String);
}

#[derive(Debug)]
pub(crate) enum ScribeRequest {
	Scan,
	Show(BookId),
	Sort(library::SortOrder),
	OpenBook(BookId),
	Next,
	Previous,
}

pub(crate) struct Scribe {
	lib: library::Library,
	order_tx: Sender<(ScribeTicket, ScribeRequest)>,
	handle: JoinHandle<Result<(), ScribeError>>,
	ticket_cnt: Rc<Cell<usize>>,
}

pub(crate) struct ScribeAssistant {
	order_tx: Sender<(ScribeTicket, ScribeRequest)>,
	ticket_cnt: Rc<Cell<usize>>,
}

#[derive(Debug)]
pub struct Settings {
	pub cache_path: PathBuf,
	pub config_path: PathBuf,
	pub data_path: PathBuf,
}

pub struct ScribeOptions {
	state_db_path: PathBuf,
	thumbnail_path: PathBuf,
}

impl From<&Settings> for ScribeOptions {
	fn from(value: &Settings) -> Self {
		let state_db_path = value.data_path.join("state.db");
		let thumbnail_path = value.cache_path.join("thumbnails");

		Self {
			state_db_path,
			thumbnail_path,
		}
	}
}

impl Scribe {
	pub(crate) fn create<Bell>(bell: Bell, settings: Settings) -> Result<Self, ScribeCreateError>
	where
		Bell: ScribeBell + Send + 'static,
	{
		log::info!("Create scribe with {:?}", settings);

		let config_path = settings.config_path.join("config.toml");
		let scribe_settings: settings::Scribe = config::Config::builder()
			.add_source(config::File::from_str(
				settings::DEFAULT_SCRIBE_CONFIG,
				config::FileFormat::Toml,
			))
			.add_source(config::File::from(config_path).required(false))
			.add_source(config::Environment::with_prefix("SCRAPE").separator("_"))
			.build()?
			.try_deserialize()?;

		#[cfg(target_os = "android")]
		let lib_path = scribe_settings.library.path;
		#[cfg(not(target_os = "android"))]
		let lib_path = expand_tilde_owned(scribe_settings.library.path)?;
		if !lib_path.try_exists()? {
			fs::create_dir_all(&lib_path)?;
		}
		if !lib_path.is_dir() {
			return Err(ScribeCreateError::LibraryPathNotDir(lib_path.to_path_buf()));
		}
		if !settings.cache_path.try_exists()? {
			fs::create_dir_all(&settings.cache_path)?;
		}
		if !settings.cache_path.is_dir() {
			return Err(ScribeCreateError::CachePathNotDir(settings.cache_path));
		}
		if !settings.data_path.try_exists()? {
			fs::create_dir_all(&settings.data_path)?;
		}
		if !settings.data_path.is_dir() {
			return Err(ScribeCreateError::DataPathNotDir(settings.data_path));
		}

		let options = ScribeOptions::from(&settings);
		let lib = library::Library::default();
		let worker_lib = lib.clone();
		let storage = secret_storage::create(&options.state_db_path, &settings.cache_path)?;
		let (order_tx, order_rx) = channel();
		let handle = spawn_scribe(bell, lib_path, worker_lib, storage, order_rx);

		Ok(Scribe {
			lib,
			order_tx,
			handle,
			ticket_cnt: Rc::new(Cell::new(0)),
		})
	}

	pub fn quit(self) -> Result<(), ScribeError> {
		drop(self.order_tx);
		self.handle.join().unwrap()?;
		Ok(())
	}

	pub fn assistant(&self) -> ScribeAssistant {
		ScribeAssistant {
			order_tx: self.order_tx.clone(),
			ticket_cnt: self.ticket_cnt.clone(),
		}
	}

	pub fn library(&self) -> &library::Library {
		&self.lib
	}
}

fn spawn_scribe<Bell>(
	bell: Bell,
	lib_path: PathBuf,
	worker_lib: library::Library,
	mut storage: secret_storage::SecretStorage,
	order_rx: std::sync::mpsc::Receiver<(ScribeTicket, ScribeRequest)>,
) -> JoinHandle<Result<(), ScribeError>>
where
	Bell: ScribeBell + Send + 'static,
{
	thread::spawn(move || -> Result<(), ScribeError> {
		let lib = worker_lib;
		log::info!("Started scribe worker");

		let books: BTreeMap<_, _> = storage.get_books().inspect_err(|e| log::error!("{e}"))?;
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
		bell.library_loaded();

		loop {
			let request = order_rx.recv();
			log::info!("Request received: {request:?}");
			match request {
				Ok((_ticket, ScribeRequest::Scan)) => {
					log::info!("Scan library at {}", lib_path.display());
					let books: BTreeMap<_, _> = storage
						.scan(&lib_path)
						.inspect_err(|e| log::error!("{e}"))?;
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
					bell.library_loaded();
				}
				Ok((_ticket, ScribeRequest::Show(id))) => {
					let has_thumbnail = {
						let lib = lib.read().unwrap();
						lib.thumbnails.contains_key(&id)
					};
					if !has_thumbnail {
						match storage.load_thumbnail(id) {
							Ok(tn) => {
								if tn.is_some() {
									log::trace!("Got thumbnail for {:?}", id);
								} else {
									log::trace!("No thumbnail for {:?}", id);
								}
								let mut lib = lib.write().unwrap();
								lib.thumbnails.insert(id, tn.into());
								bell.book_updated(id);
							}
							Err(e) => {
								log::error!("Failed to get thumbnail for {id:?}: {e}");
							}
						};
					}
				}
				Ok((_ticket, ScribeRequest::Sort(order))) => {
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
					bell.library_sorted();
				}
				Ok((_ticket, ScribeRequest::OpenBook(_id))) => {
				}
				Ok((_ticket, ScribeRequest::Next)) => {
				}
				Ok((_ticket, ScribeRequest::Previous)) => {
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

impl ScribeAssistant {
	fn new_ticket(&self) -> ScribeTicket {
		let ticket_id = self.ticket_cnt.get();
		self.ticket_cnt.set(ticket_id + 1);
		ScribeTicket(ticket_id)
	}

	pub fn send(&self, order: ScribeRequest) {
		let ticket = self.new_ticket();
		match self.order_tx.send((ticket, order)) {
			Ok(_) => {
				// TODO: Do something with ticket
			}
			Err(e) => {
				log::info!("Error sending to scribe: {e}");
				todo!()
			}
		};
	}

	pub fn poke_list(&self, books: &[library::Book]) {
		let ids = books.iter().map(|b| b.id).collect::<Vec<_>>();
		let ticket = self.new_ticket();
		for id in ids {
			match self.order_tx.send((ticket, ScribeRequest::Show(id))) {
				Ok(_) => {
					// TODO: Do something with ticket
				}
				Err(e) => {
					log::info!("Error sending to scribe: {e}");
					todo!()
				}
			};
		}
	}

	pub(crate) fn poke_book_open(&self, id: BookId) {
		self.send(ScribeRequest::OpenBook(id));
	}

	pub(crate) fn poke_next(&self) {
		self.send(ScribeRequest::Next);
	}

	pub(crate) fn poke_previous(&self) {
		self.send(ScribeRequest::Previous);
	}
}
