#![allow(dead_code)]
use std::marker::PhantomData;

use crate::renderer::glyphon_renderer;
use crate::renderer::gui_renderer;
use crate::renderer::pixmap_renderer;
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
	glyphon_renderer: &'a mut glyphon_renderer::Renderer,
	phantom: PhantomData<(PainterUI, PainterPixmap)>,
}

impl<'a> Painter<'a> {
	pub(crate) fn new(
		device: &'a wgpu::Device,
		queue: &'a wgpu::Queue,
		ui_input: &'a mut UiInput,
		gui_renderer: &'a mut gui_renderer::Renderer,
		pixmap_renderer: &'a mut pixmap_renderer::Renderer,
		glyphon_renderer: &'a mut glyphon_renderer::Renderer,
	) -> Self {
		Self {
			device,
			queue,
			ui_input,
			gui_renderer,
			pixmap_renderer,
			glyphon_renderer,
			phantom: PhantomData,
		}
	}

	// Temporary, while we get rid of it
	pub(crate) fn draw_glyphon<'b>(
		self,
		font_system: &mut cosmic_text::FontSystem,
		items: impl Iterator<Item = glyphon::TextArea<'b>> + Clone,
	) -> Self {
		let _ = self
			.glyphon_renderer
			.prepare(self.device, self.queue, font_system, items)
			.inspect_err(|err| log::error!("Glyphon prepare error: {err}"));
		self
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
			glyphon_renderer: self.glyphon_renderer,
			phantom: PhantomData,
		}
	}
}

impl<'a, PainterPixmap> Painter<'a, PainterUIReady, PainterPixmap> {
	pub(crate) fn draw_ui(
		self,
		run_ui: impl FnMut(&egui::Context),
	) -> Painter<'a, PainterUIPainted, PainterPixmap> {
		self.gui_renderer
			.prepare(self.device, self.queue, self.ui_input.run(run_ui));
		self.into()
	}
}

impl<'a, PainterUI> Painter<'a, PainterUI, PainterPixmapReady> {
	pub(crate) fn draw_pixmap<'i>(
		self,
		inputs: impl Iterator<Item = pixmap_renderer::PixmapInput<'i>>,
	) -> Painter<'a, PainterUI, PainterPixmapPainted> {
		self.pixmap_renderer
			.prepare(self.device, self.queue, inputs);
		self.into()
	}
}
