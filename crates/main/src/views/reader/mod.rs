mod active_areas;

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Display;
use std::fmt::Write;
use std::sync::Arc;

use egui::Button;
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
use sculpter::AtlasVersion;
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
use crate::renderer::pixmap_renderer::PixmapData;
use crate::renderer::pixmap_renderer::PixmapInstance;
use crate::renderer::pixmap_renderer::PixmapRef;
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

#[derive(PartialEq, Eq)]
enum ReaderMode {
	ReadNoUi,
	Read,
	Navigation,
	ActionSettings,
	ProfileSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Action {
	None,
	Next,
	Previous,
	Chapters,
	Settings,
	Library,
	Exit,
}

impl Display for Action {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Action::None => write!(f, "None"),
			Action::Next => write!(f, "Next"),
			Action::Previous => write!(f, "Previous"),
			Action::Chapters => write!(f, "Chapters"),
			Action::Settings => write!(f, "Settings"),
			Action::Library => write!(f, "Library"),
			Action::Exit => write!(f, "Exit"),
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ViewState {
	profile: String,

	swipe_right: Action,
	swipe_down: Action,
	swipe_left: Action,
	swipe_up: Action,
	action_key_up: Action,
	action_key_down: Action,
	action_area_right: Action,
	action_area_left: Action,

	testing: bool,
}

impl Default for ViewState {
	fn default() -> Self {
		Self {
			profile: "serif".to_string(),

			swipe_right: Action::Previous,
			swipe_down: Action::None,
			swipe_left: Action::Next,
			swipe_up: Action::None,
			action_key_up: Action::Next,
			action_key_down: Action::Previous,
			action_area_right: Action::Next,
			action_area_left: Action::Previous,

			testing: true,
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
	viewport: Viewport,
	state: ViewState,

	illustrator: Option<IllustratorAssistant>,
	mode: ReaderMode,
	active_rects: Vec<Rect>,
	chapters_page: u32,
	chapters_cards: [Option<ChapterCard>; CHAPTER_LIST_SIZE as usize],
	statusline: Option<String>,

	atlas_pixmap: Option<(PixmapRef, AtlasVersion)>,
	content_pixmaps: BTreeMap<u64, PixmapRef>,
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
			.fetch_view_state(Self::STATE_KEY)
			.inspect_err(|e| log::warn!("Error fetching state: {e}"))
			.unwrap_or_default()
			.unwrap_or_default();

		let mut view = Self {
			config,
			keeper,
			fonts,
			content,
			bell,
			book_id,

			records,
			viewport,
			state,

			illustrator: None,
			mode: ReaderMode::ReadNoUi,
			active_rects: Vec::new(),
			chapters_page: 0,
			chapters_cards: Default::default(),
			statusline: String::new().into(),

			atlas_pixmap: None,
			content_pixmaps: BTreeMap::new(),
		};

		view.create_illustrator()?;

		Ok(view)
	}

	fn create_illustrator(&mut self) -> Result<(), ReaderIllustratorError> {
		if let Some(illustrator) = self.illustrator.take() {
			log::debug!("Stop running illustrator");
			illustrator.shutdown();
			self.atlas_pixmap.take();
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
						active: item.spine.is_some_and(|s| s == state.location.spine),
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
				self.mode = ReaderMode::ActionSettings;
			}
			ReaderMode::ActionSettings => {
				self.mode = ReaderMode::Read;
			}
			ReaderMode::ProfileSettings => {
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
				let state = illustrator.state();
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
							active: item.spine.is_some_and(|s| s == state.location.spine),
						});
					} else {
						*card = None;
					}
				}
			}
			ReaderMode::ActionSettings => {}
			ReaderMode::ProfileSettings => {}
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
				let state = illustrator.state();
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
								active: item.spine.is_some_and(|s| s == state.location.spine),
							});
						} else {
							*card = None;
						}
					}
					self.chapters_page = page;
				}
			}
			ReaderMode::ActionSettings => {}
			ReaderMode::ProfileSettings => {}
		};
	}
}

impl OnAction<Action> for ReaderView {
	fn on_action(&mut self, action: Action) {
		match action {
			Action::None => {}
			Action::Library => {
				self.bell.send_event(AppEvent::OpenLibrary);
			}
			Action::Exit => {
				self.bell.send_event(AppEvent::Exit);
			}
			Action::Previous => self.prev_page(),
			Action::Next => self.next_page(),
			Action::Chapters => self.toggle_chapters(),
			Action::Settings => self.toggle_settings(),
		}
	}
}

impl ViewHandle for ReaderView {
	fn draw(&mut self, painter: Painter<'_>) {
		self.active_rects.clear();

		let mut statusline = self.statusline.take().unwrap_or_default();
		statusline.clear();
		let illustrator = self.illustrator.as_ref().expect("Illustrator not running");

		let painter = painter.draw_pixmap(|brush| {
			if !matches!(self.mode, ReaderMode::Read | ReaderMode::ReadNoUi) {
				return;
			}

			let state = illustrator.state();
			let cache = illustrator.cache();
			let page = cache.page(state.location);
			let Some((content, meta)) = page else {
				return;
			};
			let _ = write!(
				&mut statusline,
				"Chapter {} / {} Book {}%",
				meta.page, meta.pages, state.percent_read
			);

			let atlas = cache.atlas();
			let pixmap_dims = [atlas.width(), atlas.height()].into();
			let pixmap_data = PixmapData::Luma(atlas.as_raw());
			let pixmap = if let Some((pixmap, version)) = self.atlas_pixmap.take() {
				if atlas.version() != version {
					brush.update(pixmap, pixmap_dims, pixmap_data)
				} else {
					pixmap
				}
			} else {
				log::info!("Recreate atlas version {:?}", atlas.version(),);
				brush.create(pixmap_dims, pixmap_data)
			};
			for item in &content.items {
				match item {
					illustrator::DisplayItem {
						pos,
						content: illustrator::DisplayContent::Text(content),
						..
					} => {
						brush.draw(
							&pixmap,
							[pos.x, pos.y].into(),
							content.glyphs.iter().map(|g| PixmapInstance {
								pos: g.pos,
								dim: g.dim,
								uv_pos: g.uv_pos,
								uv_dim: g.uv_dim,
							}),
						);
					}
					illustrator::DisplayItem {
						hash,
						pos,
						size,
						content: illustrator::DisplayContent::Pixmap(content),
					} => {
						let pixmap = self.content_pixmaps.entry(*hash).or_insert_with(|| {
							let pixmap_dims = [content.pixmap_width, content.pixmap_height].into();
							let pixmap_data = PixmapData::RgbA(&content.pixmap_rgba);
							brush.create(pixmap_dims, pixmap_data)
						});
						brush.draw(
							pixmap,
							[pos.x, pos.y].into(),
							[PixmapInstance {
								pos: [0.; 2],
								dim: [size.width, size.height],
								uv_pos: [0; 2],
								uv_dim: [content.pixmap_width, content.pixmap_height],
							}],
						);
					}
				}
			}
			self.atlas_pixmap = Some((pixmap, atlas.version()));
		});

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
					action: Action::Library,
				},
				MenuItem {
					icon: Icon::LogOut,
					description: "Exit",
					active: false,
					action: Action::Exit,
				},
			];
			let tool_items = &[
				None,
				Some(ToolItem {
					icon: Icon::ArrowLeft,
					description: "Previous",
					active: false,
					action: Action::Previous,
				}),
				None,
				Some(ToolItem {
					icon: Icon::ListTree,
					description: "Chapters",
					active: matches!(self.mode, ReaderMode::Navigation),
					action: Action::Chapters,
				}),
				Some(ToolItem {
					icon: Icon::Cog,
					description: "Settings",
					active: matches!(self.mode, ReaderMode::ActionSettings),
					action: Action::Settings,
				}),
				None,
				Some(ToolItem {
					icon: Icon::ArrowRight,
					description: "Next",
					active: false,
					action: Action::Next,
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
			} else if matches!(
				self.mode,
				ReaderMode::ActionSettings | ReaderMode::ProfileSettings
			) {
				let central_panel = egui::CentralPanel::default().show_inside(ui, |ui| {
					if is_open {
						ui.disable();
					}

					let mut change = SettingsChange::default();
					ui.add(SettingsPanelUi::new(self, &mut change));

					if change.profile_changed
						&& let Err(e) = self.create_illustrator()
					{
						log::error!("Failed to create illustrator: {e}");
					}
					if change.state_changed
						&& let Err(e) = self.records.record_view_state(Self::STATE_KEY, &self.state)
					{
						log::warn!("Error saving state: {e}");
					}
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
			AppEvent::KeyUp => {
				self.on_action(self.state.action_key_up);
				EventResult::None
			}
			AppEvent::KeyDown => {
				self.on_action(self.state.action_key_down);
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
							ActiveAreaAction::Main => self.toggle_ui(),
							ActiveAreaAction::Left => self.on_action(self.state.action_area_left),
							ActiveAreaAction::Right => self.on_action(self.state.action_area_right),
						};
						GestureResult::Consumed
					} else {
						GestureResult::Unhandled
					}
				}
			}
			Gesture::Swipe(Direction::Right, _) => {
				self.on_action(self.state.swipe_right);
				GestureResult::Consumed
			}
			Gesture::Swipe(Direction::Down, _) => {
				self.on_action(self.state.swipe_down);
				GestureResult::Consumed
			}
			Gesture::Swipe(Direction::Left, _) => {
				self.on_action(self.state.swipe_left);
				GestureResult::Consumed
			}
			Gesture::Swipe(Direction::Up, _) => {
				self.on_action(self.state.swipe_up);
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

#[derive(Default)]
struct SettingsChange {
	state_changed: bool,
	profile_changed: bool,
}

pub(crate) struct SettingsPanelUi<'a> {
	config: &'a IllustratorConfig,
	mode: &'a mut ReaderMode,
	state: &'a mut ViewState,
	change: &'a mut SettingsChange,
}

impl<'a> SettingsPanelUi<'a> {
	fn new(reader: &'a mut ReaderView, change: &'a mut SettingsChange) -> Self {
		Self {
			config: &reader.config,
			mode: &mut reader.mode,
			state: &mut reader.state,
			change,
		}
	}
}

impl egui::Widget for SettingsPanelUi<'_> {
	fn ui(self, ui: &mut egui::Ui) -> egui::Response {
		ui.style_mut().visuals.widgets.open.bg_stroke.color = Color32::TRANSPARENT;
		ui.style_mut().visuals.widgets.active.bg_stroke.color = Color32::TRANSPARENT;
		ui.style_mut().visuals.widgets.inactive.bg_stroke.color = Color32::TRANSPARENT;
		ui.style_mut().visuals.widgets.hovered.bg_stroke.color = Color32::TRANSPARENT;

		ui.horizontal(|ui| {
			ui.selectable_value(
				self.mode,
				ReaderMode::ActionSettings,
				RichText::new("Actions").size(theme::L_SIZE),
			);
			ui.selectable_value(
				self.mode,
				ReaderMode::ProfileSettings,
				RichText::new("Profile").size(theme::L_SIZE),
			);
		});

		ui.spacing_mut().item_spacing.y = 8.;
		ui.spacing_mut().button_padding.y = 8.;

		if *self.mode == ReaderMode::ActionSettings {
			ui.vertical_centered_justified(|ui| {
				ui.horizontal(|ui| {
					let actions = [
						Action::None,
						Action::Next,
						Action::Previous,
						Action::Chapters,
						Action::Settings,
						Action::Library,
					];
					ui.vertical(|ui| {
						ui.label(RichText::new("").size(theme::M_SIZE));
						ui.add_space(5.);
						ui.label(RichText::new("Swipe Right").size(theme::M_SIZE));
						ui.add_space(10.5);
						ui.label(RichText::new("Swipe Left").size(theme::M_SIZE));
						ui.add_space(10.5);
						ui.label(RichText::new("Swipe Up").size(theme::M_SIZE));
						ui.add_space(10.5);
						ui.label(RichText::new("Swipe Down").size(theme::M_SIZE));
						ui.add_space(10.5);
						ui.label(RichText::new("Key Up").size(theme::M_SIZE));
						ui.add_space(10.5);
						ui.label(RichText::new("Key Down").size(theme::M_SIZE));
						ui.add_space(10.5);
						ui.label(RichText::new("Tap Right").size(theme::M_SIZE));
						ui.add_space(10.5);
						ui.label(RichText::new("Tap Left").size(theme::M_SIZE));
					});
					ui.columns(actions.len(), |columns| {
						for (ui, action) in columns.iter_mut().zip(actions) {
							ui.vertical_centered_justified(|ui| {
								ui.label(RichText::new(action.to_string()).size(theme::M_SIZE));
								if ui
									.add(ActionSelectableUi::new(
										&mut self.state.swipe_right,
										action,
									))
									.changed()
								{
									self.change.state_changed = true;
								}
								if ui
									.add(ActionSelectableUi::new(
										&mut self.state.swipe_left,
										action,
									))
									.changed()
								{
									self.change.state_changed = true;
								}
								if ui
									.add(ActionSelectableUi::new(&mut self.state.swipe_up, action))
									.changed()
								{
									self.change.state_changed = true;
								}
								if ui
									.add(ActionSelectableUi::new(
										&mut self.state.swipe_down,
										action,
									))
									.changed()
								{
									self.change.state_changed = true;
								}
								if ui
									.add(ActionSelectableUi::new(
										&mut self.state.action_key_up,
										action,
									))
									.changed()
								{
									self.change.state_changed = true;
								}
								if ui
									.add(ActionSelectableUi::new(
										&mut self.state.action_key_down,
										action,
									))
									.changed()
								{
									self.change.state_changed = true;
								}
								if ui
									.add(ActionSelectableUi::new(
										&mut self.state.action_area_right,
										action,
									))
									.changed()
								{
									self.change.state_changed = true;
								}
								if ui
									.add(ActionSelectableUi::new(
										&mut self.state.action_area_left,
										action,
									))
									.changed()
								{
									self.change.state_changed = true;
								}
							});
						}
					})
				});
			})
			.response
		} else {
			ui.vertical_centered_justified(|ui| {
				for (profile, _) in self.config.as_ref().iter() {
					let is_active = self.state.profile.as_str() == profile;
					let response = ui.add(
						Button::new(RichText::new(profile).size(theme::M_SIZE)).selected(is_active),
					);
					if response.clicked() {
						self.state.profile = profile.clone();
						self.change.state_changed = true;
						self.change.profile_changed = true;
					}
				}
			})
			.response
		}
	}
}

struct ActionSelectableUi<'a> {
	current_value: &'a mut Action,
	alternative: Action,
}

impl<'a> ActionSelectableUi<'a> {
	fn new(current_value: &'a mut Action, alternative: Action) -> Self {
		Self {
			current_value,
			alternative,
		}
	}
}

impl egui::Widget for ActionSelectableUi<'_> {
	fn ui(self, ui: &mut egui::Ui) -> egui::Response {
		let icon = if *self.current_value == self.alternative {
			UiIcon::new(Icon::CheckSquare).build()
		} else {
			UiIcon::new(Icon::Square).build()
		};
		let response = ui.add(Button::new(icon).selected(*self.current_value == self.alternative));
		if response.clicked() {
			*self.current_value = self.alternative;
		}
		response
	}
}

pub(crate) struct ChapterCard {
	location: Location,
	title: Arc<String>,
	active: bool,
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

		let group = egui::Frame::group(ui.style());
		if card.active {
			group.fill(ui.style().visuals.selection.bg_fill)
		} else {
			group
		}
		.show(ui, |ui| {
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
