use std::collections::BTreeMap;
use std::marker::PhantomData;

use crate::renderer::EguiMapped;
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
	gui_mapped_pixmaps: &'a mut BTreeMap<pixelator::PixmapId, EguiMapped>,
	phantom: PhantomData<(PainterUI, PainterPixmap)>,
}

impl<'a> Painter<'a> {
	pub(crate) fn new(
		ui_input: &'a mut UiInput,
		gui_renderer: &'a mut gui_renderer::Renderer,
		pixmap_renderer: &'a mut pixelator::Renderer,
		gui_mapped_pixmaps: &'a mut BTreeMap<pixelator::PixmapId, EguiMapped>,
	) -> Self {
		Self {
			ui_input,
			gui_renderer,
			pixmap_renderer,
			gui_mapped_pixmaps,
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
			gui_mapped_pixmaps: self.gui_mapped_pixmaps,
			phantom: PhantomData,
		}
	}
}

#[derive(Debug, thiserror::Error)]
pub enum PainterMappingError {
	#[error("Pixmap target is deallocated")]
	TextureDeallocated,
}

impl<'a, PainterPixmap> Painter<'a, PainterUIReady, PainterPixmap> {
	pub(crate) fn with_egui_texture(
		&mut self,
		pixmap: &pixelator::PixmapRef,
	) -> Result<egui::load::SizedTexture, PainterMappingError> {
		if let Some(entry) = self.gui_mapped_pixmaps.get(pixmap.as_ref()) {
			Ok(entry.texture)
		} else {
			let (texture_view, dims) = {
				let textures = self.pixmap_renderer.lock_textures();
				let entry = textures
					.get(pixmap.as_ref())
					.ok_or(PainterMappingError::TextureDeallocated)?;
				let texture_view = entry
					.texture()
					.create_view(&wgpu::wgt::TextureViewDescriptor::default());
				(texture_view, entry.pixmap_dims().clone())
			};
			let id = self.gui_renderer.register_native_texture(&texture_view);
			let texture = egui::load::SizedTexture {
				id,
				size: [dims.width() as f32, dims.height() as f32].into(),
			};

			self.gui_mapped_pixmaps.insert(
				*pixmap.as_ref(),
				EguiMapped {
					weak_ref: pixmap.downgrade(),
					texture,
				},
			);

			Ok(texture)
		}
	}

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
