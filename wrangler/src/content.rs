use std::collections::BTreeMap;
use std::io;
use std::io::Read;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::Context;
use std::task::Poll;
use std::task::Waker;
use std::time::SystemTime;

use crate::DocumentId;
use crate::FileContent;
use crate::Ticket;
use crate::Wrangler;
use crate::WranglerCommand;
use crate::WranglerResult;
use crate::WranglerSystem;

type ContentResult = Result<(Vec<u8>, SystemTime), io::Error>;

#[derive(Debug)]
pub(crate) enum State<T> {
	Incomplete,
	Waiting(Waker),
	Complete(Option<T>),
}

type FileContentStates = BTreeMap<Ticket, State<ContentResult>>;

pub struct FileContentFuture {
	states: Arc<Mutex<FileContentStates>>,
	ticket: Ticket,
}

impl Future for FileContentFuture {
	type Output = ContentResult;

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let mut states = self.states.lock().unwrap();
		let value = if let Some(mut state) = states.get_mut(&self.ticket) {
			match &mut state {
				State::Incomplete => {
					*state = State::Waiting(cx.waker().clone());
					None
				}
				State::Waiting(w) if w.will_wake(cx.waker()) => None,
				State::Waiting(_) => {
					*state = State::Waiting(cx.waker().clone());
					None
				}
				State::Complete(v) => {
					let value = v.take().expect("future already polled to completion");
					Some(value)
				}
			}
		} else {
			panic!("future already polled to completion");
		};

		if let Some(value) = value {
			states.remove(&self.ticket);
			Poll::Ready(value)
		} else {
			Poll::Pending
		}
	}
}

pub struct ContentWrangler {
	states: Arc<Mutex<FileContentStates>>,
}

#[derive(Clone)]
pub struct ContentWranglerAssistant {
	system: WranglerSystem,
	states: Arc<Mutex<FileContentStates>>,
}

impl ContentWrangler {
	/// Creates and registers a wrangler instance.
	///
	/// Should be called once during init.
	/// Returns `ContentWranglerAssistant` which can be cheaply cloned.
	pub fn create(system: WranglerSystem) -> ContentWranglerAssistant {
		let states = Arc::new(Mutex::new(BTreeMap::new()));

		system.register(Box::new(ContentWrangler {
			states: states.clone(),
		}));

		ContentWranglerAssistant {
			system: system.clone(),
			states: states.clone(),
		}
	}
}

impl ContentWranglerAssistant {
	pub fn load(&self, doc: DocumentId) -> FileContentFuture {
		let ticket = Ticket::take();

		let mut states = self.states.lock().unwrap();
		states.insert(ticket, State::Incomplete);
		self.system.send(WranglerCommand::Document(ticket, doc));

		FileContentFuture {
			states: self.states.clone(),
			ticket,
		}
	}
}

impl Wrangler for ContentWrangler {
	fn file(&mut self, ticket: Ticket, result: &Result<FileContent, io::Error>) -> WranglerResult {
		let is_mine = self.states.lock().unwrap().contains_key(&ticket);
		if is_mine {
			let result = match result {
				Ok(content) => {
					let mut file = content.file;
					let mut buf = Vec::with_capacity(content.size as usize);
					file.read_to_end(&mut buf).and(Ok((buf, content.timestamp)))
				}
				Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
			};

			let mut states = self.states.lock().unwrap();
			let state = states
				.get_mut(&ticket)
				.expect("future completed by other thread");
			match std::mem::replace(state, State::Complete(Some(result))) {
				State::Incomplete => {}
				State::Waiting(waker) => waker.wake(),
				State::Complete(_) => unreachable!("future already completed"),
			}

			WranglerResult::Handled
		} else {
			WranglerResult::SomebodyElsesProblem
		}
	}
}
