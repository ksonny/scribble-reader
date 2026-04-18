use fixed::types::I26F6;
use scribe::Location;
use sculpter::AtlasImage;

use crate::PageContent;
use crate::PageFlags;

const CACHE_CHAPTERS: usize = 5;

#[derive(Debug, Default)]
struct PageCacheEntry {
	spine: u32,
	pages: Vec<PageContent>,
}

#[derive(Debug)]
pub struct PageContentCache {
	index: usize,
	entries: [Option<PageCacheEntry>; CACHE_CHAPTERS],
	atlas: AtlasImage,
}

impl Default for PageContentCache {
	fn default() -> Self {
		Self {
			index: 0,
			entries: [const { None }; CACHE_CHAPTERS],
			atlas: AtlasImage::default(),
		}
	}
}

pub struct PageMetadata {
	/// Page number, starting from 1
	pub page: u32,
	/// Total number of pages for chapter
	pub pages: u32,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum NavigateError {
	#[error("Must load next chapter for navigation")]
	LoadNextChapter,
	#[error("Must load previous chapter for navigation")]
	LoadPreviousChapter,
	#[error("Current chapter not cached")]
	CurrentChapterNotCached,
	#[error("Location is start of book")]
	StartOfBook,
}

impl PageContentCache {
	pub fn atlas(&self) -> &AtlasImage {
		&self.atlas
	}

	pub(crate) fn atlas_mut(&mut self) -> &mut AtlasImage {
		&mut self.atlas
	}

	pub fn page(&self, loc: Location) -> Option<(&PageContent, PageMetadata)> {
		self.entry(loc).map(|(_, page, meta)| (page, meta))
	}

	pub(crate) fn next_page(&self, loc: Location) -> Result<Location, NavigateError> {
		let Some((entry, page, _)) = self.entry(loc) else {
			return Err(NavigateError::CurrentChapterNotCached);
		};

		if page.flags.contains(PageFlags::Last) {
			let next_spine = loc.spine + 1;
			let entry = self
				.entries
				.iter()
				.flatten()
				.find(|e| e.spine == next_spine)
				.ok_or(NavigateError::LoadNextChapter)?;

			Ok(Location {
				spine: next_spine,
				element: entry
					.pages
					.first()
					.map(|p| p.elements.start)
					.unwrap_or_default(),
			})
		} else {
			let page = entry
				.pages
				.iter()
				.find(|p| p.elements.start > page.elements.start)
				.expect("Programmer error, not last page but nothing after");

			Ok(Location {
				spine: loc.spine,
				element: page.elements.start,
			})
		}
	}

	pub(crate) fn previous_page(&self, loc: Location) -> Result<Location, NavigateError> {
		let Some((entry, page, _)) = self.entry(loc) else {
			return Err(NavigateError::CurrentChapterNotCached);
		};
		if page.flags.contains(PageFlags::First) && loc.spine == 0 {
			return Err(NavigateError::StartOfBook);
		}

		if page.flags.contains(PageFlags::First) {
			let prev_spine = loc.spine.saturating_sub(1);
			let entry = self
				.entries
				.iter()
				.flatten()
				.find(|e| e.spine == prev_spine)
				.ok_or(NavigateError::LoadPreviousChapter)?;

			Ok(Location {
				spine: prev_spine,
				element: entry
					.pages
					.last()
					.map(|p| p.elements.start)
					.unwrap_or_default(),
			})
		} else {
			let page = entry
				.pages
				.iter()
				.rfind(|p| p.elements.start < page.elements.start)
				.expect("Programmer error, not first page but nothing before");

			Ok(Location {
				spine: loc.spine,
				element: page.elements.start,
			})
		}
	}

	pub(crate) fn is_cached(&self, loc: Location) -> bool {
		self.entries.iter().flatten().any(|e| e.spine == loc.spine)
	}

	pub(crate) fn insert(&mut self, spine_index: u32, pages: Vec<PageContent>) {
		#[cfg(debug_assertions)]
		if !pages.iter().is_sorted_by_key(|p| p.elements.start) {
			let starts = pages
				.iter()
				.map(|p| (p.elements.start, p.elements.end))
				.collect::<Vec<_>>();
			panic!("Pages in chapter not sorted {starts:?}");
		}

		self.entries[self.index % CACHE_CHAPTERS] = Some(PageCacheEntry {
			spine: spine_index,
			pages,
		});
		self.index += 1;
	}

	pub(crate) fn clear(&mut self) {
		self.entries = [const { None }; CACHE_CHAPTERS];
	}

	fn entry(&self, loc: Location) -> Option<(&PageCacheEntry, &PageContent, PageMetadata)> {
		let entry = self
			.entries
			.iter()
			.flatten()
			.find(|e| e.spine == loc.spine)?;

		let (page, meta) = if loc.element == I26F6::ZERO {
			let meta = PageMetadata {
				page: 1,
				pages: entry.pages.len() as u32,
			};
			(entry.pages.first()?, meta)
		} else {
			let (index, page) = entry
				.pages
				.iter()
				.enumerate()
				.find(|(_, p)| p.elements.contains(&loc.element))
				.or_else(|| entry.pages.last().map(|p| (entry.pages.len() - 1, p)))?;
			let meta = PageMetadata {
				page: index as u32 + 1,
				pages: entry.pages.len() as u32,
			};
			(page, meta)
		};
		Some((entry, page, meta))
	}
}
