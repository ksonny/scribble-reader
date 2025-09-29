use chrono::DateTime;
use chrono::Utc;
use chrono::serde::ts_seconds;
use chrono::serde::ts_seconds_option;
use image::ImageReader;
use image::codecs::png;
use image::codecs::png::PngEncoder;
use rbook::Ebook;
use rbook::ebook::manifest::Manifest;
use rbook::ebook::manifest::ManifestEntry;
use rbook::ebook::metadata::MetaEntry;
use rbook::ebook::metadata::Metadata;
use rusqlite_migration::M;
use rusqlite_migration::Migrations;
use serde::Deserialize;
use serde::Serialize;
use serde_rusqlite::from_rows;
use serde_rusqlite::to_params_named;

use std::collections::BTreeMap;
use std::fs;
use std::io;
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
	Ebook(#[from] rbook::ebook::errors::EbookError),
	#[error(transparent)]
	Image(#[from] image::ImageError),
	#[error("Scan path is not directory")]
	ScanPathNotDir,
	#[error("Book with id not found: {0:?}")]
	BookNotFound(library::BookId),
	#[error("Unsupported cover mime: {0:?}")]
	UnsupportedCoverMime(String),
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
];
const MIGRATIONS: Migrations<'_> = Migrations::from_slice(MIGRATIONS_SLICE);

pub fn create(db_path: &Path, cache_path: &Path) -> Result<SecretStorage, SecretStorageError> {
	let mut conn = rusqlite::Connection::open(db_path)?;

	conn.pragma_update(None, "foreign_keys", "on")?;
	conn.pragma_update(None, "journal_mode", "WAL")?;

	MIGRATIONS.to_latest(&mut conn).unwrap();

	let cache_path = cache_path.to_path_buf();

	Ok(SecretStorage { conn, cache_path })
}

#[derive(Debug, Serialize)]
struct InsertBook<'a> {
	path: PathBuf,
	title: Option<&'a str>,
	author: Option<&'a str>,
	size: u64,
	#[serde(with = "ts_seconds")]
	modified_at: DateTime<Utc>,
	#[serde(with = "ts_seconds")]
	added_at: DateTime<Utc>,
}

impl From<(i64, InsertBook<'_>)> for library::Book {
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
			title: title.map(|s| Arc::new(s.to_string())),
			author: author.map(|s| Arc::new(s.to_string())),
			size,
			modified_at,
			added_at,
		}
	}
}

pub struct SecretThumbnail(pub Vec<u8>);

impl From<Option<SecretThumbnail>> for library::Thumbnail {
	fn from(value: Option<SecretThumbnail>) -> Self {
		match value {
			Some(SecretThumbnail(bytes)) => library::Thumbnail::Bytes {
				bytes: bytes.into(),
			},
			None => library::Thumbnail::None,
		}
	}
}

#[derive(Deserialize)]
struct QueryThumbnail {
	book_path: PathBuf,
	#[serde(with = "ts_seconds")]
	book_modified_at: DateTime<Utc>,
	path: Option<PathBuf>,
	#[serde(with = "ts_seconds_option")]
	added_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct InsertThumbnail<'a> {
	book_id: i64,
	path: Option<&'a Path>,
	#[serde(with = "ts_seconds")]
	added_at: DateTime<Utc>,
}

pub struct SecretStorage {
	conn: rusqlite::Connection,
	cache_path: PathBuf,
}

impl SecretStorage {
	const THUMBNAIL_SIZE: f32 = 320.0;

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
				match scan_book(&mut upsert_stmt, &entry) {
					Ok((id, book)) => {
						books.insert(id, book);
					}
					Err(e) => {
						log::error!("Failed to scan file '{}': {e}", entry.file_name().display());
					}
				}
			}
		}
		log::trace!("Total {} books", books.len());

		tx.commit()?;
		Ok(books)
	}

	pub fn load_thumbnail(
		&mut self,
		id: library::BookId,
	) -> Result<Option<SecretThumbnail>, SecretStorageError> {
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
		let result = from_rows::<QueryThumbnail>(stmt.query([id.value()])?)
			.next()
			.transpose()?;
		let Some(result) = result else {
			return Err(SecretStorageError::BookNotFound(id));
		};

		let upsert_stmt = self.conn.prepare(
			"insert into book_cache_thumbnails (book_id, path, added_at)
			values (:book_id, :path, :added_at)
			on conflict (book_id)
			do update set
				path = :path,
				added_at = :added_at;
			",
		)?;
		if result.added_at.is_none_or(|t| t < result.book_modified_at) {
			let thumb_path = format!("thumbnails/thumbnail_{}.png", id.value());
			let thumbnail_path = self.cache_path.join(&thumb_path);
			create_thumbnail(id, &result.book_path, &thumbnail_path, upsert_stmt)
		} else if let Some(path) = result.path {
			match fs::read(&path) {
				Ok(bytes) => Ok(Some(SecretThumbnail(bytes))),
				Err(e) => {
					log::warn!(
						"Failed to load thumbnail at {}, try regenrate: {e}",
						path.display()
					);
					let thumb_path = format!("thumbnails/thumbnail_{}.png", id.value());
					let thumbnail_path = self.cache_path.join(&thumb_path);
					create_thumbnail(id, &result.book_path, &thumbnail_path, upsert_stmt)
				}
			}
		} else {
			Ok(None)
		}
	}
}

fn create_thumbnail(
	id: super::BookId,
	book_path: &Path,
	thumbnail_path: &Path,
	mut upsert_stmt: rusqlite::Statement<'_>,
) -> Result<Option<SecretThumbnail>, SecretStorageError> {
	let epub = rbook::Epub::open(book_path)?;
	let epub_manifest = epub.manifest();
	let cover_image = epub_manifest.cover_image().or_else(|| {
		epub_manifest
			.images()
			.find(|i| i.key() == Some("cover-image"))
	});
	let Some(cover_image) = cover_image else {
		log::trace!("No cover image for {:?}", id);

		upsert_stmt.execute(
			to_params_named(InsertThumbnail {
				book_id: id.value(),
				path: None,
				added_at: Utc::now(),
			})?
			.to_slice()
			.as_slice(),
		)?;

		return Ok(None);
	};

	log::trace!("Found cover image for {:?}", id);
	let resource_kind = cover_image.resource_kind();
	if !matches!(resource_kind.as_str(), "image/png" | "image/jpeg") {
		return Err(SecretStorageError::UnsupportedCoverMime(
			resource_kind.to_string(),
		));
	}

	let cover_bytes = cover_image.read_bytes()?;
	let max_size = SecretStorage::THUMBNAIL_SIZE;
	let cover_bytes = cover_bytes.as_slice();
	let img = ImageReader::new(io::Cursor::new(cover_bytes))
		.with_guessed_format()?
		.decode()?;
	let img = img.resize(
		max_size as u32,
		max_size as u32,
		image::imageops::FilterType::CatmullRom,
	);

	log::trace!("Encode image as png");
	let mut bytes = Vec::new();
	let encoder = PngEncoder::new_with_quality(
		&mut bytes,
		png::CompressionType::Fast,
		png::FilterType::default(),
	);
	img.write_with_encoder(encoder)?;
	log::trace!("Write thumbnail at {}", thumbnail_path.display());
	fs::write(thumbnail_path, bytes.as_slice())?;

	upsert_stmt.execute(
		to_params_named(InsertThumbnail {
			book_id: id.value(),
			path: Some(thumbnail_path),
			added_at: Utc::now(),
		})?
		.to_slice()
		.as_slice(),
	)?;

	Ok(Some(SecretThumbnail(bytes)))
}

fn scan_book(
	upsert_stmt: &mut rusqlite::Statement<'_>,
	entry: &fs::DirEntry,
) -> Result<(library::BookId, library::Book), SecretStorageError> {
	let path = entry.path();
	let epub = rbook::Epub::open(&path)?;
	let epub_metadata = epub.metadata();
	let title = epub_metadata.title().map(|t| t.value());
	let author = epub_metadata.creators().next().map(|c| c.value());
	let entry_metadata = entry.metadata()?;
	let size = entry_metadata.size();
	let modified_at = entry_metadata
		.modified()
		.or_else(|_| entry_metadata.created())?
		.into();
	let added_at = Utc::now();
	let book = InsertBook {
		path,
		title,
		author,
		size,
		modified_at,
		added_at,
	};
	log::trace!(
		"Found {} by {}",
		book.title.unwrap_or("Unknown"),
		book.author.unwrap_or("Unknown")
	);
	let params = to_params_named(&book)?;
	let params = params.to_slice();
	let book_id = upsert_stmt.query_row(params.as_slice(), |row| row.get(0))?;
	Ok((library::BookId(book_id), (book_id, book).into()))
}
