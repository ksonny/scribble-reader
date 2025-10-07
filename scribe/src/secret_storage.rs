use chrono::Utc;
use image::ImageReader;
use image::codecs::png;
use image::codecs::png::PngEncoder;
use rbook::Ebook;
use rbook::ebook::manifest::Manifest;
use rbook::ebook::manifest::ManifestEntry;
use rbook::ebook::metadata::MetaEntry;
use rbook::ebook::metadata::Metadata;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

use crate::library;
use crate::record_keeper::InsertBook;
use crate::record_keeper::RecordKeeper;
use crate::record_keeper::RecordKeeperError;

#[derive(Debug, thiserror::Error)]
pub enum SecretStorageError {
	#[error(transparent)]
	RecordKeeper(#[from] RecordKeeperError),
	#[error("at {1}: {0}")]
	Io(std::io::Error, &'static std::panic::Location<'static>),
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

impl From<std::io::Error> for SecretStorageError {
	#[track_caller]
	fn from(err: std::io::Error) -> Self {
		Self::Io(err, std::panic::Location::caller())
	}
}

pub fn create(cache_path: &Path) -> Result<SecretStorage, SecretStorageError> {
	let cache_path = cache_path.to_path_buf();
	let thumbnail_path = cache_path.join("thumbnails");
	if !thumbnail_path.try_exists()? {
		fs::create_dir(&thumbnail_path)?;
	}

	Ok(SecretStorage { cache_path })
}

pub struct SecretStorage {
	cache_path: PathBuf,
}

impl SecretStorage {
	const THUMBNAIL_SIZE: f32 = 320.0;

	pub fn scan(
		&self,
		records: &mut RecordKeeper,
		path: &Path,
	) -> Result<BTreeMap<library::BookId, library::Book>, SecretStorageError> {
		if !path.is_dir() {
			return Err(SecretStorageError::ScanPathNotDir);
		}

		let books = fs::read_dir(path)?.filter_map(|entry| {
			let entry = entry.ok()?;
			scan_book(&entry)
				.inspect_err(|e| {
					log::error!("Failed to scan file '{}': {e}", entry.file_name().display())
				})
				.ok()
		});
		let books = records.record_book_inventory(books)?;
		log::trace!("Total {} books", books.len());

		Ok(books)
	}

	pub fn load_thumbnail(
		&self,
		records: &mut RecordKeeper,
		id: library::BookId,
	) -> Result<Option<Vec<u8>>, SecretStorageError> {
		let result = records.fetch_thumbnail(id)?;
		let Some(result) = result else {
			return Err(SecretStorageError::BookNotFound(id));
		};

		if result.added_at.is_none_or(|t| t < result.book_modified_at) {
			if let Some((path, bytes)) = create_thumbnail(id, &result.book_path, &self.cache_path)?
			{
				records.record_thumbnail(id, Some(&path))?;
				Ok(Some(bytes))
			} else {
				records.record_thumbnail(id, None)?;
				Ok(None)
			}
		} else if let Some(path) = result.path {
			match fs::read(&path) {
				Ok(bytes) => Ok(Some(bytes)),
				Err(e) => {
					log::warn!(
						"Failed to load thumbnail at {}, try regenrate: {e}",
						path.display()
					);
					if let Some((path, bytes)) =
						create_thumbnail(id, &result.book_path, &self.cache_path)?
					{
						records.record_thumbnail(id, Some(&path))?;
						Ok(Some(bytes))
					} else {
						records.record_thumbnail(id, None)?;
						Ok(None)
					}
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
	cache_path: &Path,
) -> Result<Option<(PathBuf, Vec<u8>)>, SecretStorageError> {
	let epub = rbook::Epub::open(book_path)?;
	let epub_manifest = epub.manifest();
	let cover_image = epub_manifest.cover_image().or_else(|| {
		epub_manifest
			.images()
			.find(|i| i.key() == Some("cover-image"))
	});
	let Some(cover_image) = cover_image else {
		log::trace!("No cover image for {:?}", id);
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

	let thumb_path = format!("thumbnails/thumbnail_{}.png", id.value());
	let thumbnail_path = cache_path.join(&thumb_path);
	log::trace!("Write thumbnail at {}", thumbnail_path.display());

	fs::write(&thumbnail_path, bytes.as_slice())?;

	Ok(Some((thumbnail_path, bytes)))
}

fn scan_book(entry: &fs::DirEntry) -> Result<InsertBook, SecretStorageError> {
	let path = entry.path();
	let epub = rbook::Epub::open(&path)?;
	let epub_metadata = epub.metadata();
	let title = epub_metadata.title().map(|t| t.value().to_string());
	let author = epub_metadata
		.creators()
		.next()
		.map(|c| c.value().to_string());
	let entry_metadata = entry.metadata()?;
	let size = entry_metadata.size();
	let modified_at = entry_metadata
		.modified()
		.or_else(|_| entry_metadata.created())?
		.into();
	let added_at = Utc::now();
	log::trace!(
		"Found {} by {}",
		title.as_deref().unwrap_or("Unknown"),
		author.as_deref().unwrap_or("Unknown")
	);
	Ok(InsertBook {
		path,
		title,
		author,
		size,
		modified_at,
		added_at,
	})
}
