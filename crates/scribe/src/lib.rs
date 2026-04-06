pub mod config;
mod library;
mod records;

use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use fixed::types::U26F6;

pub use library::LibraryBell;
pub use library::LibraryScribe;
pub use library::LibraryScribeAssistant;
pub use records::RecordKeeper;
pub use records::RecordKeeperAssistant;
pub use records::RecordKeeperError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
	pub spine: u32,
	pub element: U26F6,
}

impl Location {
	pub fn from_spine(spine: u32) -> Self {
		Self {
			spine,
			element: U26F6::ZERO,
		}
	}
}

impl Display for Location {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "[sp{}_el{}]", self.spine, self.element)
	}
}

#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Copy)]
pub struct BookId(pub i64);

impl Display for BookId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "[b{}]", self.0)
	}
}

impl BookId {
	pub fn into_inner(self) -> i64 {
		let BookId(id) = self;
		id
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
	pub opened_at: Option<DateTime<Utc>>,
	pub percent_read: Option<u32>,
	pub spine: Option<u32>,
	pub element: Option<U26F6>,
}

impl Book {
	pub fn location(&self) -> Location {
		Location {
			spine: self.spine.unwrap_or(0),
			element: self.element.unwrap_or(U26F6::ZERO),
		}
	}
}

#[derive(Debug, Clone, Default)]
pub enum Thumbnail {
	#[default]
	None,
	Bytes {
		bytes: Arc<[u8]>,
	},
}
