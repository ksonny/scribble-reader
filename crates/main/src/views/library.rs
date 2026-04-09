use std::array;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::fmt::Write;
use std::fs;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use egui::Align;
use egui::CentralPanel;
use egui::Color32;
use egui::CornerRadius;
use egui::Layout;
use egui::Panel;
use egui::ProgressBar;
use egui::RichText;
use egui::Vec2;
use egui::load::Bytes;
use lucide_icons::Icon;
use scribe::Book;
use scribe::BookId;
use scribe::LibraryScribeAssistant;
use scribe::RecordKeeper;
use scribe::RecordKeeperAssistant;
use scribe::RecordKeeperError;
use scribe::Thumbnail;
use serde::Deserialize;
use serde::Serialize;

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
use crate::ui::theme;
use crate::views::EventResult;
use crate::views::GestureResult;
use crate::views::ViewHandle;

pub const LIBRARY_LIST_SIZE: usize = 5;

struct BookCard {
	id: BookId,
	title: Option<Arc<String>>,
	author: Option<Arc<String>>,
	percent_read: u32,
	sort_by: SortBy,
	opened_at: Option<DateTime<Utc>>,
	modified_at: DateTime<Utc>,
	added_at: DateTime<Utc>,
	thumbnail: Thumbnail,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
enum SortBy {
	#[default]
	Modified,
	Added,
	Opened,
}

impl Display for SortBy {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			SortBy::Modified => write!(f, "Modified order"),
			SortBy::Added => write!(f, "Added order"),
			SortBy::Opened => write!(f, "Opened order"),
		}
	}
}

#[derive(Debug, PartialEq, PartialOrd, Eq, Ord)]
struct SortKey {
	modified_at: i64,
	added_at: i64,
	opened_at: Option<i64>,
}

impl SortKey {
	fn new(book: &Book) -> Self {
		Self {
			modified_at: book.modified_at.timestamp(),
			added_at: book.added_at.timestamp(),
			opened_at: book.opened_at.as_ref().map(DateTime::timestamp),
		}
	}

	fn select(&self, sort_by: SortBy) -> i64 {
		match sort_by {
			SortBy::Modified => self.modified_at,
			SortBy::Added => self.added_at,
			SortBy::Opened => self.opened_at.unwrap_or(0),
		}
	}
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
struct ViewState {
	sort_by: SortBy,
	page: u32,
}

#[derive(Debug)]
struct Shelves {
	books: BTreeMap<BookId, Book>,
	sorted: Vec<(BookId, SortKey)>,
	sort_by: SortBy,
	thumbnails: BTreeMap<BookId, Thumbnail>,
}

impl Shelves {
	fn open(books: BTreeMap<BookId, Book>, sort_by: SortBy) -> Self {
		log::info!("Open library with {} books", books.len());
		let sorted = books.iter().map(|(id, b)| (*id, SortKey::new(b))).collect();
		let mut shelves = Self {
			books,
			sort_by,
			sorted,
			thumbnails: BTreeMap::new(),
		};
		shelves.sort_books();
		shelves
	}

	fn update(&mut self, book: Book) {
		self.thumbnails.remove(&book.id);
		self.books.insert(book.id, book);
		self.sorted
			.splice(.., self.books.iter().map(|(id, b)| (*id, SortKey::new(b))));
		self.sort_books();
	}

	fn remove(&mut self, book_id: BookId) {
		self.thumbnails.remove(&book_id);
		self.books.remove(&book_id);
		self.sorted
			.splice(.., self.books.iter().map(|(id, b)| (*id, SortKey::new(b))));
		self.sort_books();
	}

	fn sort_books(&mut self) {
		self.sorted
			.sort_by_key(|(_, k)| Reverse(k.select(self.sort_by)));
	}

	fn book(&self, id: BookId) -> Option<&Book> {
		self.books.get(&id)
	}

	fn books(&self, n: std::ops::Range<u32>) -> Vec<BookId> {
		let start = n.start as usize;
		let end = (n.end as usize).min(self.sorted.len());

		self.sorted
			.get(start..end)
			.into_iter()
			.flatten()
			.map(|(id, _)| id)
			.cloned()
			.collect()
	}
}

pub(crate) struct LibraryView {
	bell: AppBell,
	records: RecordKeeperAssistant,
	scribe: LibraryScribeAssistant,
	shelves: Shelves,
	state: ViewState,
	cards: [Option<BookCard>; LIBRARY_LIST_SIZE],
	statusline: Option<String>,
}

impl LibraryView {
	const STATE_KEY: &str = "library_view_state";

	pub(crate) fn create(
		bell: AppBell,
		records: RecordKeeper,
		scribe: LibraryScribeAssistant,
	) -> Result<Self, RecordKeeperError> {
		let records = records.assistant()?;

		let state: ViewState = records
			.fetch_view_state(Self::STATE_KEY)?
			.unwrap_or_default();

		let books = match records.fetch_books() {
			Ok(books) => books,
			Err(e) => {
				log::error!("Fetch books error: {e}");
				BTreeMap::new()
			}
		};
		let mut shelves = Shelves::open(books, state.sort_by);
		let cards = read_cards(&mut shelves, &records, state.page);

		Ok(Self {
			bell,
			records,
			scribe,
			shelves,
			state,
			cards,
			statusline: None,
		})
	}

	fn prev_page(&mut self) {
		self.state.page = self.state.page.saturating_sub(1);
		self.cards = read_cards(&mut self.shelves, &self.records, self.state.page);
		let _ = self
			.records
			.record_view_state(Self::STATE_KEY, &self.state)
			.inspect_err(|e| log::warn!("Error saving state: {e}"));
	}

	fn next_page(&mut self) {
		let page = self.state.page + 1;
		let cards = read_cards(&mut self.shelves, &self.records, page);
		if cards.iter().any(|c| c.is_some()) {
			self.state.page = page;
			self.cards = cards;
			let _ = self
				.records
				.record_view_state(Self::STATE_KEY, &self.state)
				.inspect_err(|e| log::warn!("Error saving state: {e}"));
		}
	}

	fn sort_by_next(&mut self) {
		self.shelves.sort_by = match self.shelves.sort_by {
			SortBy::Opened => SortBy::Modified,
			SortBy::Modified => SortBy::Added,
			SortBy::Added => SortBy::Opened,
		};
		self.shelves.sort_books();
		self.cards = read_cards(&mut self.shelves, &self.records, self.state.page);

		self.state.sort_by = self.shelves.sort_by;
		let _ = self
			.records
			.record_view_state(Self::STATE_KEY, &self.state)
			.inspect_err(|e| log::warn!("Error saving state: {e}"));
	}
}

fn read_cards(
	shelves: &mut Shelves,
	records: &RecordKeeperAssistant,
	page: u32,
) -> [Option<BookCard>; LIBRARY_LIST_SIZE] {
	let start = page * LIBRARY_LIST_SIZE as u32;
	let end = (1 + page) * LIBRARY_LIST_SIZE as u32;
	let book_ids = shelves.books(start..end);

	array::from_fn(|i| {
		let id = book_ids.get(i)?;

		let thumb = shelves
			.thumbnails
			.entry(*id)
			.or_insert_with(|| match load_thumbnail(records, *id) {
				Ok(thumb) => thumb,
				Err(e) => {
					log::error!("Error loading thumbnail: {e}");
					Thumbnail::None
				}
			})
			.clone();
		let book = shelves.book(*id)?;

		Some(BookCard::new(book, thumb, shelves.sort_by))
	})
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
	Sort,
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
			ToolAction::Sort => self.sort_by_next(),
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
				Some(ToolItem {
					icon: Icon::ArrowDownNarrowWide,
					description: "Sort",
					active: false,
					action: ToolAction::Sort,
				}),
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
			let page = self.state.page + 1;
			let books = self.shelves.books.len();
			let (full_pages, part_page) = (books / LIBRARY_LIST_SIZE, books % LIBRARY_LIST_SIZE);
			let pages = full_pages + part_page.min(1);
			let _ = write!(statusline, "{} {} / {}", self.shelves.sort_by, page, pages);

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
				self.cards = read_cards(&mut self.shelves, &self.records, self.state.page);
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

	fn resize(&mut self, width: u32, height: u32) {
		let _ = (width, height);
	}

	fn rescale(&mut self, scale_factor: f32) {
		let _ = scale_factor;
	}
}

impl BookCard {
	fn new(book: &Book, thumbnail: Thumbnail, sort_by: SortBy) -> Self {
		BookCard {
			id: book.id,
			title: book.title.clone(),
			author: book.author.clone(),
			percent_read: book.percent_read.unwrap_or_default(),
			sort_by,
			opened_at: book.opened_at,
			modified_at: book.modified_at,
			added_at: book.added_at,
			thumbnail,
		}
	}
}

struct BookCardUi<'a> {
	card: &'a BookCard,
}

impl egui::Widget for BookCardUi<'_> {
	fn ui(self, ui: &mut egui::Ui) -> egui::Response {
		let card = self.card;
		ui.spacing_mut().item_spacing = Vec2::new(3., 3.);
		ui.group(|ui| {
			let height = ui.available_height();
			ui.horizontal(|ui| {
				let cover_width = height * 0.75;
				ui.allocate_ui([cover_width, height].into(), |ui| {
					ui.set_width(cover_width);
					ui.centered_and_justified(|ui| match &card.thumbnail {
						Thumbnail::Bytes { bytes } => {
							ui.add(egui::Image::new(egui::ImageSource::Bytes {
								uri: format!("bytes://thumbnail_{}.png", card.id.into_inner())
									.into(),
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
						.as_deref()
						.map(String::as_str)
						.unwrap_or("Unknown");
					ui.label(RichText::new(author).size(theme::S_SIZE));

					let title = card
						.title
						.as_deref()
						.map(String::as_str)
						.unwrap_or("Unknown");
					ui.label(RichText::new(title).size(theme::L_SIZE));

					match card.sort_by {
						SortBy::Modified => {
							ui.label(
								RichText::new(format!(
									"Changed {}",
									card.modified_at.format("%e %b %H:%M")
								))
								.size(theme::S_SIZE),
							);
						}
						SortBy::Opened => {
							if let Some(opened_at) = card.opened_at {
								ui.label(
									RichText::new(format!(
										"Opened {}",
										opened_at.format("%e %b %H:%M")
									))
									.size(theme::S_SIZE),
								);
							} else {
								ui.label(RichText::new("Never opened").size(theme::S_SIZE));
							}
						}
						SortBy::Added => {
							ui.label(
								RichText::new(format!(
									"Added {}",
									card.added_at.format("%e %b %H:%M")
								))
								.size(theme::S_SIZE),
							);
						}
					}

					ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
						let read_part = card.percent_read as f32 / 100.;
						ui.add(
							ProgressBar::new(read_part)
								.corner_radius(CornerRadius::ZERO)
								.fill(theme::SECONDARY_COLOR)
								.desired_height(3.),
						);
					});
				});
			});
			ui.interact(
				ui.min_rect(),
				ui.id().with(card.id.into_inner()),
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
