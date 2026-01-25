use crate::AppEvent;
use crate::gestures::GestureEvent;
use crate::renderer::Painter;
use crate::views::EventResult;
use crate::views::GestureResult;
use crate::views::ViewHandle;

pub(crate) struct ExperimentsView {}

impl ExperimentsView {
	pub(crate) fn create() -> Self {
		Self {}
	}
}

impl ViewHandle for ExperimentsView {
	fn draw(&mut self, _painter: Painter<'_>) {
		todo!()
	}

	fn event(&mut self, _event: &AppEvent) -> EventResult {
		EventResult::None
	}

	fn gesture(&mut self, _event: &GestureEvent) -> GestureResult {
		GestureResult::Unhandled
	}
}
