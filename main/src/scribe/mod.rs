#![allow(dead_code)]
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::mpsc::RecvError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread;
use std::thread::JoinHandle;

#[derive(PartialOrd, Ord, PartialEq, Eq)]
pub(crate) struct DocumentId(usize);

pub(crate) struct Document {
	path: PathBuf,
}

#[derive(Default)]
pub(crate) enum SortField {
	#[default]
	Date,
}

#[derive(Default)]
pub(crate) enum SortDirection {
	#[default]
	Ascending,
	Descending,
}

#[derive(Default)]
pub(crate) struct SortOrder(SortField, SortDirection);

#[derive(Default)]
pub(crate) struct Library {
	docs: BTreeMap<DocumentId, Document>,
	order: SortOrder,
	sorted: Vec<DocumentId>,
}

#[derive(Clone, Copy)]
pub struct ScribeTicket(usize);

pub(crate) trait ScribeBell {
	fn completed(&self, ticket: ScribeTicket);
	fn failed(&self, ticket: ScribeTicket, error: String);
}

pub(crate) enum ScribeOrder {
	RefreshMetadatas(Vec<DocumentId>),
	Thumbnails(Vec<DocumentId>),
	Sort(SortOrder),
	Quit,
}

struct ScribeRequest(ScribeTicket, ScribeOrder);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ScribeError {
	#[error(transparent)]
	Recv(#[from] RecvError),
	#[error("Failed to send order")]
	SendFailed,
}

pub(crate) struct Scribe {
	lib: Arc<RwLock<Library>>,
	order_tx: Sender<ScribeRequest>,
	handle: JoinHandle<Result<(), ScribeError>>,
	ticket_cnt: Cell<usize>,
}

impl Scribe {
	pub fn create<Bell>(bell: Bell) -> Self
	where
		Bell: ScribeBell + Send + 'static,
	{
		let lib = Arc::new(RwLock::new(Library::default()));

		let (order_tx, order_rx) = channel();
		let worker_lib = Arc::new(RwLock::new(Library::default())).clone();
		let handle = thread::spawn(move || -> Result<(), ScribeError> {
			let bell = bell;
			let _lib = worker_lib;
			let order_rx = order_rx;
			loop {
				let order = order_rx.recv();
				match order {
					Ok(ScribeRequest(ticket, ScribeOrder::RefreshMetadatas(_docs))) => {
						// TODO: Refresh
						bell.completed(ticket);
					}
					Ok(ScribeRequest(ticket, ScribeOrder::Thumbnails(_docs))) => {
						// TODO: Generate thumbnails
						bell.completed(ticket);
					}
					Ok(ScribeRequest(ticket, ScribeOrder::Sort(_order))) => {
						// TODO: Sort documents
						bell.completed(ticket);
					}
					Ok(ScribeRequest(_, ScribeOrder::Quit)) => {
						break Ok(());
					}
					Err(e) => {
						break Err(e.into());
					}
				}
			}
		});

		Scribe {
			lib,
			order_tx,
			handle,
			ticket_cnt: Cell::new(0),
		}
	}

	fn ticket(&self) -> ScribeTicket {
		let ticket_id = self.ticket_cnt.get();
		self.ticket_cnt.set(ticket_id + 1);
		ScribeTicket(ticket_id)
	}

	pub fn request(&self, order: ScribeOrder) -> Result<ScribeTicket, ScribeError> {
		let ticket = self.ticket();
		self.order_tx
			.send(ScribeRequest(ticket, order))
			.map_err(|_| ScribeError::SendFailed)?;
		Ok(ticket)
	}

	pub fn library(&self) -> &RwLock<Library> {
		&self.lib
	}
}
