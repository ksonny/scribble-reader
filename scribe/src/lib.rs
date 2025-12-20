pub mod library;
pub mod record_keeper;
mod secret_storage;
pub mod settings;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::BinaryHeap;
use std::fs;
use std::io;
use std::path::PathBuf;
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
	fn library_loaded(&self);

	fn library_sorted(&self);

	fn book_updated(&self, id: BookId);

	fn fail(&self, ticket: ScribeTicket, error: String);

	fn complete(&self, ticket: ScribeTicket);
}

#[derive(Debug)]
pub enum ScribeRequest {
	Scan,
	Show(BookId),
	#[allow(dead_code)]
	Sort(SortOrder),
}

pub struct Scribe {
	lib: library::Library,
	order_tx: Sender<(ScribeTicket, ScribeRequest)>,
	handle: JoinHandle<Result<(), ScribeError>>,
	ticket_set: BTreeSet<ScribeTicket>,
	ticket_cnt: Cell<usize>,
}

#[derive(Debug)]
pub struct Settings {
	pub cache_path: PathBuf,
	pub config_path: PathBuf,
	pub data_path: PathBuf,
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
		settings: &Settings,
	) -> Result<Self, ScribeCreateError> {
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
			return Err(ScribeCreateError::CachePathNotDir(
				settings.cache_path.clone(),
			));
		}
		if !settings.data_path.try_exists()? {
			fs::create_dir_all(&settings.data_path)?;
		}
		if !settings.data_path.is_dir() {
			return Err(ScribeCreateError::DataPathNotDir(
				settings.data_path.clone(),
			));
		}
		let state_db_path = settings.data_path.join("state.db");

		let lib = library::Library::default();
		let records = record_keeper::create(&state_db_path)?;
		let storage = secret_storage::create(&settings.cache_path)?;
		let (order_tx, order_rx) = channel();
		let handle = spawn_scribe(bell, lib_path, lib.clone(), records, storage, order_rx);

		Ok(Scribe {
			lib,
			order_tx,
			handle,
			ticket_set: BTreeSet::new(),
			ticket_cnt: Cell::new(0),
		})
	}

	pub fn library(&self) -> &library::Library {
		&self.lib
	}

	pub fn complete_ticket(&mut self, ticket: ScribeTicket) -> ScribeState {
		let set = &mut self.ticket_set;
		set.remove(&ticket);
		log::trace!("Completed ticket {ticket:?}, set has {} tickets", set.len());
		if set.is_empty() {
			ScribeState::Idle
		} else {
			ScribeState::Working
		}
	}

	fn new_ticket(&self) -> ScribeTicket {
		let ticket_id = self.ticket_cnt.get();
		self.ticket_cnt.set(ticket_id + 1);
		ScribeTicket(ticket_id)
	}

	fn send(&mut self, order: ScribeRequest) -> ScribeState {
		let ticket = self.new_ticket();
		match self.order_tx.send((ticket, order)) {
			Ok(_) => {
				self.ticket_set.insert(ticket);
				// TODO: Do something with ticket
				ScribeState::Working
			}
			Err(e) => {
				log::info!("Error sending to scribe: {e}");
				todo!();
			}
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
	order_rx: std::sync::mpsc::Receiver<(ScribeTicket, ScribeRequest)>,
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
		bell.library_loaded();

		loop {
			let request = order_rx.recv();
			log::trace!("Request received: {request:?}");
			match request {
				Ok((ticket, ScribeRequest::Scan)) => {
					log::info!("Scan library at {}", lib_path.display());
					match storage.scan(&mut records, &lib_path) {
						Ok(_) => {}
						Err(e) => {
							log::error!("Failed to scan library: {e}");
							bell.fail(ticket, format!("Failed to scan library: {e}"));
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
							bell.library_loaded();
							bell.complete(ticket);
						}
						Err(e) => {
							log::error!("Failed to load library: {e}");
							bell.fail(ticket, format!("Failed to load library: {e}"));
						}
					};
				}
				Ok((ticket, ScribeRequest::Show(id))) => {
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
								bell.book_updated(id);
								bell.complete(ticket);
							}
							Err(e) => {
								log::error!("Failed to get thumbnail for {id:?}: {e}");
								bell.fail(
									ticket,
									format!("Failed to get thumbnail for {id:?}: {e}"),
								);
							}
						};
					} else {
						bell.complete(ticket);
					}
				}
				Ok((ticket, ScribeRequest::Sort(order))) => {
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
					bell.complete(ticket);
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

pub trait ScribeAssistant {
	fn poke_scan(&mut self) -> ScribeState;
	fn poke_list(&mut self, books: &[library::Book]) -> ScribeState;
}

impl ScribeAssistant for Scribe {
	fn poke_scan(&mut self) -> ScribeState {
		self.send(ScribeRequest::Scan)
	}

	fn poke_list(&mut self, books: &[library::Book]) -> ScribeState {
		let ids = books.iter().map(|b| b.id).collect::<Vec<_>>();
		for id in ids {
			self.send(ScribeRequest::Show(id));
		}
		ScribeState::Working
	}
}
