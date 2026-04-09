use std::marker::PhantomData;

use crate::renderer::gui_renderer;
use crate::renderer::pixmap_renderer;
use crate::renderer::pixmap_renderer::PixmapBrush;
use crate::ui::UiInput;

pub(crate) struct PainterUIReady;
pub(crate) struct PainterUIPainted;
pub(crate) struct PainterPixmapReady;
pub(crate) struct PainterPixmapPainted;

pub(crate) struct Painter<'a, PainterUI = PainterUIReady, PainterPixmap = PainterPixmapReady> {
	device: &'a wgpu::Device,
	queue: &'a wgpu::Queue,
	ui_input: &'a mut UiInput,
	gui_renderer: &'a mut gui_renderer::Renderer,
	pixmap_renderer: &'a mut pixmap_renderer::Renderer,
	phantom: PhantomData<(PainterUI, PainterPixmap)>,
}

impl<'a> Painter<'a> {
	pub(crate) fn new(
		device: &'a wgpu::Device,
		queue: &'a wgpu::Queue,
		ui_input: &'a mut UiInput,
		gui_renderer: &'a mut gui_renderer::Renderer,
		pixmap_renderer: &'a mut pixmap_renderer::Renderer,
	) -> Self {
		Self {
			device,
			queue,
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
			device: self.device,
			queue: self.queue,
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
		self.gui_renderer
			.prepare(self.device, self.queue, self.ui_input.run(run_ui));
		self.into()
	}
}

impl<'a, PainterUI> Painter<'a, PainterUI, PainterPixmapReady> {
	pub(crate) fn draw_pixmap(
		self,
		run_brush: impl FnMut(&mut PixmapBrush<'_>),
	) -> Painter<'a, PainterUI, PainterPixmapPainted> {
		self.pixmap_renderer
			.prepare(self.device, self.queue, run_brush);
		self.into()
	}
}
