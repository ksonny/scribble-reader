use chrono::DateTime;
use chrono::Utc;
use chrono::serde::ts_seconds;
use chrono::serde::ts_seconds_option;
use rusqlite_migration::M;
use rusqlite_migration::Migrations;
use serde::Deserialize;
use serde::Serialize;
use serde_rusqlite::from_rows;
use serde_rusqlite::to_params_named;

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::scribe::library;

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
			exist boolean not null
		);",
	),
	M::up(
		"create table book_cache_thumbnails (
			book_id integer primary key,
			path text,
			added_at integer not null,
			foreign key (book_id) references books(id)
				on update cascade
				on delete cascade
		);",
	),
	M::up(
		"create table book_reading_state (
			book_id integer primary key,
			opened_at integer,
			words_position integer,
			words_total integer not null,
			foreign key (book_id) references books(id)
				on update cascade
				on delete cascade
		);",
	),
];
const MIGRATIONS: Migrations<'_> = Migrations::from_slice(MIGRATIONS_SLICE);

#[derive(Debug, thiserror::Error)]
pub enum SecretRecordKeeperError {
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

impl From<rusqlite::Error> for SecretRecordKeeperError {
	#[track_caller]
	fn from(err: rusqlite::Error) -> Self {
		Self::Rusqlite(err, std::panic::Location::caller())
	}
}

impl From<rusqlite::types::FromSqlError> for SecretRecordKeeperError {
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
pub struct SecretBook {
	pub(crate) id: i64,
	pub(crate) path: PathBuf,
	pub(crate) title: Option<String>,
	pub(crate) author: Option<String>,
	pub(crate) size: u64,
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

#[derive(Debug, Serialize)]
pub struct InsertThumbnail {
	pub book_id: i64,
	pub path: Option<PathBuf>,
	#[serde(with = "ts_seconds")]
	pub added_at: DateTime<Utc>,
}

pub struct SecretRecordKeeper {
	conn: rusqlite::Connection,
}

pub fn create(db_path: &Path) -> Result<SecretRecordKeeper, SecretRecordKeeperError> {
	let mut conn = rusqlite::Connection::open(db_path)?;

	conn.pragma_update(None, "foreign_keys", "on")?;
	conn.pragma_update(None, "journal_mode", "WAL")?;

	MIGRATIONS.to_latest(&mut conn).unwrap();

	Ok(SecretRecordKeeper { conn })
}

impl SecretRecordKeeper {
	pub fn fetch_books(
		&self,
	) -> Result<BTreeMap<library::BookId, library::Book>, SecretRecordKeeperError> {
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
		Ok(from_rows::<SecretBook>(stmt.query([])?)
			.map(|b| b.map(|b| (library::BookId(b.id), b.into())))
			.collect::<Result<_, _>>()?)
	}

	pub fn record_book_inventory(
		&mut self,
		books_iter: impl Iterator<Item = InsertBook>,
	) -> Result<BTreeMap<library::BookId, library::Book>, SecretRecordKeeperError> {
		let tx = self.conn.transaction()?;
		tx.execute("update books set exist = false;", [])?;
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
		let mut map = BTreeMap::new();
		for book in books_iter {
			let params = to_params_named(&book)?;
			let params = params.to_slice();
			let book_id = upsert_stmt.query_row(params.as_slice(), |row| row.get(0))?;
			map.insert(
				library::BookId(book_id),
				Into::<library::Book>::into((book_id, book)),
			);
		}
		drop(upsert_stmt);
		tx.commit()?;
		Ok(map)
	}

	pub fn fetch_thumbnail(
		&self,
		id: super::BookId,
	) -> Result<Option<QueryThumbnail>, SecretRecordKeeperError> {
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
		from_rows::<QueryThumbnail>(stmt.query([id.value()])?)
			.next()
			.transpose()
			.map_err(|e| e.into())
	}

	pub fn record_thumbnail(
		&mut self,
		thumbnail: InsertThumbnail,
	) -> Result<(), SecretRecordKeeperError> {
		let mut upsert_stmt = self.conn.prepare(
			"insert into book_cache_thumbnails (book_id, path, added_at)
				values (:book_id, :path, :added_at)
				on conflict (book_id)
				do update set
					path = :path,
					added_at = :added_at;
				",
		)?;
		upsert_stmt.execute(to_params_named(thumbnail)?.to_slice().as_slice())?;
		Ok(())
	}
}
