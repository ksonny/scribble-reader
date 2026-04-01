use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use fixed::types::U26F6;

#[allow(dead_code)]
#[derive(Debug, Default, Clone, Copy)]
pub enum SortField {
	#[default]
	Added,
	Modified,
	Title,
}

#[allow(dead_code)]
#[derive(Debug, Default, Clone, Copy)]
pub enum SortDirection {
	#[default]
	Ascending,
	Descending,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SortOrder(pub SortField, pub SortDirection);

#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Copy)]
pub struct BookId(pub i64);

impl Display for BookId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "[b{}]", self.0)
	}
}

impl BookId {
	pub fn value(&self) -> i64 {
		let BookId(id) = self;
		*id
	}
}

#[derive(Debug, Clone, Copy)]
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

#[allow(dead_code)]
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
