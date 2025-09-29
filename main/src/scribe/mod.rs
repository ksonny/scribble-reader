#![allow(dead_code)]
mod settings;

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
	#[error(transparent)]
	SecretStorage(#[from] secret_storage::SecretStorageError),
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

	fn complete(&self, ticket: ScribeTicket) {
		let _ = ticket;
	}

	fn fail(&self, error: String);
}

#[derive(Debug)]
pub(crate) enum ScribeRequest {
	Scan,
	List(Vec<BookId>),
	Sort(library::SortOrder),
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

		let lib_path = settings.data_path.join(scribe_settings.library.path);
		if !lib_path.try_exists()? {
			fs::create_dir_all(&lib_path)?;
		}
		if !lib_path.is_dir() {
			return Err(ScribeCreateError::LibraryPathNotDir(lib_path.to_path_buf()));
		}

		let options = ScribeOptions::from(&settings);
		let lib = library::Library::default();
		let worker_lib = lib.clone();
		let storage = secret_storage::connect(&options.state_db_path)?;
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
				Ok((ticket, ScribeRequest::Scan)) => {
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
					bell.complete(ticket);
				}
				Ok((ticket, ScribeRequest::List(ids))) => {
					// TODO: Actually do something
					for id in ids {
						bell.book_updated(id);
					}
					bell.complete(ticket);
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

	pub fn poke_list(&self, books: &[library::Book]) -> () {
		let ids = books.iter().map(|b| b.id).collect::<Vec<_>>();
		if !ids.is_empty() {
			self.send(ScribeRequest::List(ids));
		}
	}
}

mod library {
	use std::collections::BTreeMap;
	use std::path::PathBuf;
	use std::sync::Arc;
	use std::sync::RwLock;

	use chrono::DateTime;
	use chrono::Utc;

	use crate::scribe::library;

	#[derive(Debug, Default, Clone, Copy)]
	pub(crate) enum SortField {
		#[default]
		Added,
		Modified,
		Title,
	}

	#[derive(Debug, Default, Clone, Copy)]
	pub(crate) enum SortDirection {
		#[default]
		Ascending,
		Descending,
	}

	#[derive(Debug, Default, Clone, Copy)]
	pub(crate) struct SortOrder(pub SortField, pub SortDirection);

	#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Copy)]
	pub struct BookId(pub i64);

	#[derive(Debug, Clone)]
	pub struct Book {
		pub id: BookId,
		pub path: PathBuf,
		pub title: Option<Arc<String>>,
		pub author: Option<Arc<String>>,
		pub size: i64,
		pub modified_at: DateTime<Utc>,
		pub added_at: DateTime<Utc>,
	}

	#[derive(Debug, Default)]
	pub(crate) struct SecretLibrary {
		pub(crate) books: BTreeMap<BookId, Book>,
		pub(crate) order: SortOrder,
		pub(crate) sorted: Vec<BookId>,
	}

	#[derive(Default, Clone)]
	pub struct Library(Arc<RwLock<SecretLibrary>>);

	impl Library {
		pub(crate) fn read(
			&self,
		) -> Result<
			std::sync::RwLockReadGuard<'_, SecretLibrary>,
			std::sync::PoisonError<std::sync::RwLockReadGuard<'_, SecretLibrary>>,
		> {
			let Library(lib) = self;
			lib.read()
		}

		pub(crate) fn write(
			&self,
		) -> std::result::Result<
			std::sync::RwLockWriteGuard<'_, SecretLibrary>,
			std::sync::PoisonError<std::sync::RwLockWriteGuard<'_, SecretLibrary>>,
		> {
			let Library(lib) = self;
			lib.write()
		}

		pub fn book(&self, id: BookId) -> Option<Book> {
			let lib = self.read().unwrap();
			lib.books.get(&id).cloned()
		}

		pub fn books(&self, n: std::ops::Range<u32>) -> Vec<library::Book> {
			let lib = self.read().unwrap();
			let start = n.start as usize;
			let end = (n.end as usize).min(lib.sorted.len());
			let books = lib
				.sorted
				.get(start..end)
				.into_iter()
				.flatten()
				.filter_map(|id| lib.books.get(id).cloned())
				.collect::<Vec<_>>();
			log::info!(
				"Requested books {start}..{end}, received {} from all books {}",
				books.len(),
				lib.sorted.len()
			);
			books
		}
	}
}

mod secret_storage {
	use chrono::DateTime;
	use chrono::Utc;
	use chrono::serde::ts_seconds;
	use epub::doc::EpubDoc;
	use rusqlite_migration::M;
	use rusqlite_migration::Migrations;
	use serde::Deserialize;
	use serde::Serialize;
	use serde_rusqlite::from_rows;
	use serde_rusqlite::to_params_named;

	use std::collections::BTreeMap;
	use std::fs;
	use std::os::unix::fs::MetadataExt;
	use std::path::Path;
	use std::path::PathBuf;
	use std::sync::Arc;

	use crate::scribe::library;

	#[derive(Debug, thiserror::Error)]
	pub enum SecretStorageError {
		#[error("at {1}: {0}")]
		Io(std::io::Error, &'static std::panic::Location<'static>),
		#[error("at {1}: {0}")]
		Rusqlite(rusqlite::Error, &'static std::panic::Location<'static>),
		#[error("at {1}: {0}")]
		RusqliteFromSql(
			rusqlite::types::FromSqlError,
			&'static std::panic::Location<'static>,
		),
		#[error(transparent)]
		SerdeRusqlite(#[from] serde_rusqlite::Error),
		#[error(transparent)]
		SystemTime(#[from] std::time::SystemTimeError),
		#[error(transparent)]
		Doc(#[from] epub::doc::DocError),
		#[error("Scan path is not directory")]
		ScanPathNotDir,
	}

	impl From<rusqlite::Error> for SecretStorageError {
		#[track_caller]
		fn from(err: rusqlite::Error) -> Self {
			Self::Rusqlite(err, std::panic::Location::caller())
		}
	}

	impl From<rusqlite::types::FromSqlError> for SecretStorageError {
		#[track_caller]
		fn from(err: rusqlite::types::FromSqlError) -> Self {
			Self::RusqliteFromSql(err, std::panic::Location::caller())
		}
	}

	impl From<std::io::Error> for SecretStorageError {
		#[track_caller]
		fn from(err: std::io::Error) -> Self {
			Self::Io(err, std::panic::Location::caller())
		}
	}

	#[derive(Debug, Deserialize)]
	pub struct SecretBook {
		pub(crate) id: i64,
		pub(crate) path: PathBuf,
		pub(crate) title: Option<String>,
		pub(crate) author: Option<String>,
		pub(crate) size: i64,
		#[serde(with = "ts_seconds")]
		pub(crate) modified_at: DateTime<Utc>,
		#[serde(with = "ts_seconds")]
		pub(crate) added_at: DateTime<Utc>,
	}

	impl From<SecretBook> for library::Book {
		fn from(value: SecretBook) -> Self {
			library::Book {
				id: library::BookId(value.id),
				path: value.path,
				title: value.title.map(Arc::new),
				author: value.author.map(Arc::new),
				size: value.size,
				modified_at: value.modified_at,
				added_at: value.added_at,
			}
		}
	}

	const MIGRATIONS_SLICE: &[M<'_>] = &[M::up(
		"create table books (
			id integer primary key,
			path text not null unique,
			title text,
			author text,
			size integer not null,
			modified_at integer not null,
			added_at integer not null,
			exist boolean not null
		);",
	)];
	const MIGRATIONS: Migrations<'_> = Migrations::from_slice(MIGRATIONS_SLICE);

	pub fn connect(db_path: &Path) -> Result<SecretStorage, SecretStorageError> {
		let mut conn = rusqlite::Connection::open(db_path)?;

		conn.pragma_update(None, "mmap_size", 30000000000u64)?;
		conn.pragma_update(None, "page_size", 32768u64)?;
		conn.pragma_update(None, "foreign_keys", "on")?;
		conn.pragma_update(None, "journal_mode", "WAL")?;

		MIGRATIONS.to_latest(&mut conn).unwrap();

		Ok(SecretStorage { conn })
	}

	#[derive(Debug, Serialize)]
	struct InsertBook {
		path: PathBuf,
		title: Option<String>,
		author: Option<String>,
		size: i64,
		#[serde(with = "ts_seconds")]
		modified_at: DateTime<Utc>,
		#[serde(with = "ts_seconds")]
		added_at: DateTime<Utc>,
	}

	impl From<(i64, InsertBook)> for library::Book {
		fn from(value: (i64, InsertBook)) -> library::Book {
			let (
				id,
				InsertBook {
					path,
					title,
					author,
					size,
					modified_at,
					added_at,
				},
			) = value;
			library::Book {
				id: library::BookId(id),
				path,
				title: title.map(Arc::new),
				author: author.map(Arc::new),
				size,
				modified_at,
				added_at,
			}
		}
	}

	pub struct SecretStorage {
		conn: rusqlite::Connection,
	}

	impl SecretStorage {
		pub fn scan(
			&mut self,
			path: &Path,
		) -> Result<BTreeMap<library::BookId, library::Book>, SecretStorageError> {
			if !path.is_dir() {
				return Err(SecretStorageError::ScanPathNotDir);
			}

			let tx = self.conn.transaction()?;
			tx.execute("update books set exist = false;", [])?;

			let mut books = BTreeMap::new();
			{
				let mut upsert_stmt = tx.prepare(
					"insert into books (path, title, author, size, modified_at, added_at, exist)
					values (:path, :title, :author, :size, :modified_at, :added_at, true)
					on conflict (path)
					do update set
						title = :title,
						author = :author,
						size = :size,
						modified_at = :modified_at,
						exist = true
					returning id;
				",
				)?;
				for entry in fs::read_dir(path)? {
					let entry = entry?;
					match scan_book(&entry) {
						Ok(book) => {
							log::trace!(
								"Found {} by {}",
								book.title.as_deref().unwrap_or("Unknown"),
								book.author.as_deref().unwrap_or("Unknown")
							);
							let params = to_params_named(&book)?;
							let params = params.to_slice();
							let book_id =
								upsert_stmt.query_row(params.as_slice(), |row| row.get(0))?;
							let book: library::Book = (book_id, book).into();
							books.insert(book.id, book);
						}
						Err(e) => {
							log::error!(
								"Failed to read book '{}': {e}",
								entry.file_name().display()
							);
						}
					};
				}
			}
			log::trace!("Total {} books", books.len());

			tx.commit()?;
			Ok(books)
		}

		pub fn get_books(
			&self,
		) -> Result<BTreeMap<library::BookId, library::Book>, SecretStorageError> {
			let mut stmt = self.conn.prepare(
				"select
					id,
					path,
					title,
					author,
					size,
					modified_at,
					added_at
				from books
				where exist = true;
			",
			)?;
			let series = from_rows::<SecretBook>(stmt.query([])?)
				.map(|book| book.map(|b| (library::BookId(b.id), b.into())))
				.collect::<Result<BTreeMap<_, _>, _>>()?;
			Ok(series)
		}
	}

	fn scan_book(entry: &fs::DirEntry) -> Result<InsertBook, SecretStorageError> {
		let path = entry.path();
		let doc = EpubDoc::new(&path)?;
		let title = doc.mdata("title");
		let author = doc.mdata("creator");
		let size = entry.metadata()?.size() as i64;
		let modified_at = entry.metadata()?.modified()?.into();
		let added_at = Utc::now();

		Ok(InsertBook {
			path,
			title,
			author,
			size,
			modified_at,
			added_at,
		})
	}
}
