use std::marker::PhantomData;

use crate::renderer::gui_renderer;
use crate::ui::UiInput;

pub(crate) struct PainterUIReady;
pub(crate) struct PainterUIPainted;
pub(crate) struct PainterPixmapReady;
pub(crate) struct PainterPixmapPainted;

pub(crate) struct Painter<'a, PainterUI = PainterUIReady, PainterPixmap = PainterPixmapReady> {
	ui_input: &'a mut UiInput,
	gui_renderer: &'a mut gui_renderer::Renderer,
	pixmap_renderer: &'a mut pixelator::Renderer,
	phantom: PhantomData<(PainterUI, PainterPixmap)>,
}

impl<'a> Painter<'a> {
	pub(crate) fn new(
		ui_input: &'a mut UiInput,
		gui_renderer: &'a mut gui_renderer::Renderer,
		pixmap_renderer: &'a mut pixelator::Renderer,
	) -> Self {
		Self {
			ui_input,
			gui_renderer,
			pixmap_renderer,
			phantom: PhantomData,
		}
	}
}

impl<'a, APainterUI, APainterPixmap> Painter<'a, APainterUI, APainterPixmap> {
	fn into<BPainterUI, BPainterPixmap>(self) -> Painter<'a, BPainterUI, BPainterPixmap> {
		Painter {
			pixmap_renderer: self.pixmap_renderer,
			ui_input: self.ui_input,
			gui_renderer: self.gui_renderer,
			phantom: PhantomData,
		}
	}
}

impl<'a, PainterPixmap> Painter<'a, PainterUIReady, PainterPixmap> {
	pub(crate) fn draw_ui(
		self,
		run_ui: impl FnMut(&mut egui::Ui),
	) -> Painter<'a, PainterUIPainted, PainterPixmap> {
		self.gui_renderer.prepare(self.ui_input.run(run_ui));
		self.into()
	}
}

impl<'a, PainterUI> Painter<'a, PainterUI, PainterPixmapReady> {
	pub(crate) fn draw_pixmap(
		self,
		run_brush: impl FnMut(&mut pixelator::PixmapBrush<'_>),
	) -> Painter<'a, PainterUI, PainterPixmapPainted> {
		self.pixmap_renderer.prepare(run_brush);
		self.into()
	}
}
