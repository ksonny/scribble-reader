use fixed::types::I26F6;
use scribe::library::Location;
use sculpter::AtlasImage;

use crate::BookSpineItem;
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
	pub page: usize,
	/// Total number of pages for chapter
	pub pages: usize,
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

	pub(crate) fn next_page(&self, spine: &[BookSpineItem], loc: Location) -> Location {
		self.entry(loc)
			.map(|(entry, page, _)| {
				if page.flags.contains(PageFlags::Last) {
					spine
						.get(entry.spine as usize + 1)
						.map(|item| Location {
							spine: item.index,
							element: item.elements.start,
						})
						// End of book
						.unwrap_or(loc)
				} else {
					entry
						.pages
						.iter()
						.find(|p| p.elements.start > page.elements.start)
						.or(entry.pages.last())
						.map(|p| Location {
							spine: entry.spine,
							element: p.elements.start,
						})
						.expect("Programmer error, not last page but nothing after")
				}
			})
			.unwrap_or(loc)
	}

	pub(crate) fn previous_page(&self, spine: &[BookSpineItem], loc: Location) -> Location {
		self.entry(loc)
			.map(|(entry, page, _)| {
				if page.flags.contains(PageFlags::First) {
					spine
						.get(entry.spine.saturating_sub(1) as usize)
						.map(|item| Location {
							spine: item.index,
							element: item.elements.end,
						})
						// Start of book
						.unwrap_or(loc)
				} else {
					entry
						.pages
						.iter()
						.rfind(|p| p.elements.start < page.elements.start)
						.map(|p| Location {
							spine: entry.spine,
							element: p.elements.start,
						})
						.expect("Programmer error, not first page but nothing before")
				}
			})
			.unwrap_or(loc)
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
				pages: entry.pages.len(),
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
				page: index + 1,
				pages: entry.pages.len(),
			};
			(page, meta)
		};
		Some((entry, page, meta))
	}
}
