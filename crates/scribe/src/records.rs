use chrono::DateTime;
use chrono::Utc;
use chrono::serde::ts_seconds;
use chrono::serde::ts_seconds_option;
use fixed::types::U26F6;
use rusqlite_migration::M;
use rusqlite_migration::Migrations;
use serde::Deserialize;
use serde::Serialize;
use serde_rusqlite::from_row;
use serde_rusqlite::from_rows;
use serde_rusqlite::to_params_named;

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use crate::Book;
use crate::BookId;
use crate::Location;

const MIGRATIONS_SLICE: &[M<'_>] = &[
	M::up(
		"create table books (
			id integer primary key,
			path text not null unique,
			title text,
			author text,
			size integer not null,
			modified_at integer not null,
			added_at integer not null,
			exist integer not null check (exist in (0, 1))
		) strict;",
	),
	M::up(
		"create table book_cache_thumbnails (
			book_id integer primary key,
			path text,
			added_at integer not null,
			foreign key (book_id) references books(id)
				on update cascade
				on delete cascade
		) strict;",
	),
	M::up(
		"create table book_reading_state (
			book_id integer primary key,
			opened_at integer not null,
			spine integer,
			element integer,
			foreign key (book_id) references books(id)
				on update cascade
				on delete cascade
		) strict;",
	),
	M::up(
		"alter table book_reading_state
			add column percent_read integer;
		",
	),
];
const MIGRATIONS: Migrations<'_> = Migrations::from_slice(MIGRATIONS_SLICE);

#[derive(Debug, thiserror::Error)]
pub enum RecordKeeperError {
	#[error("at {1}: {0}")]
	Rusqlite(rusqlite::Error, &'static std::panic::Location<'static>),
	#[error("at {1}: {0}")]
	RusqliteFromSql(
		rusqlite::types::FromSqlError,
		&'static std::panic::Location<'static>,
	),
	#[error(transparent)]
	SerdeRusqlite(#[from] serde_rusqlite::Error),
}

impl From<rusqlite::Error> for RecordKeeperError {
	#[track_caller]
	fn from(err: rusqlite::Error) -> Self {
		Self::Rusqlite(err, std::panic::Location::caller())
	}
}

impl From<rusqlite::types::FromSqlError> for RecordKeeperError {
	#[track_caller]
	fn from(err: rusqlite::types::FromSqlError) -> Self {
		Self::RusqliteFromSql(err, std::panic::Location::caller())
	}
}

#[derive(Deserialize)]
pub struct QueryThumbnail {
	pub book_path: PathBuf,
	#[serde(with = "ts_seconds")]
	pub book_modified_at: DateTime<Utc>,
	pub path: Option<PathBuf>,
	#[serde(with = "ts_seconds_option")]
	pub added_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct SecretBook {
	id: i64,
	path: PathBuf,
	title: Option<String>,
	author: Option<String>,
	size: u64,
	#[serde(with = "ts_seconds")]
	modified_at: DateTime<Utc>,
	#[serde(with = "ts_seconds")]
	added_at: DateTime<Utc>,
	#[serde(with = "ts_seconds_option")]
	opened_at: Option<DateTime<Utc>>,
	spine: Option<u32>,
	element: Option<u32>,
	percent_read: Option<u32>,
}

impl From<SecretBook> for Book {
	fn from(value: SecretBook) -> Self {
		Book {
			id: BookId(value.id),
			path: value.path,
			title: value.title.map(Arc::new),
			author: value.author.map(Arc::new),
			size: value.size,
			modified_at: value.modified_at,
			added_at: value.added_at,
			opened_at: value.opened_at,
			spine: value.spine,
			element: value.element.map(U26F6::from_bits),
			percent_read: value.percent_read,
		}
	}
}

#[derive(Debug, Serialize)]
pub struct UpdateBook<'a> {
	pub book_id: i64,
	pub title: Option<&'a str>,
	pub author: Option<&'a str>,
	#[serde(with = "ts_seconds")]
	pub modified_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct InsertBook {
	pub path: PathBuf,
	pub title: Option<String>,
	pub author: Option<String>,
	pub size: u64,
	#[serde(with = "ts_seconds")]
	pub modified_at: DateTime<Utc>,
	#[serde(with = "ts_seconds")]
	pub added_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct InsertThumbnail<'a> {
	pub book_id: i64,
	pub path: Option<&'a Path>,
	#[serde(with = "ts_seconds")]
	pub added_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct InsertBookState {
	pub book_id: i64,
	#[serde(with = "ts_seconds")]
	pub opened_at: DateTime<Utc>,
	pub spine: u32,
	pub element: u32,
	pub percent_read: u32,
}

pub struct RecordKeeperAssistant {
	conn: rusqlite::Connection,
}

#[derive(Clone)]
pub struct RecordKeeper {
	db_path: Arc<PathBuf>,
}

impl RecordKeeper {
	pub fn new(data_path: &Path) -> Self {
		let db_path = data_path.join("state.db");
		Self {
			db_path: Arc::new(db_path),
		}
	}

	pub fn assistant(&self) -> Result<RecordKeeperAssistant, RecordKeeperError> {
		let mut conn = rusqlite::Connection::open(self.db_path.as_ref())?;

		conn.pragma_update(None, "foreign_keys", "on")?;
		conn.pragma_update(None, "journal_mode", "WAL")?;

		static ONCE: OnceLock<()> = OnceLock::new();
		ONCE.get_or_init(|| MIGRATIONS.to_latest(&mut conn).unwrap());

		Ok(RecordKeeperAssistant { conn })
	}
}

impl RecordKeeperAssistant {
	pub fn fetch_book(&self, id: BookId) -> Result<Book, RecordKeeperError> {
		let mut stmt = self.conn.prepare(
			"select
				bo.id,
				bo.path,
				bo.title,
				bo.author,
				bo.size,
				bo.modified_at,
				bo.added_at,
				bs.opened_at,
				bs.spine,
				bs.element,
				bs.percent_read
			from books bo
			left join book_reading_state bs on bs.book_id = bo.id
			where bo.exist = true
				and id = ?1;
			",
		)?;
		Ok(stmt
			.query_one([id.into_inner()], |row| Ok(from_row::<SecretBook>(row)))??
			.into())
	}

	pub fn fetch_books(&self) -> Result<BTreeMap<BookId, Book>, RecordKeeperError> {
		let mut stmt = self.conn.prepare(
			"select
				bo.id,
				bo.path,
				bo.title,
				bo.author,
				bo.size,
				bo.modified_at,
				bo.added_at,
				bs.opened_at,
				bs.spine,
				bs.element,
				bs.percent_read
			from books bo
			left join book_reading_state bs on bs.book_id = bo.id
			where bo.exist = true
			",
		)?;
		Ok(from_rows::<SecretBook>(stmt.query([])?)
			.map(|b| b.map(|b| (BookId(b.id), b.into())))
			.collect::<Result<_, _>>()?)
	}

	pub fn upsert_book(&mut self, book: InsertBook) -> Result<BookId, RecordKeeperError> {
		let mut insert_stmt = self.conn.prepare(
			"insert into books (path, title, author, size, modified_at, added_at, exist)
				values (:path, :title, :author, :size, :modified_at, :added_at, true)
			on conflict (path)
			do update set
				title = :title,
				author = :author,
				size = :size,
				modified_at = :modified_at,
				exist = true
			returning books.id",
		)?;
		let params = to_params_named(&book)?;
		let params = params.to_slice();
		let book_id: i64 = insert_stmt.query_one(params.as_slice(), |row| row.get(0))?;

		Ok(BookId(book_id))
	}

	pub fn unexist_books<'a>(
		&mut self,
		book_ids: impl IntoIterator<Item = &'a BookId>,
	) -> Result<(), RecordKeeperError> {
		let tx = self.conn.transaction()?;
		let mut unexist_stmt = tx.prepare("update books set exist = false where id = ?1")?;
		for id in book_ids {
			log::info!("Unexist book {:?}", id);
			unexist_stmt.execute([id.into_inner()])?;
		}
		drop(unexist_stmt);
		tx.commit()?;
		Ok(())
	}

	pub fn fetch_thumbnail(
		&self,
		id: super::BookId,
	) -> Result<Option<QueryThumbnail>, RecordKeeperError> {
		let mut stmt = self.conn.prepare(
			"select
				bo.path as book_path,
				bo.modified_at as book_modified_at,
				th.path,
				th.added_at
			from books bo
			left join book_cache_thumbnails th on th.book_id = bo.id
			where id = ?1;
			",
		)?;
		from_rows::<QueryThumbnail>(stmt.query([id.into_inner()])?)
			.next()
			.transpose()
			.map_err(|e| e.into())
	}

	pub fn update_book(&mut self, book: UpdateBook) -> Result<(), RecordKeeperError> {
		let mut stmt = self.conn.prepare(
			"update books set
				title = :title,
				author = :author,
				modified_at = :modified_at,
				exist = true
			where id = :book_id;
			",
		)?;
		stmt.execute(to_params_named(book)?.to_slice().as_slice())?;
		Ok(())
	}

	pub fn record_thumbnail(
		&mut self,
		id: super::BookId,
		path: Option<&Path>,
	) -> Result<(), RecordKeeperError> {
		let mut stmt = self.conn.prepare(
			"insert into book_cache_thumbnails (book_id, path, added_at)
				values (:book_id, :path, :added_at)
			on conflict (book_id)
			do update set
				path = :path,
				added_at = :added_at;
			",
		)?;
		let thumbnail = InsertThumbnail {
			book_id: id.into_inner(),
			added_at: Utc::now(),
			path,
		};
		stmt.execute(to_params_named(thumbnail)?.to_slice().as_slice())?;
		Ok(())
	}

	pub fn record_book_state(
		&self,
		id: super::BookId,
		loc: Location,
		percent_read: u32,
	) -> Result<(), RecordKeeperError> {
		let mut stmt = self.conn.prepare(
			"insert into book_reading_state (book_id, opened_at, spine, element, percent_read)
				values (:book_id, :opened_at, :spine, :element, :percent_read)
			on conflict (book_id)
			do update set
				opened_at = :opened_at,
				spine = :spine,
				element = :element,
				percent_read = :percent_read;
			",
		)?;
		let state = InsertBookState {
			book_id: id.into_inner(),
			opened_at: Utc::now(),
			spine: loc.spine,
			element: loc.element.to_bits(),
			percent_read,
		};
		stmt.execute(to_params_named(state)?.to_slice().as_slice())?;
		Ok(())
	}
}
