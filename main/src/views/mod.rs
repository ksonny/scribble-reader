mod experiments;
mod library;
mod reader;

use illustrator::IllustratorHandle;
use scribe::ScribeAssistant;

use crate::AppBell;
use crate::AppEvent;
use crate::gestures::GestureEvent;
use crate::renderer::Painter;

pub(crate) enum EventResult {
	None,
	RequestRedraw,
}

pub(crate) enum GestureResult {
	Unhandled,
	Consumed,
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
}

#[allow(clippy::large_enum_variant)]
enum Views {
	Loading,
	Library(library::LibraryView),
	Reader(reader::ReaderView),
	Experiments(experiments::ExperimentsView),
}

pub(crate) struct AppView {
	bell: AppBell,
	scale_factor: f32,
	screen_width: u32,
	screen_height: u32,
	view: Views,
}

impl AppView {
	pub(crate) fn new(bell: AppBell) -> Self {
		bell.send_event(AppEvent::OpenLibrary);
		Self {
			bell,
			scale_factor: 1.0,
			screen_width: 800,
			screen_height: 600,
			view: Views::Loading,
		}
	}

	pub(crate) fn library(&mut self, scribe: ScribeAssistant) {
		self.view = Views::Library(library::LibraryView::create(self.bell.clone(), scribe))
	}

	pub(crate) fn reader(&mut self, illustrator: IllustratorHandle) {
		self.view = Views::Reader(reader::ReaderView::create(
			self.bell.clone(),
			illustrator,
			self.screen_width,
			self.screen_height,
			self.scale_factor,
		))
	}

	pub(crate) fn experiments(&mut self) {
		self.view = Views::Experiments(experiments::ExperimentsView::create())
	}
}

impl ViewHandle for AppView {
	fn draw(&mut self, painter: Painter) {
		match &mut self.view {
			Views::Loading => {}
			Views::Library(view) => view.draw(painter),
			Views::Reader(view) => view.draw(painter),
			Views::Experiments(view) => view.draw(painter),
		}
	}

	fn event(&mut self, event: &AppEvent) -> EventResult {
		match &mut self.view {
			Views::Loading => EventResult::None,
			Views::Library(view) => view.event(event),
			Views::Reader(view) => view.event(event),
			Views::Experiments(view) => view.event(event),
		}
	}

	fn gesture(&mut self, event: &GestureEvent) -> GestureResult {
		match &mut self.view {
			Views::Loading => GestureResult::Unhandled,
			Views::Library(view) => view.gesture(event),
			Views::Reader(view) => view.gesture(event),
			Views::Experiments(view) => view.gesture(event),
		}
	}

	fn resize(&mut self, width: u32, height: u32) {
		self.screen_width = width;
		self.screen_height = height;

		match &mut self.view {
			Views::Loading => {}
			Views::Library(view) => view.resize(width, height),
			Views::Reader(view) => view.resize(width, height),
			Views::Experiments(view) => view.resize(width, height),
		}
	}

	fn rescale(&mut self, scale_factor: f32) {
		self.scale_factor = scale_factor;

		match &mut self.view {
			Views::Loading => {}
			Views::Library(view) => view.rescale(scale_factor),
			Views::Reader(view) => view.rescale(scale_factor),
			Views::Experiments(view) => view.rescale(scale_factor),
		}
	}
}
