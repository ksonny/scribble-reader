use std::array;
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
use scribe::ScribeRequest;
use scribe::library;
use scribe::library::BookId;

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

struct Thumbnail {
	bytes: Arc<[u8]>,
}

struct BookCard {
	id: BookId,
	title: Option<Arc<String>>,
	author: Option<Arc<String>>,
	modified_at: DateTime<Utc>,
	thumbnail: Option<Thumbnail>,
}

pub(crate) struct LibraryView {
	bell: AppBell,
	scribe: ScribeAssistant,
	page: u32,
	cards: [Option<BookCard>; LIBRARY_LIST_SIZE],
}

impl LibraryView {
	pub(crate) fn create(bell: AppBell, scribe: ScribeAssistant) -> Self {
		// TODO: Preserve page somewhere
		let page = 0;
		let cards = read_cards(&scribe, page);

		Self {
			bell,
			scribe,
			page,
			cards,
		}
	}

	fn prev_page(&mut self) {
		self.page = self.page.saturating_sub(1);
		self.cards = read_cards(&self.scribe, self.page);
	}

	fn next_page(&mut self) {
		let page = self.page + 1;
		let cards = read_cards(&self.scribe, page);
		if cards.iter().any(|c| c.is_some()) {
			self.page = page;
			self.cards = cards;
		}
	}
}

fn read_cards(scribe: &ScribeAssistant, page: u32) -> [Option<BookCard>; LIBRARY_LIST_SIZE] {
	let start = page * LIBRARY_LIST_SIZE as u32;
	let end = (1 + page) * LIBRARY_LIST_SIZE as u32;
	let books = scribe.library().books(start..end);
	let mut books_iter = books.into_iter().map(|b| {
		let id = b.id;
		let thumb = scribe.library().thumbnail(id);
		(b, thumb)
	});
	let cards = array::from_fn(|_| books_iter.next().map(BookCard::new));

	for card in &cards {
		if let Some(card) = card
			&& card.thumbnail.is_none()
		{
			scribe.send(ScribeRequest::Show(card.id));
		}
	}
	cards
}

#[derive(Clone, Copy)]
enum MenuAction {
	Exit,
	Refresh,
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
			MenuAction::Refresh => {
				self.scribe.send(ScribeRequest::Scan);
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
					description: "Refresh",
					active: false,
					action: MenuAction::Refresh,
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

			let top_panel = Panel::top("top")
				.show_inside(ui, |ui| MainMenuBar::new(self, menu_items, false).ui(ui));
			let is_open = top_panel.inner.context_menu_opened();

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
			AppEvent::LibraryUpdated => {
				self.cards = read_cards(&self.scribe, self.page);
				EventResult::RequestRedraw
			}
			AppEvent::LibraryBookUpdated(id) => {
				if self.cards.iter().flatten().any(|c| &c.id == id) {
					self.cards = read_cards(&self.scribe, self.page);
					EventResult::RequestRedraw
				} else {
					EventResult::None
				}
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
	fn new(entry: (library::Book, Option<library::Thumbnail>)) -> Self {
		let (b, tn) = entry;
		BookCard {
			id: b.id,
			title: b.title,
			author: b.author,
			modified_at: b.modified_at,
			thumbnail: tn.and_then(|tn| match tn {
				library::Thumbnail::Bytes { bytes } => Some(Thumbnail { bytes }),
				library::Thumbnail::None => None,
			}),
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
						Some(Thumbnail { bytes }) => {
							ui.add(egui::Image::new(egui::ImageSource::Bytes {
								uri: format!("bytes://thumbnail_{}.png", card.id.value()).into(),
								bytes: Bytes::Shared(bytes.clone()),
							}))
						}
						None => ui.label(
							UiIcon::new(Icon::Book)
								.size(cover_width)
								.color(Color32::GRAY)
								.build(),
						),
					});
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
