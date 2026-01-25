use glyphon::Cache;
use glyphon::FontSystem;
use glyphon::PrepareError;
use glyphon::Resolution;
use glyphon::SwashCache;
use glyphon::TextArea;
use glyphon::TextAtlas;
use glyphon::TextRenderer;
use glyphon::Viewport;
use wgpu::Device;
use wgpu::MultisampleState;
use wgpu::Queue;
use wgpu::RenderPass;
use wgpu::TextureFormat;

pub(crate) struct Renderer {
	swash_cache: SwashCache,
	viewport: Viewport,
	atlas: TextAtlas,
	text_renderer: TextRenderer,
}

impl Renderer {
	pub(crate) fn new(device: &Device, queue: &Queue, format: TextureFormat) -> Self {
		let swash_cache = SwashCache::new();
		let cache = Cache::new(device);
		let viewport = Viewport::new(device, &cache);
		let mut atlas = TextAtlas::new(device, queue, &cache, format);
		let text_renderer =
			TextRenderer::new(&mut atlas, device, MultisampleState::default(), None);
		Self {
			swash_cache,
			viewport,
			atlas,
			text_renderer,
		}
	}

	pub(crate) fn resize(&mut self, queue: &Queue, width: u32, height: u32) {
		self.viewport.update(queue, Resolution { width, height });
	}

	pub(crate) fn prepare<'a>(
		&mut self,
		device: &Device,
		queue: &Queue,
		font_system: &mut FontSystem,
		items: impl Iterator<Item = TextArea<'a>> + Clone,
	) -> Result<(), PrepareError> {
		self.text_renderer.prepare(
			device,
			queue,
			font_system,
			&mut self.atlas,
			&self.viewport,
			items,
			&mut self.swash_cache,
		)?;
		Ok(())
	}

	pub(crate) fn render(
		&self,
		rpass: &mut RenderPass<'_>,
	) -> std::result::Result<(), glyphon::RenderError> {
		self.text_renderer
			.render(&self.atlas, &self.viewport, rpass)
	}

	pub(crate) fn cleanup(&mut self) {
		self.atlas.trim();
	}
}
