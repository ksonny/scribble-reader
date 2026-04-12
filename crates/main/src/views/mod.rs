mod experiments;
mod library;
mod reader;

use scribe::BookId;
use scribe::LibraryScribeAssistant;
use scribe::RecordKeeper;
use scribe::config::IllustratorConfig;
use sculpter::SculpterFonts;
use wrangler::content::ContentWranglerAssistant;

use crate::AppBell;
use crate::AppEvent;
use crate::gestures::GestureEvent;
use crate::renderer::Painter;
use crate::ui::UiIcon;

pub(crate) enum EventResult {
	None,
	RequestRedraw,
}

pub(crate) enum GestureResult {
	Unhandled,
	Consumed,
}

#[derive(Clone)]
struct Viewport {
	screen_width: u32,
	screen_height: u32,
	scale_factor: f32,
}

pub(crate) trait ViewHandle {
	fn draw(&mut self, painter: Painter<'_>);

	fn event(&mut self, event: &AppEvent) -> EventResult;

	fn gesture(&mut self, event: &GestureEvent) -> GestureResult;

	fn resize(&mut self, width: u32, height: u32) {
		let _ = (width, height);
	}

	fn rescale(&mut self, scale_factor: f32) {
		let _ = scale_factor;
	}

	/// About to be closed.
	///
	/// Free any resources and get ready to be dropped.
	fn close(&mut self) {}
}

#[allow(clippy::large_enum_variant)]
enum Views {
	Loading,
	Library(library::LibraryView),
	Reader(reader::ReaderView),
	Experiments(experiments::ExperimentsView),
	Error(String),
}

pub(crate) struct AppView {
	bell: AppBell,
	viewport: Viewport,
	view: Views,
}

impl AppView {
	pub(crate) fn new(bell: AppBell) -> Self {
		let viewport = Viewport {
			screen_width: 800,
			screen_height: 600,
			scale_factor: 1.0,
		};
		Self {
			bell,
			viewport,
			view: Views::Loading,
		}
	}

	pub(crate) fn library(&mut self, records: RecordKeeper, scribe: LibraryScribeAssistant) {
		self.close();
		match library::LibraryView::create(self.bell.clone(), records, scribe) {
			Ok(view) => self.view = Views::Library(view),
			Err(e) => {
				log::error!("Library view error: {}", e);
				self.view = Views::Error(format!("Library view error: {}", e));
			}
		};
	}

	pub(crate) fn reader(
		&mut self,
		config: IllustratorConfig,
		keeper: RecordKeeper,
		fonts: SculpterFonts,
		content: ContentWranglerAssistant,
		bell: AppBell,
		book_id: BookId,
	) {
		self.close();
		match reader::ReaderView::create(
			config,
			keeper,
			fonts,
			content,
			bell,
			book_id,
			self.viewport.clone(),
		) {
			Ok(view) => self.view = Views::Reader(view),
			Err(e) => {
				log::error!("Failed to create reader: {e}");
				self.view = Views::Error(format!("Reader view error: {}", e));
			}
		};
	}

	pub(crate) fn experiments(&mut self, fonts: sculpter::SculpterFonts) {
		self.close();
		self.view = Views::Experiments(experiments::ExperimentsView::create(
			self.bell.clone(),
			fonts,
			self.viewport.clone(),
		))
	}
}

impl ViewHandle for AppView {
	fn draw(&mut self, painter: Painter) {
		match &mut self.view {
			Views::Loading => {
				painter.draw_ui(|ui| {
					ui.centered_and_justified(|ui| {
						ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
						ui.label(
							UiIcon::new(lucide_icons::Icon::RefreshCw)
								.large()
								.text("Loading")
								.build(),
						);
					});
				});
			}
			Views::Error(error) => {
				painter.draw_ui(|ui| {
					ui.centered_and_justified(|ui| {
						ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
						ui.label(
							UiIcon::new(lucide_icons::Icon::AlertTriangle)
								.large()
								.text(error)
								.build(),
						);
					});
				});
			}
			Views::Library(view) => view.draw(painter),
			Views::Reader(view) => view.draw(painter),
			Views::Experiments(view) => view.draw(painter),
		}
	}

	fn event(&mut self, event: &AppEvent) -> EventResult {
		match &mut self.view {
			Views::Loading => EventResult::None,
			Views::Error(_) => EventResult::None,
			Views::Library(view) => view.event(event),
			Views::Reader(view) => view.event(event),
			Views::Experiments(view) => view.event(event),
		}
	}

	fn gesture(&mut self, event: &GestureEvent) -> GestureResult {
		match &mut self.view {
			Views::Loading => GestureResult::Unhandled,
			Views::Error(_) => GestureResult::Unhandled,
			Views::Library(view) => view.gesture(event),
			Views::Reader(view) => view.gesture(event),
			Views::Experiments(view) => view.gesture(event),
		}
	}

	fn resize(&mut self, width: u32, height: u32) {
		self.viewport.screen_width = width;
		self.viewport.screen_height = height;

		match &mut self.view {
			Views::Loading => {}
			Views::Error(_) => {}
			Views::Library(view) => view.resize(width, height),
			Views::Reader(view) => view.resize(width, height),
			Views::Experiments(view) => view.resize(width, height),
		}
	}

	fn rescale(&mut self, scale_factor: f32) {
		self.viewport.scale_factor = scale_factor;

		match &mut self.view {
			Views::Loading => {}
			Views::Error(_) => {}
			Views::Library(view) => view.rescale(scale_factor),
			Views::Reader(view) => view.rescale(scale_factor),
			Views::Experiments(view) => view.rescale(scale_factor),
		}
	}

	fn close(&mut self) {
		match &mut self.view {
			Views::Loading => {}
			Views::Error(_) => {}
			Views::Library(view) => view.close(),
			Views::Reader(view) => view.close(),
			Views::Experiments(view) => view.close(),
		}
	}
}
