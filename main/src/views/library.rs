use std::array;
use std::collections::BTreeMap;
use std::collections::BinaryHeap;
use std::fmt::Write;
use std::fs;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use egui::CentralPanel;
use egui::Color32;
use egui::Panel;
use egui::RichText;
use egui::TextStyle;
use egui::load::Bytes;
use lucide_icons::Icon;
use scribe::ScribeAssistant;
use scribe::library;
use scribe::library::Book;
use scribe::library::BookId;
use scribe::library::Thumbnail;
use scribe::record_keeper::RecordKeeper;
use scribe::record_keeper::RecordKeeperAssistant;
use scribe::record_keeper::RecordKeeperError;

use crate::AppBell;
use crate::AppEvent;
use crate::gestures::Direction;
use crate::gestures::Gesture;
use crate::renderer::Painter;
use crate::ui::MainMenuBar;
use crate::ui::MenuItem;
use crate::ui::OnAction;
use crate::ui::ToolBar;
use crate::ui::ToolItem;
use crate::ui::UiIcon;
use crate::views::EventResult;
use crate::views::GestureResult;
use crate::views::ViewHandle;

pub const LIBRARY_LIST_SIZE: usize = 5;

struct BookCard {
	id: BookId,
	title: Option<Arc<String>>,
	author: Option<Arc<String>>,
	modified_at: DateTime<Utc>,
	thumbnail: Thumbnail,
}

#[derive(Debug, Default)]
struct Shelves {
	pub(crate) books: BTreeMap<BookId, Book>,
	pub(crate) sorted: Vec<BookId>,
	pub(crate) thumbnails: BTreeMap<BookId, Thumbnail>,
}

impl Shelves {
	fn open(books: BTreeMap<BookId, Book>) -> Self {
		let books_len = books.len();
		let sorted = books
			.values()
			.map(|book| (book.modified_at, book.id))
			.collect::<BinaryHeap<_>>()
			.into_iter()
			.map(|(_, id)| id)
			.collect();
		log::info!("Open library with {books_len} books");

		Self {
			books,
			sorted,
			thumbnails: BTreeMap::new(),
		}
	}

	fn update(&mut self, book: library::Book) {
		self.thumbnails.remove(&book.id);
		self.books.insert(book.id, book);
		self.sorted.splice(
			..,
			self.books
				.values()
				.map(|book| (book.modified_at, book.id))
				.collect::<BinaryHeap<_>>()
				.into_iter()
				.map(|(_, id)| id),
		);
	}

	fn remove(&mut self, book_id: BookId) {
		self.thumbnails.remove(&book_id);
		self.books.remove(&book_id);
		self.sorted.splice(
			..,
			self.books
				.values()
				.map(|book| (book.modified_at, book.id))
				.collect::<BinaryHeap<_>>()
				.into_iter()
				.map(|(_, id)| id),
		);
	}

	fn books(&self, n: std::ops::Range<u32>) -> Vec<Book> {
		let start = n.start as usize;
		let end = (n.end as usize).min(self.sorted.len());
		let books = self
			.sorted
			.get(start..end)
			.into_iter()
			.flatten()
			.filter_map(|id| self.books.get(id).cloned())
			.collect::<Vec<_>>();
		log::trace!(
			"Requested books {start}..{end}, received {} from all books {}",
			books.len(),
			self.sorted.len()
		);
		books
	}
}

pub(crate) struct LibraryView {
	bell: AppBell,
	records: RecordKeeperAssistant,
	scribe: ScribeAssistant,
	shelves: Shelves,
	page: u32,
	cards: [Option<BookCard>; LIBRARY_LIST_SIZE],
	statusline: Option<String>,
}

impl LibraryView {
	pub(crate) fn create(
		bell: AppBell,
		records: RecordKeeper,
		scribe: ScribeAssistant,
	) -> Result<Self, RecordKeeperError> {
		// TODO: Preserve page somewhere
		let page = 0;

		let records = records.assistant()?;
		let books = match records.fetch_books() {
			Ok(books) => books,
			Err(e) => {
				log::error!("Fetch books error: {e}");
				BTreeMap::new()
			}
		};
		let mut shelves = Shelves::open(books);
		let cards = read_cards(&mut shelves, &records, page);

		Ok(Self {
			bell,
			records,
			scribe,
			shelves,
			page,
			cards,
			statusline: None,
		})
	}

	fn prev_page(&mut self) {
		self.page = self.page.saturating_sub(1);
		self.cards = read_cards(&mut self.shelves, &self.records, self.page);
	}

	fn next_page(&mut self) {
		let page = self.page + 1;
		let cards = read_cards(&mut self.shelves, &self.records, page);
		if cards.iter().any(|c| c.is_some()) {
			self.page = page;
			self.cards = cards;
		}
	}
}

fn read_cards(
	shelves: &mut Shelves,
	records: &RecordKeeperAssistant,
	page: u32,
) -> [Option<BookCard>; LIBRARY_LIST_SIZE] {
	let start = page * LIBRARY_LIST_SIZE as u32;
	let end = (1 + page) * LIBRARY_LIST_SIZE as u32;
	let books = shelves.books(start..end);
	let mut books_iter = books.into_iter().map(|b| {
		let id = b.id;
		let thumb = shelves
			.thumbnails
			.entry(id)
			.or_insert_with(|| match load_thumbnail(records, id) {
				Ok(thumb) => thumb,
				Err(e) => {
					log::error!("Error loading thumbnail: {e}");
					Thumbnail::None
				}
			})
			.clone();

		(b, thumb)
	});
	array::from_fn(|_| books_iter.next().map(BookCard::new))
}

fn load_thumbnail(
	records: &RecordKeeperAssistant,
	id: BookId,
) -> Result<Thumbnail, RecordKeeperError> {
	if let Some(thumbnail) = records.fetch_thumbnail(id)?
		&& let Some(path) = thumbnail.path.as_deref()
	{
		match fs::read(path) {
			Ok(bytes) => {
				let thumbnail = Thumbnail::Bytes {
					bytes: bytes.into(),
				};
				Ok(thumbnail)
			}
			Err(e) => {
				log::warn!("Failed to load thumbnail at {}: {e}", path.display());
				Ok(Thumbnail::None)
			}
		}
	} else {
		Ok(Thumbnail::None)
	}
}

#[derive(Clone, Copy)]
enum MenuAction {
	Exit,
	Scan,
	OpenExperiment,
}

#[derive(Clone, Copy)]
enum ToolAction {
	Prev,
	Next,
}

impl OnAction<MenuAction> for LibraryView {
	fn on_action(&mut self, action: MenuAction) {
		match action {
			MenuAction::Exit => {
				self.bell.send_event(AppEvent::Exit);
			}
			MenuAction::Scan => {
				self.scribe.scan();
			}
			MenuAction::OpenExperiment => {
				self.bell.send_event(AppEvent::OpenExperiments);
			}
		}
	}
}

impl OnAction<ToolAction> for LibraryView {
	fn on_action(&mut self, action: ToolAction) {
		match action {
			ToolAction::Prev => self.prev_page(),
			ToolAction::Next => self.next_page(),
		}
	}
}

impl ViewHandle for LibraryView {
	fn draw<'a, 'b>(&'a mut self, painter: Painter<'b>) {
		painter.draw_ui(|ui| {
			let menu_items = &[
				MenuItem {
					icon: Icon::RefreshCw,
					description: "Scan",
					active: false,
					action: MenuAction::Scan,
				},
				MenuItem {
					icon: Icon::RefreshCw,
					description: "Experiment",
					active: false,
					action: MenuAction::OpenExperiment,
				},
				MenuItem {
					icon: Icon::LogOut,
					description: "Exit",
					active: false,
					action: MenuAction::Exit,
				},
			];
			let tool_items = &[
				None,
				Some(ToolItem {
					icon: Icon::ArrowLeft,
					description: "Previous",
					active: false,
					action: ToolAction::Prev,
				}),
				None,
				None,
				None,
				None,
				Some(ToolItem {
					icon: Icon::ArrowRight,
					description: "Next",
					active: false,
					action: ToolAction::Next,
				}),
				None,
			];

			let mut statusline = self.statusline.take().unwrap_or_default();
			statusline.clear();
			let page = self.page + 1;
			let books = self.shelves.books.len();
			let (full_pages, part_page) = (books / LIBRARY_LIST_SIZE, books % LIBRARY_LIST_SIZE);
			let pages = full_pages + part_page.max(1);
			let _ = write!(statusline, "{} / {}", page, pages);

			let working = self.scribe.working();
			let top_panel = Panel::top("top").show_inside(ui, |ui| {
				MainMenuBar::new(self, menu_items)
					.with_loading(working)
					.with_status(Some(&statusline))
					.ui(ui)
			});
			let is_open = top_panel.inner.context_menu_opened();

			self.statusline = Some(statusline);

			Panel::bottom("bottom")
				.show_inside(ui, |ui| ToolBar::new(self, tool_items, is_open).ui(ui));

			CentralPanel::default().show_inside(ui, |ui| {
				if is_open {
					ui.disable();
				}

				let height = ui.available_height()
					- (LIBRARY_LIST_SIZE as f32 - 1.0) * ui.spacing().item_spacing.y;
				let card_height = height / LIBRARY_LIST_SIZE as f32;

				ui.vertical(|ui| {
					for card in self.cards.iter().flatten() {
						ui.allocate_ui([ui.available_width(), card_height].into(), |ui| {
							if ui.add(card.ui()).clicked() {
								self.bell.send_event(AppEvent::OpenReader(card.id));
							}
						});
					}
				})
			});
		});
	}

	fn event(&mut self, event: &AppEvent) -> EventResult {
		match event {
			AppEvent::BookUpdated(id) => {
				match self.records.fetch_book(*id) {
					Ok(book) => {
						self.shelves.update(book);
					}
					Err(e) => {
						log::error!("Error fetching book {id}: {e}");
						self.shelves.remove(*id);
					}
				};
				self.cards = read_cards(&mut self.shelves, &self.records, self.page);
				EventResult::RequestRedraw
			}
			AppEvent::NavigateNext => {
				self.next_page();
				EventResult::RequestRedraw
			}
			AppEvent::NavigatePrevious => {
				self.prev_page();
				EventResult::RequestRedraw
			}
			_ => EventResult::None,
		}
	}

	fn gesture(&mut self, event: &crate::gestures::GestureEvent) -> GestureResult {
		match event.gesture {
			Gesture::Swipe(Direction::Right, _) => {
				self.prev_page();
				GestureResult::Consumed
			}
			Gesture::Swipe(Direction::Left, _) => {
				self.next_page();
				GestureResult::Consumed
			}
			_ => GestureResult::Unhandled,
		}
	}
}

impl BookCard {
	fn new(entry: (library::Book, library::Thumbnail)) -> Self {
		let (b, tn) = entry;
		BookCard {
			id: b.id,
			title: b.title,
			author: b.author,
			modified_at: b.modified_at,
			thumbnail: tn,
		}
	}
}

struct BookCardUi<'a> {
	card: &'a BookCard,
}

impl egui::Widget for BookCardUi<'_> {
	fn ui(self, ui: &mut egui::Ui) -> egui::Response {
		let card = self.card;
		ui.group(|ui| {
			ui.set_min_size(ui.available_size());
			let height = ui.available_height();
			let width = ui.available_width();
			ui.horizontal(|ui| {
				ui.set_width(width);
				let cover_width = height * 0.75;
				ui.allocate_ui([cover_width, height].into(), |ui| {
					ui.set_width(cover_width);
					ui.centered_and_justified(|ui| match &card.thumbnail {
						Thumbnail::Bytes { bytes } => {
							ui.add(egui::Image::new(egui::ImageSource::Bytes {
								uri: format!("bytes://thumbnail_{}.png", card.id.value()).into(),
								bytes: Bytes::Shared(bytes.clone()),
							}))
						}
						Thumbnail::None => ui.label(
							UiIcon::new(Icon::Book)
								.size(cover_width)
								.color(Color32::GRAY)
								.build(),
						),
					})
				});
				ui.separator();
				ui.vertical(|ui| {
					let author = card
						.author
						.as_ref()
						.map(|t| t.as_str())
						.unwrap_or("Unknown");
					ui.label(author);
					let title = card.title.as_ref().map(|t| t.as_str()).unwrap_or("Unknown");
					ui.label(RichText::new(title).text_style(TextStyle::Heading));
					ui.label(format!("{}", card.modified_at.format("%Y-%m-%d %H:%M")));
				});
			});
			ui.interact(
				ui.min_rect(),
				ui.id().with(card.id.value()),
				egui::Sense::click(),
			)
		})
		.inner
	}
}

impl BookCard {
	pub(crate) fn ui<'a>(&'a self) -> BookCardUi<'a> {
		BookCardUi { card: self }
	}
}
