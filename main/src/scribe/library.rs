use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use chrono::DateTime;
use chrono::Utc;

use crate::scribe::library;

#[allow(dead_code)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) enum SortField {
	#[default]
	Added,
	Modified,
	Title,
}

#[allow(dead_code)]
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

impl BookId {
	pub fn value(&self) -> i64 {
		let BookId(id) = self;
		*id
	}
}

#[derive(Debug, Clone)]
pub struct Book {
	pub id: BookId,
	pub path: PathBuf,
	pub title: Option<Arc<String>>,
	pub author: Option<Arc<String>>,
	pub size: u64,
	pub modified_at: DateTime<Utc>,
	pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum Thumbnail {
	None,
	Bytes {
		bytes: Arc<[u8]>,
	},
}

#[derive(Debug, Default)]
pub(crate) struct SecretLibrary {
	pub(crate) books: BTreeMap<BookId, Book>,
	pub(crate) order: SortOrder,
	pub(crate) sorted: Vec<BookId>,
	pub(crate) thumbnails: BTreeMap<BookId, Thumbnail>,
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
		log::trace!(
			"Requested books {start}..{end}, received {} from all books {}",
			books.len(),
			lib.sorted.len()
		);
		books
	}

	pub fn thumbnail(&self, id: BookId) -> Option<Thumbnail> {
		let lib = self.read().unwrap();
		lib.thumbnails.get(&id).cloned()
	}
}
