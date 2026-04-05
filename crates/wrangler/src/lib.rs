pub mod content;

use std::fmt::Display;
use std::fs;
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread;
use std::thread::JoinHandle;
use std::time::SystemTime;

use expand_tilde::expand_tilde_owned;

#[derive(Debug, Clone)]
pub struct DocumentTree(Arc<String>);

impl DocumentTree {
	pub fn new(id: Arc<String>) -> Self {
		Self(id)
	}

	fn path(&self) -> &Path {
		let Self(id) = self;
		Path::new(id.as_ref())
	}

	pub fn into_inner(self) -> Arc<String> {
		let Self(id) = self;
		id
	}
}

impl AsRef<str> for DocumentTree {
	fn as_ref(&self) -> &str {
		let Self(id) = self;
		id.as_str()
	}
}

impl Display for DocumentTree {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.as_ref())
	}
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord)]
pub struct DocumentId(Arc<String>);

impl DocumentId {
	pub fn path(&self) -> &Path {
		let Self(id) = self;
		Path::new(id.as_ref())
	}

	pub fn new(id: String) -> Self {
		Self(Arc::new(id))
	}
}

impl AsRef<str> for DocumentId {
	fn as_ref(&self) -> &str {
		let Self(id) = self;
		id.as_str()
	}
}

impl Display for DocumentId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.path().display())
	}
}

#[derive(Debug)]
pub enum WranglerCommand {
	SetTree(DocumentTree),
	Document(Ticket, DocumentId),
	ExploreTree(Ticket),
	Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
pub struct Ticket(u64);

impl Ticket {
	pub fn new(ticket_id: u64) -> Ticket {
		Self(ticket_id)
	}

	pub fn take() -> Ticket {
		static COUNTER: AtomicU64 = AtomicU64::new(0);
		Self(COUNTER.fetch_add(1, Ordering::AcqRel))
	}

	pub fn value(self) -> u64 {
		let Self(id) = self;
		id
	}
}

impl Display for Ticket {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let Ticket(t) = self;
		write!(f, "[t{t}]")
	}
}

#[derive(Debug)]
pub enum WranglerResult {
	Handled,
	SomebodyElsesProblem,
}

#[derive(Debug)]
pub struct DiscoveryDocument<'a> {
	pub document: DocumentId,
	pub file_name: &'a str,
	pub size: u64,
	pub timestamp: SystemTime,
}

#[derive(Debug)]
pub enum Discovery<'a> {
	Begin,
	Document(&'a DiscoveryDocument<'a>),
	End,
}

pub struct FileContent<'a> {
	pub document: DocumentId,
	pub size: u64,
	pub timestamp: SystemTime,
	pub file: &'a File,
}

pub trait Wrangler: Send {
	fn file<'a>(
		&mut self,
		ticket: Ticket,
		result: &Result<FileContent<'a>, io::Error>,
	) -> WranglerResult;

	fn discover(&mut self, ticket: Ticket, discovery: Discovery) -> WranglerResult {
		let _ = (ticket, discovery);
		WranglerResult::SomebodyElsesProblem
	}
}

#[derive(Clone)]
pub struct WranglerSystem {
	sender: Sender<WranglerCommand>,
	wranglers: Arc<Mutex<Vec<Box<dyn Wrangler>>>>,
}

impl WranglerSystem {
	pub fn new(
		sender: Sender<WranglerCommand>,
		wranglers: Arc<Mutex<Vec<Box<dyn Wrangler>>>>,
	) -> Self {
		Self { sender, wranglers }
	}

	pub fn register(&self, wrangler: Box<dyn Wrangler>) {
		let mut wranglers = self.wranglers.lock().unwrap();
		wranglers.push(wrangler);
	}

	pub fn send(&self, cmd: WranglerCommand) {
		self.sender.send(cmd).unwrap();
	}
}

pub fn create_wrangler(document_tree: DocumentTree) -> (WranglerSystem, JoinHandle<()>) {
	fn open_file(path: &Path) -> Result<(File, u64, SystemTime), io::Error> {
		let file = fs::File::open(path)?;
		let metadata = file.metadata()?;
		let size = metadata.size();
		let timestamp = metadata
			.modified()
			.or_else(|_| metadata.created())
			.unwrap_or(SystemTime::now());
		Ok((file, size, timestamp))
	}

	let (sender, receiver) = channel();
	let wranglers = Arc::new(Mutex::new(Vec::new()));

	let system = WranglerSystem::new(sender, wranglers.clone());

	let handle = thread::spawn(move || {
		let mut document_tree = document_tree;

		for cmd in receiver.into_iter() {
			match cmd {
				WranglerCommand::SetTree(tree) => {
					log::info!("Set tree {tree}");
					document_tree = tree;
				}
				WranglerCommand::Document(ticket, document) => {
					let path = document_tree.path().join(document.path());
					let result = open_file(&path);
					let result = match result {
						Ok((ref file, size, timestamp)) => Ok(FileContent {
							document,
							size,
							timestamp,
							file,
						}),
						Err(err) => Err(err),
					};
					for wrangler in &mut *wranglers.lock().unwrap() {
						let result = wrangler.file(ticket, &result);
						if matches!(result, WranglerResult::Handled) {
							break;
						}
					}
				}
				WranglerCommand::ExploreTree(ticket) => {
					let mut ws = wranglers.lock().unwrap();
					let mut wrangler = None;
					for w in &mut *ws {
						let result = w.discover(ticket, Discovery::Begin);
						if matches!(result, WranglerResult::Handled) {
							wrangler = Some(w);
							break;
						}
					}
					let Some(wrangler) = wrangler else {
						log::warn!("Unhandled discover ticket {ticket}");
						continue;
					};

					let root_path = match expand_tilde_owned(document_tree.path()) {
						Ok(path) => path,
						Err(err) => {
							log::error!("Failed tilde expand tree {}: {}", document_tree, err);
							continue;
						}
					};
					let mut folders = vec![root_path];
					loop {
						let Some(folder) = folders.pop() else {
							break;
						};
						let entries = match fs::read_dir(&folder) {
							Ok(entries) => entries,
							Err(err) => {
								log::error!("Failed folder {}: {}", folder.display(), err);
								continue;
							}
						};

						for entry in entries {
							let entry = match entry {
								Ok(entry) => entry,
								Err(err) => {
									log::error!("Failed entry in {}: {}", folder.display(), err);
									continue;
								}
							};
							let path = entry.path();
							if path.is_dir() {
								folders.push(path);
								continue;
							}
							let metadata = match entry.metadata() {
								Ok(metadata) => metadata,
								Err(err) => {
									log::error!("Failed metadata for {}: {}", path.display(), err);
									continue;
								}
							};

							let file_path =
								path.strip_prefix(document_tree.path()).unwrap_or(&path);
							let document = DocumentId::new(file_path.to_string_lossy().into());
							let name = entry.file_name();
							let name = &name.to_string_lossy();
							let size = metadata.size();
							let timestamp = metadata
								.modified()
								.or_else(|_| metadata.created())
								.unwrap_or(SystemTime::now());

							let result = wrangler.discover(
								ticket,
								Discovery::Document(&DiscoveryDocument {
									document,
									file_name: name,
									size,
									timestamp,
								}),
							);
							debug_assert!(
								matches!(result, WranglerResult::Handled),
								"Unexpected discovery result"
							);
						}
					}

					let result = wrangler.discover(ticket, Discovery::End);
					debug_assert!(
						matches!(result, WranglerResult::Handled),
						"Unexpected discovery end result"
					);
				}
				WranglerCommand::Shutdown => {
					log::info!("Wrangler shutdown");
					return;
				}
			}
		}
	});

	(system, handle)
}
