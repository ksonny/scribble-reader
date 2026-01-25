mod active_areas;

use std::sync::Arc;

use egui::Rect;
use egui::RichText;
use illustrator::IllustratorHandle;
use lucide_icons::Icon;
use scribe::library::Location;

use crate::AppBell;
use crate::AppEvent;
use crate::gestures::Direction;
use crate::gestures::Gesture;
use crate::gestures::GestureEvent;
use crate::renderer::Painter;
use crate::renderer::pixmap_renderer;
use crate::ui::MainMenuBar;
use crate::ui::MenuItem;
use crate::ui::OnAction;
use crate::ui::ToolBar;
use crate::ui::ToolItem;
use crate::ui::theme;
use crate::views::EventResult;
use crate::views::GestureResult;
use crate::views::ViewHandle;
use crate::views::reader::active_areas::ActiveAreaAction;
use crate::views::reader::active_areas::ActiveAreas;

pub const CHAPTER_LIST_SIZE: u32 = 12;

enum ReaderMode {
	ReadNoUi,
	Read,
	Chapters,
	Settings,
}

#[derive(Debug)]
pub(crate) struct ChapterCard {
	location: Location,
	title: Arc<String>,
}

pub(crate) struct ReaderView {
	bell: AppBell,
	illustrator: IllustratorHandle,
	screen_width: u32,
	screen_height: u32,
	scale_factor: f32,
	mode: ReaderMode,
	rects: Vec<Rect>,
	page: u32,
	cards: [Option<ChapterCard>; CHAPTER_LIST_SIZE as usize],
}

impl ReaderView {
	pub(crate) fn create(
		bell: AppBell,
		illustrator: IllustratorHandle,
		screen_width: u32,
		screen_height: u32,
		scale_factor: f32,
	) -> Self {
		Self {
			bell,
			illustrator,
			screen_width,
			screen_height,
			scale_factor,
			mode: ReaderMode::ReadNoUi,
			rects: Vec::new(),
			page: 0,
			cards: Default::default(),
		}
	}

	fn toggle_ui(&mut self) {
		if matches!(self.mode, ReaderMode::ReadNoUi) {
			self.mode = ReaderMode::Read;
		} else if matches!(self.mode, ReaderMode::Read) {
			self.mode = ReaderMode::ReadNoUi;
		}
	}

	fn toggle_chapters(&mut self) {
		if matches!(self.mode, ReaderMode::Chapters) {
			self.mode = ReaderMode::Read;
		} else {
			let loc = self.illustrator.location();
			let chapters = self.illustrator.toc.read().unwrap();
			let index = chapters
				.items
				.iter()
				.position(|i| i.location.spine == loc.spine);
			let page = index
				.map(|index| index as u32 / CHAPTER_LIST_SIZE)
				.unwrap_or(0);
			let offset = page * CHAPTER_LIST_SIZE;

			let mut item_iter = chapters.items.iter().skip(offset as usize);
			for card in self.cards.as_mut() {
				if let Some(item) = item_iter.next() {
					*card = Some(ChapterCard {
						location: item.location,
						title: item.title.clone(),
					});
				} else {
					*card = None;
				}
			}
			self.page = page;
			self.mode = ReaderMode::Chapters;
		}
	}

	fn toggle_settings(&mut self) {
		if matches!(self.mode, ReaderMode::Settings) {
			self.mode = ReaderMode::Read;
		} else {
			self.mode = ReaderMode::Settings;
		}
	}

	fn prev_page(&mut self) {
		match self.mode {
			ReaderMode::Read | ReaderMode::ReadNoUi => {
				let _ = self
					.illustrator
					.previous_page()
					.inspect_err(|err| log::error!("Previous page error: {err}"));
			}
			ReaderMode::Chapters => {
				self.page = self.page.saturating_sub(1);
				let offset = self.page * CHAPTER_LIST_SIZE;
				let chapters = self.illustrator.toc.read().unwrap();
				let mut item_iter = chapters.items.iter().skip(offset as usize);
				for card in self.cards.as_mut() {
					if let Some(item) = item_iter.next() {
						*card = Some(ChapterCard {
							location: item.location,
							title: item.title.clone(),
						});
					} else {
						*card = None;
					}
				}
			}
			ReaderMode::Settings => {}
		};
	}

	fn next_page(&mut self) {
		match self.mode {
			ReaderMode::Read | ReaderMode::ReadNoUi => {
				let _ = self
					.illustrator
					.next_page()
					.inspect_err(|err| log::error!("Next page error: {err}"));
			}
			ReaderMode::Chapters => {
				let page = self.page + 1;
				let offset = (page * CHAPTER_LIST_SIZE) as usize;
				let chapters = self.illustrator.toc.read().unwrap();
				if chapters.items.len() > offset {
					let mut item_iter = chapters.items.iter().skip(offset);
					for card in self.cards.as_mut() {
						if let Some(item) = item_iter.next() {
							*card = Some(ChapterCard {
								location: item.location,
								title: item.title.clone(),
							});
						} else {
							*card = None;
						}
					}
					self.page = page;
				}
			}
			ReaderMode::Settings => {}
		};
	}
}

#[derive(Clone, Copy)]
enum MenuAction {
	Library,
	Exit,
}

#[derive(Clone, Copy)]
enum ToolAction {
	Prev,
	Next,
	Chapters,
	Settings,
}

impl OnAction<MenuAction> for ReaderView {
	fn on_action(&mut self, action: MenuAction) {
		match action {
			MenuAction::Library => {
				self.bell.send_event(AppEvent::OpenLibrary);
			}
			MenuAction::Exit => {
				self.bell.send_event(AppEvent::Exit);
			}
		}
	}
}

impl OnAction<ToolAction> for ReaderView {
	fn on_action(&mut self, action: ToolAction) {
		match action {
			ToolAction::Prev => self.prev_page(),
			ToolAction::Next => self.next_page(),
			ToolAction::Chapters => self.toggle_chapters(),
			ToolAction::Settings => self.toggle_settings(),
		}
	}
}

impl ViewHandle for ReaderView {
	fn draw(&mut self, painter: Painter<'_>) {
		self.rects.clear();

		let painter = if matches!(self.mode, ReaderMode::Read | ReaderMode::ReadNoUi) {
			let loc = self.illustrator.location();
			let cache = self.illustrator.cache();
			let content = cache.page(loc);
			if let Some(content) = content {
				let text_areas = content.items.iter().filter_map(|it| match it {
					illustrator::DisplayItem {
						pos,
						content: illustrator::DisplayContent::Text(item),
						..
					} => Some(glyphon::TextArea {
						buffer: &item.buffer,
						left: pos.x as f32,
						top: pos.y as f32,
						scale: 1.0,
						bounds: glyphon::TextBounds::default(),
						default_color: glyphon::Color::rgb(0, 0, 0),
						custom_glyphs: &[],
					}),
					_ => None,
				});
				let pixmaps = content.items.iter().filter_map(|it| match it {
					illustrator::DisplayItem {
						pos,
						size,
						content: illustrator::DisplayContent::Pixmap(item),
					} => Some(pixmap_renderer::PixmapInput {
						pixmap_rgba: &item.pixmap_rgba,
						pixmap_width: item.pixmap_width,
						pixmap_height: item.pixmap_height,
						targets: vec![pixmap_renderer::PixmapTargetInput {
							pos: [pos.x, pos.y],
							dim: [size.width, size.height],
							tex_pos: [0; 2],
							tex_dim: [item.pixmap_width, item.pixmap_height],
						}],
					}),
					_ => None,
				});

				let mut font_system = self.illustrator.font_system.lock().unwrap();
				painter
					.draw_glyphon(&mut font_system, text_areas)
					.draw_pixmap(pixmaps)
			} else {
				let mut font_system = self.illustrator.font_system.lock().unwrap();
				painter
					.draw_glyphon(&mut font_system, [].into_iter())
					.draw_pixmap([].into_iter())
			}
		} else {
			let mut font_system = self.illustrator.font_system.lock().unwrap();
			painter
				.draw_glyphon(&mut font_system, [].into_iter())
				.draw_pixmap([].into_iter())
		};

		painter.draw_ui(|ctx| {
			if matches!(self.mode, ReaderMode::ReadNoUi) {
				return;
			}

			let menu_items = &[
				MenuItem {
					icon: lucide_icons::Icon::Library,
					description: "Library",
					active: false,
					action: MenuAction::Library,
				},
				MenuItem {
					icon: lucide_icons::Icon::LogOut,
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
				Some(ToolItem {
					icon: Icon::ListTree,
					description: "Chapters",
					active: matches!(self.mode, ReaderMode::Chapters),
					action: ToolAction::Chapters,
				}),
				Some(ToolItem {
					icon: Icon::Cog,
					description: "Settings",
					active: matches!(self.mode, ReaderMode::Settings),
					action: ToolAction::Settings,
				}),
				None,
				Some(ToolItem {
					icon: Icon::ArrowRight,
					description: "Next",
					active: false,
					action: ToolAction::Next,
				}),
				None,
			];

			let top_panel = egui::TopBottomPanel::top("top")
				.show(ctx, |ui| MainMenuBar::new(self, menu_items, false).ui(ui));
			let is_open = top_panel.inner.context_menu_opened();
			if !is_open {
				self.rects.push(top_panel.response.interact_rect);
			} else {
				self.rects.push(ctx.screen_rect())
			}

			let bottom_panel = egui::TopBottomPanel::bottom("bottom")
				.show(ctx, |ui| ToolBar::new(self, tool_items, is_open).ui(ui));
			if !is_open {
				self.rects.push(bottom_panel.response.interact_rect);
			}

			if matches!(self.mode, ReaderMode::Chapters) {
				let central_panel = egui::CentralPanel::default().show(ctx, |ui| {
					if is_open {
						ui.disable();
					}

					let height = ui.available_height()
						- (CHAPTER_LIST_SIZE as f32 - 1.0) * ui.spacing().item_spacing.y;
					let card_height = height / CHAPTER_LIST_SIZE as f32;

					ui.vertical(|ui| {
						let mut untoggle = false;
						for card in self.cards.iter().flatten() {
							ui.allocate_ui([ui.available_width(), card_height].into(), |ui| {
								if ui.add(card.ui()).clicked() {
									let _ = self
										.illustrator
										.goto(card.location)
										.inspect_err(|err| log::error!("Goto error: {err}"));
									untoggle = true;
								}
							});
						}
						if untoggle {
							self.toggle_chapters();
						}
					});
				});
				if !is_open {
					self.rects.push(central_panel.response.interact_rect);
				}
			} else if matches!(self.mode, ReaderMode::Settings) {
				let central_panel = egui::CentralPanel::default().show(ctx, |ui| {
					if is_open {
						ui.disable();
					}

					// TODO
				});
				if !is_open {
					self.rects.push(central_panel.response.interact_rect);
				}
			}
		});
	}

	fn event(&mut self, event: &AppEvent) -> EventResult {
		if let AppEvent::BookContentReady(..) = event {
			EventResult::RequestRedraw
		} else {
			EventResult::None
		}
	}

	fn gesture(&mut self, event: &GestureEvent) -> GestureResult {
		match event.gesture {
			Gesture::Tap => {
				let pos = egui::pos2(event.loc.x as f32, event.loc.y as f32) / self.scale_factor;
				if self.rects.iter().any(|r| r.contains(pos)) {
					GestureResult::Unhandled
				} else {
					let areas = ActiveAreas::new(self.screen_width, self.screen_height);
					if let Some(action) = areas.action(event.loc) {
						match action {
							ActiveAreaAction::ToggleUi => self.toggle_ui(),
							ActiveAreaAction::PreviousPage => self.prev_page(),
							ActiveAreaAction::NextPage => self.next_page(),
						};
						GestureResult::Consumed
					} else {
						GestureResult::Unhandled
					}
				}
			}
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

	fn rescale(&mut self, scale_factor: f32) {
		self.scale_factor = scale_factor;
	}

	fn resize(&mut self, width: u32, height: u32) {
		self.screen_width = width;
		self.screen_height = height;
	}
}

impl ChapterCard {
	fn ui<'a>(&'a self) -> ChapterCardUi<'a> {
		ChapterCardUi { card: self }
	}
}

pub(crate) struct ChapterCardUi<'a> {
	card: &'a ChapterCard,
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
