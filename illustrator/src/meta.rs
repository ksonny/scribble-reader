use std::collections::BTreeMap;
use std::io;
use std::io::Cursor;
use std::io::Read;
use std::io::Seek;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use epub::doc::EpubDoc;
use scribe::library::Location;
use serde::Deserialize;
use zip::ZipArchive;

use crate::SharedVec;
use crate::html_parser::NodeTreeBuilder;
use crate::html_parser::TreeBuilderError;

#[derive(Debug, thiserror::Error)]
pub enum IllustratorBookMetaError {
	#[error(transparent)]
	TreeBuilder(#[from] TreeBuilderError),
	#[error(transparent)]
	Zip(#[from] zip::result::ZipError),
	#[error("epub error: {0}")]
	Epub(#[from] epub::doc::DocError),
	#[error("Missing resource {0}")]
	MissingResource(String),
}

#[derive(Debug)]
pub(crate) struct BookResource {
	pub(crate) path: PathBuf,
	#[allow(unused)]
	pub(crate) mime: mime::Mime,
}

impl BookResource {
	fn new(path: PathBuf, mime: &str) -> Self {
		Self {
			path,
			mime: mime.parse().unwrap_or(mime::TEXT_HTML),
		}
	}
}

#[derive(Debug, Deserialize)]
pub(crate) struct NavLabel {
	text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Content {
	#[serde(rename = "@src")]
	src: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NavPoint {
	#[serde(rename = "@id")]
	id: String,
	#[serde(rename = "navLabel")]
	nav_label: NavLabel,
	#[serde(rename = "content")]
	content: Content,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NavMap {
	#[serde(rename = "navPoint")]
	nav_points: Vec<NavPoint>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Navigation {
	#[serde(rename = "navMap")]
	nav_map: NavMap,
}

fn read_navigation(s: &str) -> Result<Navigation, quick_xml::de::DeError> {
	quick_xml::de::from_str(s)
}

pub struct IllustratorToCItem {
	pub title: Arc<String>,
	pub location: Location,
}

#[derive(Default)]
pub struct IllustratorToC {
	pub items: Vec<IllustratorToCItem>,
}

impl Navigation {
	fn into_toc(self, spine: &[BookSpineItem]) -> IllustratorToC {
		let spine_lookup = spine
			.iter()
			.enumerate()
			.map(|(i, s)| (&s.idref, i))
			.collect::<BTreeMap<_, _>>();
		let mut items = Vec::new();
		for nav_point in &self.nav_map.nav_points {
			if let Some(spine) = spine_lookup.get(&nav_point.content.src) {
				items.push(IllustratorToCItem {
					title: Arc::new(nav_point.nav_label.text.clone()),
					location: Location {
						spine: *spine as u32,
						element: 0,
					},
				});
			} else {
				log::warn!(
					"Failed to find {} {} in spine",
					nav_point.id,
					nav_point.nav_label.text
				);
			}
		}
		IllustratorToC { items }
	}
}

#[derive(Debug)]
pub(crate) struct BookSpineItem {
	pub(crate) index: u32,
	pub(crate) idref: String,
	pub(crate) elements: Range<u32>,
}

#[allow(unused)]
#[derive(Debug)]
pub(crate) struct BookMeta {
	pub(crate) resources: BTreeMap<String, BookResource>,
	pub(crate) spine: Vec<BookSpineItem>,
	pub(crate) cover_id: Option<String>,
}

impl BookMeta {
	fn create<R: Seek + Read>(
		epub: EpubDoc<R>,
		archive: &mut ZipArchive<R>,
	) -> Result<Self, IllustratorBookMetaError> {
		let start = Instant::now();
		let EpubDoc {
			resources,
			spine,
			metadata,
			..
		} = epub;
		let cover_id = metadata
			.iter()
			.find(|data| data.property == "cover")
			.map(|m| m.value.clone());
		let title = metadata
			.iter()
			.find(|data| data.property == "title")
			.map(|m| m.value.clone());

		let resources = resources
			.into_iter()
			.map(|(key, res)| (key, BookResource::new(res.path, &res.mime)))
			.collect::<BTreeMap<_, _>>();
		let spine = {
			let mut builder = NodeTreeBuilder::new();
			let mut items = Vec::new();
			for (index, item) in spine.into_iter().enumerate() {
				let res = resources
					.get(&item.idref)
					.ok_or_else(|| IllustratorBookMetaError::MissingResource(item.idref.clone()))?;
				let file = archive.by_path(&res.path)?;
				let tree = builder.read_from(file)?;
				let node_count = tree.tree.node_count();
				items.push(BookSpineItem {
					index: index as u32,
					idref: item.idref,
					elements: 0..node_count,
				});
				builder = tree.into_builder();
			}
			items
		};
		let dur = Instant::now().duration_since(start);
		log::info!("Opened {:?} in {}", title, dur.as_secs_f64());

		Ok(Self {
			resources,
			spine,
			cover_id,
		})
	}

	pub(crate) fn spine_resource(&self, loc: Location) -> Option<(&BookSpineItem, &Path)> {
		self.spine
			.get(loc.spine as usize)
			.and_then(|s| self.resources.get(&s.idref).map(|r| (s, r.path.as_path())))
	}
}

pub(crate) fn read_book_meta(
	bytes: SharedVec,
	archive: &mut ZipArchive<io::Cursor<SharedVec>>,
) -> Result<(BookMeta, Option<IllustratorToC>), IllustratorBookMetaError> {
	let doc = EpubDoc::from_reader(Cursor::new(bytes.clone()))
		.inspect_err(|e| log::error!("Error: {e}"))?;
	let book_meta = BookMeta::create(doc, archive)?;
	let toc = book_meta.resources.get("ncx").and_then(|res| {
		let mut buf = String::new();
		archive
			.by_path(&res.path)
			.inspect_err(|e| log::error!("Error: {e}"))
			.ok()?
			.read_to_string(&mut buf)
			.inspect_err(|e| log::error!("Error: {e}"))
			.ok()?;
		let nav = read_navigation(&buf)
			.inspect_err(|e| log::error!("Error: {e}"))
			.ok()?;
		Some(nav.into_toc(&book_meta.spine))
	});
	Ok((book_meta, toc))
}

#[cfg(test)]
mod tests {
	use quick_xml::de::from_str;

	#[test]
	fn test_parse_basic() -> Result<(), quick_xml::de::DeError> {
		let input = r#"
<?xml version="1.0" encoding="UTF-8"?>
<ncx version="2005-1" xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <head>
    <meta name="dtb:depth" content="1" />
    <meta name="dtb:totalPageCount" content="0" />
    <meta name="dtb:maxPageNumber" content="0" />
  </head>
  <docTitle>
    <text>Table Of Contents</text>
  </docTitle>
  <navMap>
    <navPoint id="navPoint-1">
      <navLabel>
       <text>Cover</text>
      </navLabel>
      <content src="cover.xhtml"/>
    </navPoint>
  </navMap>
</ncx>"#;

		let _nav_map: super::Navigation = from_str(input)?;
		Ok(())
	}
}
