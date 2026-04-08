mod active_areas;

use std::fmt::Write;
use std::sync::Arc;

use egui::Color32;
use egui::Rect;
use egui::RichText;
use illustrator::IllustratorAssistant;
use illustrator::IllustratorCreateError;
use illustrator::IllustratorRequestError;
use illustrator::create_illustrator;
use lucide_icons::Icon;
use scribe::BookId;
use scribe::Location;
use scribe::RecordKeeper;
use scribe::RecordKeeperAssistant;
use scribe::RecordKeeperError;
use scribe::config::IllustratorConfig;
use sculpter::SculpterFonts;
use serde::Deserialize;
use serde::Serialize;
use wrangler::content::ContentWranglerAssistant;

use crate::AppBell;
use crate::AppEvent;
use crate::gestures::Direction;
use crate::gestures::Gesture;
use crate::gestures::GestureEvent;
use crate::renderer::Painter;
use crate::renderer::pixmap_renderer;
use crate::renderer::pixmap_renderer::PixmapTargetInput;
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
use crate::views::Viewport;
use crate::views::reader::active_areas::ActiveAreaAction;
use crate::views::reader::active_areas::ActiveAreas;

pub const CHAPTER_LIST_SIZE: u32 = 12;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReaderViewCreateError {
	#[error(transparent)]
	Illustrator(#[from] ReaderIllustratorError),
	#[error(transparent)]
	RecordKeeper(#[from] RecordKeeperError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReaderIllustratorError {
	#[error(transparent)]
	IllustratorCreate(#[from] IllustratorCreateError),
	#[error(transparent)]
	IllustratorRequest(#[from] IllustratorRequestError),
	#[error(transparent)]
	RecordKeeper(#[from] RecordKeeperError),
}

enum EditState {
	Unchanged,
	Changed,
}

enum ReaderMode {
	ReadNoUi,
	Read,
	Navigation,
	Settings(EditState),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ViewState {
	profile: String,
}

impl Default for ViewState {
	fn default() -> Self {
		Self {
			profile: "serif".to_string(),
		}
	}
}

pub(crate) struct ReaderView {
	config: IllustratorConfig,
	keeper: RecordKeeper,
	fonts: SculpterFonts,
	content: ContentWranglerAssistant,
	bell: AppBell,
	book_id: BookId,

	records: RecordKeeperAssistant,
	state: ViewState,
	viewport: Viewport,

	illustrator: Option<IllustratorAssistant>,
	mode: ReaderMode,
	active_rects: Vec<Rect>,
	chapters_page: u32,
	chapters_cards: [Option<ChapterCard>; CHAPTER_LIST_SIZE as usize],
	statusline: Option<String>,
}

impl ReaderView {
	const STATE_KEY: &str = "reader_view_state";

	pub(crate) fn create(
		config: IllustratorConfig,
		keeper: RecordKeeper,
		fonts: SculpterFonts,
		content: ContentWranglerAssistant,
		bell: AppBell,
		book_id: BookId,
		viewport: Viewport,
	) -> Result<Self, ReaderViewCreateError> {
		let records = keeper.assistant()?;
		let state: ViewState = records
			.fetch_view_state(Self::STATE_KEY)?
			.unwrap_or_default();

		let mut view = Self {
			config,
			keeper,
			fonts,
			content,
			bell,
			book_id,

			records,
			state,
			viewport,

			illustrator: None,
			mode: ReaderMode::ReadNoUi,
			active_rects: Vec::new(),
			chapters_page: 0,
			chapters_cards: Default::default(),
			statusline: String::new().into(),
		};

		view.create_illustrator()?;

		Ok(view)
	}

	fn create_illustrator(&mut self) -> Result<(), ReaderIllustratorError> {
		if let Some(illustrator) = self.illustrator.take() {
			log::debug!("Stop running illustrator");
			illustrator.shutdown();
		}
		let profile = self
			.config
			.as_ref()
			.get(&self.state.profile)
			.cloned()
			.unwrap_or_default();
		log::debug!("Create with profile {}", &self.state.profile);
		let illustrator = create_illustrator(
			self.keeper.assistant()?,
			self.fonts.clone(),
			self.content.clone(),
			self.bell.clone(),
			profile,
			self.book_id,
		)?;
		illustrator.rescale(self.viewport.scale_factor)?;
		illustrator.resize(self.viewport.screen_width, self.viewport.screen_height)?;

		self.illustrator = Some(illustrator);

		Ok(())
	}

	fn toggle_ui(&mut self) {
		if matches!(self.mode, ReaderMode::ReadNoUi) {
			self.mode = ReaderMode::Read;
		} else if matches!(self.mode, ReaderMode::Read) {
			self.mode = ReaderMode::ReadNoUi;
		}
	}

	fn toggle_chapters(&mut self) {
		if matches!(self.mode, ReaderMode::Navigation) {
			self.mode = ReaderMode::Read;
		} else {
			let illustrator = self.illustrator.as_ref().expect("Illustrator not running");
			let state = illustrator.state();
			let navigation = illustrator.navigation();
			let nav_points = navigation
				.as_ref()
				.map(|n| n.nav_points.as_slice())
				.unwrap_or_default();

			let page = state.location.spine / CHAPTER_LIST_SIZE;
			let offset = page * CHAPTER_LIST_SIZE;
			let mut item_iter = nav_points.iter().skip(offset as usize);
			for card in self.chapters_cards.as_mut() {
				if let Some(item) = item_iter.next() {
					*card = Some(ChapterCard {
						location: Location::from_spine(item.spine.unwrap_or_default()),
						title: item.title.clone(),
					});
				} else {
					*card = None;
				}
			}
			self.chapters_page = page;
			self.mode = ReaderMode::Navigation;
		}
	}

	fn toggle_settings(&mut self) {
		match self.mode {
			ReaderMode::Read => {
				self.mode = ReaderMode::Settings(EditState::Unchanged);
			}
			ReaderMode::Settings(EditState::Changed) => {
				if let Err(e) = self.create_illustrator() {
					log::error!("Failed to create illustrator: {e}");
				}
				if let Err(e) = self.records.record_view_state(Self::STATE_KEY, &self.state) {
					log::warn!("Error saving state: {e}");
				}
				self.mode = ReaderMode::Read;
			}
			ReaderMode::Settings(EditState::Unchanged) => {
				self.mode = ReaderMode::Read;
			}
			_ => {}
		}
	}

	fn prev_page(&mut self) {
		match self.mode {
			ReaderMode::Read | ReaderMode::ReadNoUi => {
				let illustrator = self.illustrator.as_mut().expect("Illustrator not running");
				let _ = illustrator
					.previous_page()
					.inspect_err(|err| log::error!("Previous page error: {err}"));
			}
			ReaderMode::Navigation => {
				self.chapters_page = self.chapters_page.saturating_sub(1);
				let offset = self.chapters_page * CHAPTER_LIST_SIZE;

				let illustrator = self.illustrator.as_ref().expect("Illustrator not running");
				let navigation = illustrator.navigation();
				let nav_points = navigation
					.as_ref()
					.map(|n| n.nav_points.as_slice())
					.unwrap_or_default();

				let mut item_iter = nav_points.iter().skip(offset as usize);
				for card in self.chapters_cards.as_mut() {
					if let Some(item) = item_iter.next() {
						*card = Some(ChapterCard {
							location: Location::from_spine(item.spine.unwrap_or_default()),
							title: item.title.clone(),
						});
					} else {
						*card = None;
					}
				}
			}
			ReaderMode::Settings(_) => {}
		};
	}

	fn next_page(&mut self) {
		match self.mode {
			ReaderMode::Read | ReaderMode::ReadNoUi => {
				let illustrator = self.illustrator.as_mut().expect("Illustrator not running");
				let _ = illustrator
					.next_page()
					.inspect_err(|err| log::error!("Next page error: {err}"));
			}
			ReaderMode::Navigation => {
				let page = self.chapters_page + 1;
				let offset = (page * CHAPTER_LIST_SIZE) as usize;

				let illustrator = self.illustrator.as_ref().expect("Illustrator not running");
				let navigation = illustrator.navigation();
				let nav_points = navigation
					.as_ref()
					.map(|n| n.nav_points.as_slice())
					.unwrap_or_default();

				if nav_points.len() > offset {
					let mut item_iter = nav_points.iter().skip(offset);
					for card in self.chapters_cards.as_mut() {
						if let Some(item) = item_iter.next() {
							*card = Some(ChapterCard {
								location: Location::from_spine(item.spine.unwrap_or_default()),
								title: item.title.clone(),
							});
						} else {
							*card = None;
						}
					}
					self.chapters_page = page;
				}
			}
			ReaderMode::Settings(_) => {}
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
		self.active_rects.clear();

		let mut statusline = self.statusline.take().unwrap_or_default();
		statusline.clear();
		let illustrator = self.illustrator.as_ref().expect("Illustrator not running");

		let painter = if matches!(self.mode, ReaderMode::Read | ReaderMode::ReadNoUi) {
			let state = illustrator.state();
			let cache = illustrator.cache();
			let page = cache.page(state.location);
			if let Some((content, meta)) = page {
				let _ = write!(
					&mut statusline,
					"Chapter {} / {} Book {}%",
					meta.page, meta.pages, state.percent_read
				);

				let mut glyph_targets = Vec::new();
				for item in &content.items {
					if let illustrator::DisplayItem {
						pos,
						content: illustrator::DisplayContent::Text(block),
						..
					} = item
					{
						glyph_targets.extend(block.glyphs.iter().map(|g| PixmapTargetInput {
							pos: [pos.x + g.pos[0], pos.y + g.pos[1]],
							dim: g.size,
							tex_pos: g.uv_pos,
							tex_dim: g.uv_size,
						}));
					}
				}
				let atlas_pixmap = if !glyph_targets.is_empty() {
					// TODO: Allow texture reuse in pixmap renderer
					let atlas = cache.atlas();
					Some(pixmap_renderer::PixmapInput {
						pixmap: pixmap_renderer::Pixmap::Luma(atlas.as_raw()),
						pixmap_dim: [atlas.width(), atlas.height()],
						offset_pos: [0; 2],
						targets: glyph_targets,
					})
				} else {
					None
				};

				let pixmaps = content.items.iter().filter_map(|it| match it {
					illustrator::DisplayItem {
						pos,
						size,
						content: illustrator::DisplayContent::Pixmap(item),
					} => Some(pixmap_renderer::PixmapInput {
						pixmap: pixmap_renderer::Pixmap::RgbA(&item.pixmap_rgba),
						pixmap_dim: [item.pixmap_width, item.pixmap_height],
						offset_pos: [0; 2],
						targets: vec![pixmap_renderer::PixmapTargetInput {
							pos: [pos.x, pos.y],
							dim: [size.width, size.height],
							tex_pos: [0; 2],
							tex_dim: [item.pixmap_width, item.pixmap_height],
						}],
					}),
					_ => None,
				});

				painter.draw_pixmap(pixmaps.chain(atlas_pixmap))
			} else {
				painter.draw_pixmap([].into_iter())
			}
		} else {
			painter.draw_pixmap([].into_iter())
		};

		let working = illustrator.working();
		painter.draw_ui(|ui| {
			if matches!(self.mode, ReaderMode::ReadNoUi) {
				return;
			}

			let menu_items = &[
				MenuItem {
					icon: Icon::Library,
					description: "Library",
					active: false,
					action: MenuAction::Library,
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
				Some(ToolItem {
					icon: Icon::ListTree,
					description: "Chapters",
					active: matches!(self.mode, ReaderMode::Navigation),
					action: ToolAction::Chapters,
				}),
				Some(ToolItem {
					icon: Icon::Cog,
					description: "Settings",
					active: matches!(self.mode, ReaderMode::Settings(_)),
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

			let top_panel = egui::Panel::top("top").show_inside(ui, |ui| {
				MainMenuBar::new(self, menu_items)
					.with_loading(working)
					.with_status(Some(&statusline))
					.ui(ui)
			});
			let is_open = top_panel.inner.context_menu_opened();
			if !is_open {
				self.active_rects.push(top_panel.response.interact_rect);
			} else {
				self.active_rects.push(ui.content_rect())
			}

			let bottom_panel = egui::Panel::bottom("bottom")
				.show_inside(ui, |ui| ToolBar::new(self, tool_items, is_open).ui(ui));
			if !is_open {
				self.active_rects.push(bottom_panel.response.interact_rect);
			}

			if matches!(self.mode, ReaderMode::Navigation) {
				let central_panel = egui::CentralPanel::default().show_inside(ui, |ui| {
					if is_open {
						ui.disable();
					}

					let height = ui.available_height()
						- (CHAPTER_LIST_SIZE as f32 - 1.0) * ui.spacing().item_spacing.y;
					let card_height = height / CHAPTER_LIST_SIZE as f32;

					ui.vertical(|ui| {
						let mut untoggle = false;
						for card in self.chapters_cards.iter().flatten() {
							ui.allocate_ui([ui.available_width(), card_height].into(), |ui| {
								if ui.add(card.ui()).clicked() {
									let illustrator =
										self.illustrator.as_mut().expect("Illustrator not running");
									let _ = illustrator
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
					self.active_rects.push(central_panel.response.interact_rect);
				}
			} else if matches!(self.mode, ReaderMode::Settings(_)) {
				let central_panel = egui::CentralPanel::default().show_inside(ui, |ui| {
					if is_open {
						ui.disable();
					}
					ui.vertical_centered_justified(|ui| {
						ui.spacing_mut().item_spacing.y = 12.;
						ui.spacing_mut().button_padding.y = 6.;
						ui.heading("Profiles");
						for (profile, _) in self.config.as_ref().iter() {
							let is_active = self.state.profile.as_str() == profile;
							if ui
								.button(
									UiIcon::new(Icon::FileSliders)
										.large()
										.text(profile)
										.color(if is_active {
											theme::ACCENT_COLOR
										} else {
											Color32::BLACK
										})
										.build(),
								)
								.clicked()
							{
								self.mode = ReaderMode::Settings(EditState::Changed);
								self.state.profile = profile.clone();
							}
						}
					})
				});
				if !is_open {
					self.active_rects.push(central_panel.response.interact_rect);
				}
			}
		});

		self.statusline = Some(statusline);
	}

	fn event(&mut self, event: &AppEvent) -> EventResult {
		match event {
			AppEvent::BookContentReady(..) => EventResult::RequestRedraw,
			AppEvent::NavigateNext => {
				self.next_page();
				EventResult::None
			}
			AppEvent::NavigatePrevious => {
				self.prev_page();
				EventResult::None
			}
			_ => EventResult::None,
		}
	}

	fn gesture(&mut self, event: &GestureEvent) -> GestureResult {
		match event.gesture {
			Gesture::Tap => {
				let pos =
					egui::pos2(event.loc.x as f32, event.loc.y as f32) / self.viewport.scale_factor;
				if self.active_rects.iter().any(|r| r.contains(pos)) {
					GestureResult::Unhandled
				} else {
					let areas =
						ActiveAreas::new(self.viewport.screen_width, self.viewport.screen_height);
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
		self.viewport.scale_factor = scale_factor;
		let illustrator = self.illustrator.as_ref().expect("Illustrator not running");
		let _ = illustrator.rescale(scale_factor);
	}

	fn resize(&mut self, width: u32, height: u32) {
		self.viewport.screen_width = width;
		self.viewport.screen_height = height;
		let illustrator = self.illustrator.as_ref().expect("Illustrator not running");
		let _ = illustrator.resize(width, height);
	}

	fn close(&mut self) {
		if let Some(illustrator) = self.illustrator.take() {
			illustrator.shutdown();
		}
	}
}

#[derive(Debug)]
pub(crate) struct ChapterCard {
	location: Location,
	title: Arc<String>,
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
