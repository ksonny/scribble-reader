#![allow(dead_code)]
mod html_parser;

use std::collections::BTreeMap;
use std::fmt::Display;
use std::fs;
use std::io;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::mpsc::RecvError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread::JoinHandle;
use std::time::Instant;

use epub::doc::EpubDoc;
use epub::doc::NavPoint;
use fixed::types::I26F6;
use image::DynamicImage;
use scribe::library;
use scribe::record_keeper::RecordKeeper;
use zip::ZipArchive;

use crate::html_parser::EdgeRef;
use crate::html_parser::NodeTreeBuilder;
use crate::html_parser::Text;

#[derive(Debug, thiserror::Error)]
pub enum IllustratorError {
	#[error(transparent)]
	RecordKeeper(#[from] scribe::record_keeper::RecordKeeperError),
	#[error(transparent)]
	TreeBuilder(#[from] html_parser::TreeBuilderError),
	#[error(transparent)]
	Epub(#[from] epub::doc::DocError),
	#[error(transparent)]
	Zip(#[from] zip::result::ZipError),
	#[error("at {1}: {0}")]
	Io(std::io::Error, &'static std::panic::Location<'static>),
}

impl From<std::io::Error> for IllustratorError {
	#[track_caller]
	fn from(err: std::io::Error) -> Self {
		Self::Io(err, std::panic::Location::caller())
	}
}

#[derive(Default)]
struct SecretIllustrator {
	dpi: I26F6,
	pixels: DynamicImage,
}

pub struct Illustrator {
	handle: JoinHandle<Result<(), IllustratorError>>,
	state: Arc<RwLock<SecretIllustrator>>,
	req_tx: Sender<Request>,
}

#[derive(Debug)]
pub enum Location {
	Word(u64),
}

impl Display for Location {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Location::Word(word) => write!(f, "[Word {word}]"),
		}
	}
}

pub enum Request {
	Goto(Location),
}

enum TaskResult {
	Done,
}

#[derive(Clone)]
struct SharedVec(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedVec {
	fn as_ref(&self) -> &[u8] {
		let Self(data) = self;
		data
	}
}

struct BookResource {
	path: PathBuf,
	mime: mime::Mime,
}

impl BookResource {
	fn new(path: PathBuf, mime: &str) -> Self {
		Self {
			path,
			mime: mime.parse().unwrap_or(mime::TEXT_HTML),
		}
	}

	fn words<R: io::Seek + io::Read>(
		&self,
		archive: &mut ZipArchive<R>,
	) -> Result<u64, IllustratorError> {
		let file = archive.by_path(&self.path)?;
		let tree = NodeTreeBuilder::create()?.read_from(file)?;
		let mut words = 0;
		for node in tree.nodes() {
			if let EdgeRef::Text(Text { t }) = node {
				words += count_words(t);
			}
		}
		Ok(words)
	}
}

pub fn count_words(input: &str) -> u64 {
	let mut words = 0;
	let mut in_word = false;
	for b in input.as_bytes() {
		if matches!(*b, b'\t' | b'\n' | b' ') {
			if in_word {
				in_word = false;
				words += 1;
			}
		} else if !in_word {
			in_word = true;
		}
	}
	words
}

struct BookToCItem {
	label: String,
	content: PathBuf,
	play_order: usize,
}

struct BookSpineItem {
	idref: String,
	words: u64,
}

struct BookMeta {
	resources: BTreeMap<String, BookResource>,
	spine: Vec<BookSpineItem>,
	toc: Vec<BookToCItem>,
	toc_title: String,
	cover_id: Option<String>,
	words: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum FromEpubError {}

impl BookMeta {
	fn create<R: io::Seek + io::Read>(
		epub: EpubDoc<R>,
		archive: &mut ZipArchive<R>,
	) -> Result<Self, IllustratorError> {
		let start = Instant::now();
		let EpubDoc {
			resources,
			spine,
			toc,
			toc_title,
			cover_id,
			metadata,
			..
		} = epub;
		let resources = resources
			.into_iter()
			.map(|(key, (path, mime))| (key, BookResource::new(path, &mime)))
			.collect::<BTreeMap<_, _>>();
		let toc = {
			let mut items = Vec::new();
			convert_toc(&mut items, toc.into_iter());
			items
		};
		let spine = spine
			.into_iter()
			.map(|item| {
				let res = resources.get(&item.idref);
				let words = res.map(|r| r.words(archive)).unwrap_or(Ok(0))?;
				Ok(BookSpineItem {
					idref: item.idref,
					words,
				})
			})
			.collect::<Result<Vec<_>, IllustratorError>>()?;
		let words = spine.iter().map(|s| s.words).sum();
		let dur = Instant::now().duration_since(start);

		log::info!(
			"Opened {:?} in {}",
			metadata.get("title"),
			dur.as_secs_f64()
		);

		Ok(Self {
			resources,
			spine,
			toc_title,
			toc,
			cover_id,
			words,
		})
	}
}

fn convert_toc(toc_items: &mut Vec<BookToCItem>, iter: impl Iterator<Item = NavPoint>) {
	for item in iter {
		let NavPoint {
			label,
			content,
			children,
			play_order,
		} = item;
		toc_items.push(BookToCItem {
			label,
			content,
			play_order,
		});
		convert_toc(toc_items, children.into_iter());
	}
}

pub fn spawn_illustrator(records: RecordKeeper, id: library::BookId) -> Illustrator {
	let (req_tx, req_rx) = channel();

	let handle = std::thread::spawn(move || {
		log::info!("Open book {id:?}");
		let book = records
			.fetch_book(id)
			.inspect_err(|e| log::error!("Error: {e}"))?;
		let bytes = SharedVec(Arc::new(
			fs::read(&book.path).inspect_err(|e| log::error!("Error: {e}"))?,
		));
		let doc = EpubDoc::from_reader(Cursor::new(bytes.clone()))
			.inspect_err(|e| log::error!("Error: {e}"))?;
		let mut archive =
			ZipArchive::new(Cursor::new(bytes)).inspect_err(|e| log::error!("Error: {e}"))?;
		let book_meta =
			BookMeta::create(doc, &mut archive).inspect_err(|e| log::error!("Error: {e}"))?;

		log::info!(
			"Opened book with {} words, {} resources, {} spine items",
			book_meta.words,
			book_meta.resources.len(),
			book_meta.spine.len()
		);

		loop {
			let req = req_rx.recv();
			match req {
				Ok(Request::Goto(loc)) => {
					log::trace!("Goto {loc}");
				}
				Err(RecvError) => {
					log::info!("Illustrator worker terminated");
					break Ok(());
				}
			}
		}
	});

	Illustrator {
		handle,
		state: Default::default(),
		req_tx,
	}
}
