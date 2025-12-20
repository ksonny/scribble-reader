use std::array;
use std::sync::Arc;
use std::time::Instant;

use chrono::DateTime;
use chrono::Utc;
use egui::Color32;
use egui::Context;
use egui::FontFamily;
use egui::FontId;
use egui::ImageSource;
use egui::Layout;
use egui::Rect;
use egui::RichText;
use egui::Stroke;
use egui::TextFormat;
use egui::TextStyle;
use egui::TextWrapMode;
use egui::Vec2;
use egui::ViewportId;
use egui::epaint::text::FontInsert;
use egui::load::Bytes;
use egui::text::LayoutJob;
use illustrator::IllustratorHandle;
use lucide_icons::Icon;
use scribe::ScribeAssistant;
use scribe::ScribeState;

use scribe::library;
use scribe::library::BookId;
use scribe::library::Location;

use crate::gestures;

pub mod theme {
	use egui::Color32;
	use egui::FontFamily;
	use egui::FontId;
	use egui::TextStyle;
	use lazy_static::lazy_static;

	pub const DEFAULT_SIZE: f32 = 14.0;
	pub const S_SIZE: f32 = 12.0;
	pub const M_SIZE: f32 = 18.0;
	pub const L_SIZE: f32 = 24.0;
	pub const XL_SIZE: f32 = 48.0;
	pub const ACCENT_COLOR: Color32 = Color32::DARK_RED;

	lazy_static! {
		pub static ref ICON_FONT_FAMILY: FontFamily = FontFamily::Name("lucide-icons".into());
		pub static ref ICON_FONT: FontId = FontId::new(DEFAULT_SIZE, ICON_FONT_FAMILY.clone());
		pub static ref ICON_L_FONT: FontId = FontId::new(L_SIZE, ICON_FONT_FAMILY.clone());
		pub static ref ICON_XL_FONT: FontId = FontId::new(XL_SIZE, ICON_FONT_FAMILY.clone());
		pub static ref ICON_STYLE: TextStyle = TextStyle::Name("ICON_STYLE".into());
		pub static ref ICON_L_STYLE: TextStyle = TextStyle::Name("ICON_L_STYLE".into());
		pub static ref ICON_XL_STYLE: TextStyle = TextStyle::Name("ICON_XL_STYLE".into());
		pub static ref HEADING2: TextStyle = TextStyle::Name("HEADING2".into());
	}
}

pub fn create_egui_ctx() -> Context {
	let egui_ctx = Context::default();
	egui_extras::install_image_loaders(&egui_ctx);

	egui_ctx.add_font(FontInsert::new(
		"lucide-icons",
		egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES),
		vec![egui::epaint::text::InsertFontFamily {
			family: theme::ICON_FONT_FAMILY.clone(),
			priority: egui::epaint::text::FontPriority::Lowest,
		}],
	));
	egui_ctx.set_theme(egui::Theme::Light);
	egui_ctx.style_mut(|style| {
		style.animation_time = 0.0;
		style.spacing.item_spacing = Vec2::new(5.0, 5.0);
		style.wrap_mode = Some(TextWrapMode::Truncate);
		style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.open.weak_bg_fill = Color32::TRANSPARENT;
		style.visuals.widgets.open.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.open.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
		style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.active.expansion = 0.0;
		style.visuals.widgets.active.weak_bg_fill = Color32::LIGHT_GRAY;
		style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.hovered.expansion = 2.0;
		style.visuals.widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
		style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::BLACK);

		style.text_styles = [
			(
				TextStyle::Heading,
				FontId::new(25.0, FontFamily::Proportional),
			),
			(
				theme::HEADING2.clone(),
				FontId::new(theme::M_SIZE, FontFamily::Proportional),
			),
			(
				TextStyle::Body,
				FontId::new(theme::DEFAULT_SIZE, FontFamily::Proportional),
			),
			(
				TextStyle::Monospace,
				FontId::new(theme::DEFAULT_SIZE, FontFamily::Monospace),
			),
			(
				TextStyle::Button,
				FontId::new(theme::M_SIZE, FontFamily::Proportional),
			),
			(
				TextStyle::Small,
				FontId::new(theme::S_SIZE, FontFamily::Proportional),
			),
			(
				theme::ICON_STYLE.clone(),
				FontId::new(theme::DEFAULT_SIZE, theme::ICON_FONT_FAMILY.clone()),
			),
			(
				theme::ICON_L_STYLE.clone(),
				FontId::new(theme::L_SIZE, theme::ICON_FONT_FAMILY.clone()),
			),
			(
				theme::ICON_XL_STYLE.clone(),
				FontId::new(theme::XL_SIZE, theme::ICON_FONT_FAMILY.clone()),
			),
		]
		.into();
	});
	egui_ctx
}

pub struct UiInput {
	start_time: Instant,
	pixels_per_point: f32,
	egui_input: egui::RawInput,
}

impl Default for UiInput {
	fn default() -> Self {
		Self::new()
	}
}

impl UiInput {
	pub fn new() -> Self {
		Self {
			start_time: Instant::now(),
			egui_input: egui::RawInput::default(),
			pixels_per_point: 1.0,
		}
	}

	pub fn resume(&mut self, size: winit::dpi::PhysicalSize<u32>, scale_factor: f32) {
		self.pixels_per_point = scale_factor;
		self.egui_input.screen_rect = Some(egui::Rect::from_min_size(
			Default::default(),
			egui::vec2(size.width as f32, size.height as f32) / self.pixels_per_point,
		));
		self.egui_input.viewport_id = ViewportId::ROOT;
		self.egui_input
			.viewports
			.entry(ViewportId::ROOT)
			.or_default()
			.native_pixels_per_point = Some(scale_factor);
	}

	pub fn handle_gesture(&mut self, event: &gestures::GestureEvent) {
		use gestures::Gesture;
		let pos = self.translate_pos(event.loc);
		if event.gesture == Gesture::Tap {
			self.egui_input.events.push(egui::Event::PointerButton {
				pos,
				button: egui::PointerButton::Primary,
				pressed: true,
				modifiers: egui::Modifiers::default(),
			});
			self.egui_input.events.push(egui::Event::PointerButton {
				pos,
				button: egui::PointerButton::Primary,
				pressed: false,
				modifiers: egui::Modifiers::default(),
			});
		}
	}

	pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
		self.egui_input.screen_rect = Some(egui::Rect::from_min_size(
			Default::default(),
			egui::vec2(size.width as f32, size.height as f32) / self.pixels_per_point,
		));
	}

	pub fn rescale(&mut self, scale_factor: f32) {
		self.pixels_per_point = scale_factor;
		self.egui_input
			.viewports
			.entry(ViewportId::ROOT)
			.or_default()
			.native_pixels_per_point = Some(scale_factor);
	}

	pub fn tick(&mut self) -> egui::RawInput {
		self.egui_input.time = Some(Instant::now().duration_since(self.start_time).as_secs_f64());
		self.egui_input.take()
	}

	pub fn translate_pos(&self, loc: gestures::Location) -> egui::Pos2 {
		egui::pos2(loc.x as f32, loc.y as f32) / self.pixels_per_point
	}
}

pub trait GuiView {
	fn draw(&mut self, ctx: &Context, poke_stick: &impl Bell);
}

pub(crate) struct Thumbnail {
	pub(crate) bytes: Arc<[u8]>,
}

pub(crate) struct BookCard {
	pub(crate) id: BookId,
	pub(crate) title: Option<Arc<String>>,
	pub(crate) author: Option<Arc<String>>,
	pub(crate) modified_at: DateTime<Utc>,
	pub(crate) thumbnail: Option<Thumbnail>,
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
						Some(Thumbnail { bytes }) => ui.add(egui::Image::new(ImageSource::Bytes {
							uri: format!("bytes://thumbnail_{}.png", card.id.value()).into(),
							bytes: Bytes::Shared(bytes.clone()),
						})),
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
	fn ui<'a>(&'a self) -> BookCardUi<'a> {
		BookCardUi { card: self }
	}
}

pub const LIBRARY_LIST_SIZE: u32 = 5;

pub(crate) struct ListView {
	pub(crate) page: u32,
	pub(crate) cards: [Option<BookCard>; LIBRARY_LIST_SIZE as usize],
}

impl ListView {
	fn draw(&self, ui: &mut egui::Ui, poke_stick: &impl Bell) {
		let height =
			ui.available_height() - (LIBRARY_LIST_SIZE as f32 - 1.0) * ui.spacing().item_spacing.y;
		let card_height = height / LIBRARY_LIST_SIZE as f32;

		ui.vertical(|ui| {
			for card in self.cards.iter().flatten() {
				ui.allocate_ui([ui.available_width(), card_height].into(), |ui| {
					if ui.add(card.ui()).clicked() {
						poke_stick.open_book(card.id);
					}
				});
			}
		});
	}
}

#[derive(Debug)]
pub(crate) struct ToCCard {
	pub(crate) location: Location,
	pub(crate) title: Arc<String>,
}

struct ChapterCardUi<'a> {
	card: &'a ToCCard,
}

impl egui::Widget for ChapterCardUi<'_> {
	fn ui(self, ui: &mut egui::Ui) -> egui::Response {
		let card = self.card;
		ui.group(|ui| {
			ui.set_min_size(ui.available_size());
			let title = card.title.as_ref();
			ui.label(RichText::new(title).text_style(theme::HEADING2.clone()));
			ui.interact(
				ui.min_rect(),
				ui.id().with(card.location.spine),
				egui::Sense::click(),
			)
		})
		.inner
	}
}

impl ToCCard {
	fn ui<'a>(&'a self) -> ChapterCardUi<'a> {
		ChapterCardUi { card: self }
	}
}

pub const TOC_LIST_SIZE: u32 = 12;

#[derive(Default, Debug)]
pub(crate) struct ToCView {
	pub(crate) page: u32,
	pub(crate) cards: [Option<ToCCard>; TOC_LIST_SIZE as usize],
}

impl ToCView {
	fn draw(&self, ui: &mut egui::Ui, poke_stick: &impl Bell) {
		let height =
			ui.available_height() - (TOC_LIST_SIZE as f32 - 1.0) * ui.spacing().item_spacing.y;
		let card_height = height / TOC_LIST_SIZE as f32;

		ui.vertical(|ui| {
			for card in self.cards.iter().flatten() {
				ui.allocate_ui([ui.available_width(), card_height].into(), |ui| {
					if ui.add(card.ui()).clicked() {
						poke_stick.goto_location(card.location);
					}
				});
			}
		});
	}
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum IllustratorInnerView {
	Book,
	ToC(ToCView),
}

pub(crate) struct IllustratorView {
	view: IllustratorInnerView,
	handle: IllustratorHandle,
}

pub trait Bell {
	fn scan_library(&self);

	fn next_page(&self);

	fn previous_page(&self);

	fn goto_location(&self, loc: Location);

	fn open_book(&self, id: BookId);

	fn open_library(&self);

	fn toggle_chapters(&self);
}

pub enum FeatureView {
	Illustrator(IllustratorView),
	List(ListView),
}

impl Default for FeatureView {
	fn default() -> Self {
		FeatureView::List(ListView {
			page: 0,
			cards: Default::default(),
		})
	}
}

#[derive(Default)]
pub struct MainView {
	pub invisible: bool,
	pub working: ScribeState,
	pub menu_open: bool,
	pub feature: FeatureView,
	pub rects: Vec<Rect>,
}

impl MainView {
	pub fn is_inside_ui_element(&self, pos: egui::Pos2) -> bool {
		self.rects.iter().any(|r| r.contains(pos))
	}

	pub(crate) fn open_library(&mut self) {
		self.invisible = false;
		self.feature = FeatureView::List(ListView {
			cards: Default::default(),
			page: 0,
		});
	}

	pub(crate) fn library_loaded(&mut self, scribe: &mut scribe::Scribe) {
		match &mut self.feature {
			FeatureView::Illustrator(_) => {}
			FeatureView::List(list) => {
				let books = scribe.library().books(0..LIBRARY_LIST_SIZE);
				self.working = scribe.poke_list(&books);
				let mut books_iter = books.into_iter().map(|b| {
					let id = b.id;
					(b, scribe.library().thumbnail(id))
				});
				list.cards = array::from_fn(|_| books_iter.next().map(BookCard::new));
				list.page = 0;
			}
		}
	}

	pub(crate) fn book_updated(&mut self, scribe: &scribe::Scribe, id: BookId) {
		match &mut self.feature {
			FeatureView::Illustrator(_) => {}
			FeatureView::List(list) => {
				let card = list.cards.iter_mut().flatten().find(|c| c.id == id);
				if let Some(card) = card
					&& let Some(book) = scribe.library().book(id)
				{
					log::trace!("Update book {id:?}");
					*card = BookCard::new((book, scribe.library().thumbnail(id)));
				}
			}
		}
	}

	pub(crate) fn open_book(&mut self, handle: IllustratorHandle) {
		self.invisible = true;
		self.feature = FeatureView::Illustrator(IllustratorView {
			view: IllustratorInnerView::Book,
			handle,
		});
	}

	pub(crate) fn next_page(&mut self, scribe: &mut scribe::Scribe) {
		match &mut self.feature {
			FeatureView::Illustrator(IllustratorView {
				view: IllustratorInnerView::Book,
				handle,
			}) => {
				if let Err(e) = handle.next_page() {
					log::error!("Illustrator error: {e}");
				}
			}
			FeatureView::Illustrator(IllustratorView {
				view: IllustratorInnerView::ToC(toc_view),
				handle,
			}) => {
				let page = toc_view.page + 1;
				let offset = (page * TOC_LIST_SIZE) as usize;
				let toc = handle.toc.read().unwrap();
				if toc.items.len() > offset {
					let mut item_iter = toc.items.iter().skip(offset);
					for card in toc_view.cards.as_mut() {
						if let Some(item) = item_iter.next() {
							*card = Some(ToCCard {
								location: item.location,
								title: item.title.clone(),
							});
						} else {
							*card = None;
						}
					}
					toc_view.page = page;
				}
			}
			FeatureView::List(list) => {
				let page = list.page + 1;
				let r = (page * LIBRARY_LIST_SIZE)..(page * LIBRARY_LIST_SIZE + LIBRARY_LIST_SIZE);
				let books = scribe.library().books(r);
				if !books.is_empty() {
					self.working = scribe.poke_list(&books);
					let mut books_iter = books.into_iter().map(|b| {
						let id = b.id;
						(b, scribe.library().thumbnail(id))
					});
					list.cards = std::array::from_fn(|_| books_iter.next().map(BookCard::new));
					list.page = page;
				}
			}
		}
	}

	pub(crate) fn previous_page(&mut self, scribe: &mut scribe::Scribe) {
		match &mut self.feature {
			FeatureView::Illustrator(IllustratorView {
				view: IllustratorInnerView::Book,
				handle,
			}) => {
				if let Err(e) = handle.previous_page() {
					log::error!("Illustrator error: {e}");
				}
			}
			FeatureView::Illustrator(IllustratorView {
				view: IllustratorInnerView::ToC(toc_view),
				handle,
			}) => {
				toc_view.page = toc_view.page.saturating_sub(1);
				let offset = toc_view.page * TOC_LIST_SIZE;
				let toc = handle.toc.read().unwrap();
				let mut item_iter = toc.items.iter().skip(offset as usize);
				for card in toc_view.cards.as_mut() {
					if let Some(item) = item_iter.next() {
						*card = Some(ToCCard {
							location: item.location,
							title: item.title.clone(),
						});
					} else {
						*card = None;
					}
				}
			}
			FeatureView::List(list) => {
				let page = list.page.saturating_sub(1);
				let r = (page * LIBRARY_LIST_SIZE)..(page * LIBRARY_LIST_SIZE + LIBRARY_LIST_SIZE);
				let books = scribe.library().books(r);
				self.working = scribe.poke_list(&books);
				let mut books_iter = books.into_iter().map(|b| {
					let id = b.id;
					(b, scribe.library().thumbnail(id))
				});
				list.cards = std::array::from_fn(|_| books_iter.next().map(BookCard::new));
				list.page = page;
			}
		}
	}

	pub(crate) fn goto(&mut self, location: Location) {
		match &mut self.feature {
			FeatureView::Illustrator(illustrator) => {
				if let Err(e) = illustrator.handle.goto(location) {
					log::error!("Illustrator error: {e}");
				}
				illustrator.view = IllustratorInnerView::Book;
			}
			FeatureView::List(_) => {}
		}
	}

	pub(crate) fn toggle_toc(&mut self) {
		if let FeatureView::Illustrator(IllustratorView { view, handle }) = &mut self.feature {
			match view {
				IllustratorInnerView::Book => {
					let loc = *handle.location.read().unwrap();
					let mut toc_view = ToCView::default();
					let toc = handle.toc.read().unwrap();
					let index = toc.items.iter().position(|i| i.location.spine == loc.spine);
					let page = index.map(|index| index as u32 / TOC_LIST_SIZE).unwrap_or(0);
					let offset = page * TOC_LIST_SIZE;

					let mut item_iter = toc.items.iter().skip(offset as usize);
					for card in toc_view.cards.as_mut() {
						if let Some(item) = item_iter.next() {
							*card = Some(ToCCard {
								location: item.location,
								title: item.title.clone(),
							});
						} else {
							*card = None;
						}
					}
					toc_view.page = page;
					*view = IllustratorInnerView::ToC(toc_view);
				}
				IllustratorInnerView::ToC(_) => {
					*view = IllustratorInnerView::Book;
				}
			}
		}
	}

	pub(crate) fn toggle_ui(&mut self) {
		self.invisible = !self.invisible;
	}
}

impl GuiView for MainView {
	fn draw(&mut self, ctx: &Context, poke_stick: &impl Bell) {
		self.rects.clear();
		self.menu_open = false;

		if self.invisible {
			return;
		}

		let top_panel = egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
			egui::MenuBar::new().ui(ui, |ui| {
				let menu = ui.menu_button(UiIcon::new(Icon::Menu).large().build(), |ui| {
					if ui
						.button(UiIcon::new(Icon::Library).text("Library").large().build())
						.clicked()
					{
						poke_stick.open_library();
					}
					if ui
						.button(
							UiIcon::new(Icon::RefreshCw)
								.text("Rescan library")
								.large()
								.build(),
						)
						.clicked()
					{
						poke_stick.scan_library();
					}
					if ui
						.button(UiIcon::new(Icon::DoorOpen).text("Quit").large().build())
						.clicked()
					{
						ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
					}
				});
				if menu.response.context_menu_opened() {
					self.rects.push(ctx.screen_rect());
					self.menu_open = true;
				}

				ui.label(RichText::new("Scribble reader").size(theme::L_SIZE));

				ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
					if matches!(self.working, ScribeState::Working) {
						ui.label(
							UiIcon::new(Icon::RefreshCw)
								.color(Color32::GRAY)
								.large()
								.build(),
						);
					}
				});
			});
		});
		self.rects.push(top_panel.response.interact_rect);

		let bottom_panel = egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
			if self.menu_open {
				ui.disable();
			}
			ui.vertical(|ui| {
				ui.add_space(5.0);
				ui.horizontal(|ui| {
					ui.columns(7, |columns| {
						columns[1].with_layout(
							Layout::centered_and_justified(egui::Direction::LeftToRight),
							|ui| {
								let width = ui.available_width();
								ui.set_height(width * 0.5);
								if ui
									.button(UiIcon::new(Icon::MoveLeft).xlarge().build())
									.clicked()
								{
									poke_stick.previous_page();
								}
							},
						);
						if let FeatureView::Illustrator(ref illustrator) = self.feature {
							let book_open = matches!(illustrator.view, IllustratorInnerView::Book);
							columns[3].with_layout(
								Layout::centered_and_justified(egui::Direction::RightToLeft),
								|ui| {
									ui.set_height(ui.available_width() * 0.5);
									if ui
										.button(
											UiIcon::new(Icon::ListTree)
												.xlarge()
												.color(if book_open {
													Color32::BLACK
												} else {
													theme::ACCENT_COLOR
												})
												.build(),
										)
										.clicked()
									{
										poke_stick.toggle_chapters();
									}
								},
							);
						}
						columns[5].with_layout(
							Layout::centered_and_justified(egui::Direction::RightToLeft),
							|ui| {
								ui.set_height(ui.available_width() * 0.5);
								if ui
									.button(UiIcon::new(Icon::MoveRight).xlarge().build())
									.clicked()
								{
									poke_stick.next_page();
								}
							},
						);
					});
				});
				ui.add_space(3.0);
			});
		});
		self.rects.push(bottom_panel.response.interact_rect);

		match &self.feature {
			FeatureView::Illustrator(IllustratorView {
				view: IllustratorInnerView::Book,
				..
			}) => {}
			FeatureView::Illustrator(IllustratorView {
				view: IllustratorInnerView::ToC(chapters),
				..
			}) => {
				let central_panel = egui::CentralPanel::default().show(ctx, |ui| {
					if self.menu_open {
						ui.disable();
					}
					chapters.draw(ui, poke_stick)
				});
				self.rects.push(central_panel.response.interact_rect);
			}
			FeatureView::List(list) => {
				let central_panel = egui::CentralPanel::default().show(ctx, |ui| {
					if self.menu_open {
						ui.disable();
					}
					list.draw(ui, poke_stick)
				});
				self.rects.push(central_panel.response.interact_rect);
			}
		}
	}
}

struct UiIcon<'a> {
	color: Color32,
	icon_font: FontId,
	icon: Icon,
	text_font: FontId,
	text: Option<&'a str>,
}

impl UiIcon<'_> {
	fn new(icon: Icon) -> Self {
		UiIcon {
			color: Color32::BLACK,
			icon_font: theme::ICON_FONT.clone(),
			icon,
			text_font: FontId::new(theme::DEFAULT_SIZE, FontFamily::Proportional),
			text: None,
		}
	}

	fn color(self, color: Color32) -> Self {
		Self { color, ..self }
	}

	fn text<'a>(self, text: &'a str) -> UiIcon<'a> {
		UiIcon {
			text: Some(text),
			..self
		}
	}

	fn size(self, size: f32) -> Self {
		Self {
			icon_font: FontId::new(size, theme::ICON_FONT_FAMILY.clone()),
			text_font: FontId::new(size, FontFamily::Proportional),
			..self
		}
	}

	fn large(self) -> Self {
		Self {
			icon_font: theme::ICON_L_FONT.clone(),
			text_font: FontId::new(theme::L_SIZE, FontFamily::Proportional),
			..self
		}
	}

	fn xlarge(self) -> Self {
		Self {
			icon_font: theme::ICON_XL_FONT.clone(),
			text_font: FontId::new(theme::XL_SIZE, FontFamily::Proportional),
			..self
		}
	}

	fn build(self) -> egui::text::LayoutJob {
		let mut job = LayoutJob::default();
		let mut char_buf = [0; 4];
		job.append(
			self.icon.unicode().encode_utf8(&mut char_buf),
			0.0,
			TextFormat {
				font_id: self.icon_font,
				color: self.color,
				..Default::default()
			},
		);
		if let Some(text) = self.text {
			job.append(
				text,
				5.0,
				TextFormat {
					font_id: self.text_font,
					color: self.color,
					..Default::default()
				},
			);
		}
		job
	}
}
