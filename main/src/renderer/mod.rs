mod glyphon_renderer;
mod gui_renderer;
mod pixmap_renderer;

use illustrator::DisplayItem;
use winit::dpi::PhysicalSize;

use egui_wgpu::wgpu::{
	self,
};
use std::sync::Arc;
use winit::window::Window;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RendererError {
	#[error(transparent)]
	CreateSurface(#[from] wgpu::CreateSurfaceError),
	#[error(transparent)]
	RequestDevice(#[from] wgpu::RequestDeviceError),
	#[error(transparent)]
	RequestAdapter(#[from] wgpu::wgt::RequestAdapterError),
	#[error(transparent)]
	Surface(#[from] wgpu::SurfaceError),
	#[error(transparent)]
	PixmapPrepare(#[from] pixmap_renderer::PrepareError),
	#[error(transparent)]
	PixmapRender(#[from] pixmap_renderer::RenderError),
	#[error(transparent)]
	GlyphonPrepare(#[from] glyphon::PrepareError),
	#[error(transparent)]
	GlyphonRender(#[from] glyphon::RenderError),
	#[error("Failed to get surface format")]
	NoTextureFormat,
	#[error("Failed to get surface alpha mode")]
	NoAlphaMode,
	#[error("Surface not available, probably suspended")]
	SurfaceNotAvailable,
}

struct SurfaceState<'window> {
	window: Arc<Window>,
	format: wgpu::TextureFormat,
	alpha_mode: wgpu::CompositeAlphaMode,
	surface: wgpu::Surface<'window>,
}

impl<'window> SurfaceState<'window> {
	fn setup_swapchain(&self, device: &wgpu::Device, width: u32, height: u32) {
		let surface_configuration = wgpu::SurfaceConfiguration {
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
			format: self.format,
			width,
			height,
			present_mode: wgpu::PresentMode::AutoVsync,
			alpha_mode: self.alpha_mode,
			view_formats: vec![self.format],
			desired_maximum_frame_latency: 2,
		};
		self.surface.configure(device, &surface_configuration);
	}
}

#[allow(unused)]
pub(crate) struct Renderer<'window> {
	instance: wgpu::Instance,
	adapter: wgpu::Adapter,
	device: wgpu::Device,
	queue: wgpu::Queue,
	pixmap_renderer: pixmap_renderer::Renderer,
	glyphon_renderer: glyphon_renderer::Renderer,
	gui_renderer: gui_renderer::Renderer,
	surface_state: Option<SurfaceState<'window>>,
	resized: Option<PhysicalSize<u32>>,
	rescale: Option<f64>,
}

impl Renderer<'_> {
	pub(crate) async fn create(
		window: Window,
		egui_ctx: &egui::Context,
	) -> Result<Self, RendererError> {
		let window = Arc::new(window);
		let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
			backends: wgpu::Backends::all(),
			..Default::default()
		});
		let surface = instance.create_surface(window.clone())?;

		let adapter =
			wgpu::util::initialize_adapter_from_env_or_default(&instance, Some(&surface)).await?;

		let (device, queue) = adapter
			.request_device(&wgpu::DeviceDescriptor {
				label: None,
				required_features: wgpu::Features::empty(),
				#[cfg(target_os = "android")]
				required_limits: wgpu::Limits::downlevel_webgl2_defaults()
					.using_resolution(adapter.limits()),
				#[cfg(not(target_os = "android"))]
				required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
				memory_hints: wgpu::MemoryHints::MemoryUsage,
				trace: wgpu::Trace::Off,
			})
			.await?;

		let size = window.inner_size();
		let (format, alpha_mode) = surface_format(&surface, &adapter)?;

		let mut pixmap_renderer = pixmap_renderer::Renderer::new(&device, format);
		pixmap_renderer.resize(size.width, size.height);

		let mut glyphon_renderer = glyphon_renderer::Renderer::new(&device, &queue, format);
		glyphon_renderer.resize(&queue, size.width, size.height);

		let mut gui_renderer = gui_renderer::Renderer::new(&device, format, egui_ctx.clone());
		gui_renderer.resume(&device, window.clone());

		let surface_state = SurfaceState {
			window,
			surface,
			format,
			alpha_mode,
		};
		surface_state.setup_swapchain(&device, size.width, size.height);

		Ok(Renderer {
			instance,
			adapter,
			device,
			queue,
			pixmap_renderer,
			glyphon_renderer,
			gui_renderer,
			resized: None,
			rescale: None,
			surface_state: Some(surface_state),
		})
	}

	pub(crate) fn resume(&mut self, window: Window) -> Result<(), RendererError> {
		if self.surface_state.is_some() {
			// Already initailized
			return Ok(());
		}

		let window = Arc::new(window);

		let surface = self.instance.create_surface(window.clone())?;

		let size = window.inner_size();
		let (format, alpha_mode) = surface_format(&surface, &self.adapter)?;

		self.pixmap_renderer.resize(size.width, size.height);

		self.glyphon_renderer
			.resize(&self.queue, size.width, size.height);
		self.gui_renderer.resume(&self.device, window.clone());

		let surface_state = SurfaceState {
			window,
			surface,
			format,
			alpha_mode,
		};
		surface_state.setup_swapchain(&self.device, size.width, size.height);
		self.surface_state = Some(surface_state);

		Ok(())
	}

	pub(crate) fn suspend(&mut self) {
		self.gui_renderer.suspend();
		self.surface_state.take();
	}

	pub(crate) fn resize(&mut self, size: PhysicalSize<u32>) {
		self.resized = Some(size);
	}

	pub(crate) fn rescale(&mut self, scale_factor: f64) {
		self.rescale = Some(scale_factor);
	}

	pub(crate) fn request_redraw(&self) {
		if let Some(surface_state) = self.surface_state.as_ref() {
			surface_state.window.request_redraw();
		}
	}

	pub(crate) fn prepare_ui(&mut self, output: egui::output::FullOutput) {
		self.gui_renderer.prepare(&self.device, &self.queue, output);
	}

	pub(crate) fn prepare_page<'a>(
		&mut self,
		font_system: &mut cosmic_text::FontSystem,
		items: impl Iterator<Item = &'a DisplayItem> + Clone,
	) -> Result<(), RendererError> {
		let pixmap_input = items.clone().filter_map(|d| match d {
			illustrator::DisplayItem {
				pos,
				size,
				content: illustrator::DisplayContent::Pixmap(item),
			} => Some(pixmap_renderer::PixmapInput {
				pixmap_rgba: &item.pixmap_rgba,
				pixmap_width: item.pixmap_width,
				pixmap_height: item.pixmap_height,
				targets: vec![pixmap_renderer::PixmapTargetInput {
					pos: [pos.x, pos.y],
					dim: [size.width, size.height],
					tex_pos: [0; 2],
					tex_dim: [item.pixmap_width, item.pixmap_height],
				}],
			}),
			_ => None,
		});
		self.pixmap_renderer
			.prepare(&self.device, &self.queue, pixmap_input)?;

		let text_areas = items.filter_map(|d| match d {
			illustrator::DisplayItem {
				pos,
				content: illustrator::DisplayContent::Text(item),
				..
			} => Some(glyphon::TextArea {
				buffer: &item.buffer,
				left: pos.x as f32,
				top: pos.y as f32,
				scale: 1.0,
				bounds: glyphon::TextBounds::default(),
				default_color: glyphon::Color::rgb(0, 0, 0),
				custom_glyphs: &[],
			}),
			_ => None,
		});
		self.glyphon_renderer
			.prepare(&self.device, &self.queue, font_system, text_areas)?;
		Ok(())
	}

	pub(crate) fn render(&mut self) -> Result<(), RendererError> {
		let surface_state = self
			.surface_state
			.as_mut()
			.ok_or(RendererError::SurfaceNotAvailable)?;
		if let Some(size) = self.resized.take() {
			self.gui_renderer.resize(size.width, size.height);
			self.pixmap_renderer.resize(size.width, size.height);
			self.glyphon_renderer
				.resize(&self.queue, size.width, size.height);
			surface_state.setup_swapchain(&self.device, size.width, size.height);
		}
		if let Some(scale_factor) = self.rescale.take() {
			self.gui_renderer.rescale(scale_factor);
		}

		match surface_state.surface.get_current_texture() {
			Ok(frame) => {
				let view = frame
					.texture
					.create_view(&wgpu::TextureViewDescriptor::default());

				let mut encoder =
					self.device
						.create_command_encoder(&wgpu::wgt::CommandEncoderDescriptor {
							label: Some("Renderer encoder"),
						});

				self.gui_renderer
					.update_buffers(&self.device, &self.queue, &mut encoder);

				let mut rpass = encoder
					.begin_render_pass(&wgpu::RenderPassDescriptor {
						label: Some("Main pass"),
						color_attachments: &[Some(wgpu::RenderPassColorAttachment {
							view: &view,
							resolve_target: None,
							ops: wgpu::Operations {
								load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
								store: wgpu::StoreOp::Store,
							},
						})],
						depth_stencil_attachment: None,
						timestamp_writes: None,
						occlusion_query_set: None,
					})
					.forget_lifetime();

				self.pixmap_renderer.render(&mut rpass)?;
				self.glyphon_renderer.render(&mut rpass)?;
				self.gui_renderer.render(&mut rpass);

				drop(rpass);
				self.queue.submit(Some(encoder.finish()));
				frame.present();

				self.glyphon_renderer.cleanup();
				self.gui_renderer.cleanup();

				Ok(())
			}
			Err(e @ wgpu::SurfaceError::OutOfMemory) => {
				log::error!("Swapchain error: {e}");
				Err(e.into())
			}
			Err(e) => {
				log::warn!("Hopefully recoverable error in render: {e}");
				Ok(())
			}
		}
	}
}

fn surface_format(
	surface: &wgpu::Surface<'_>,
	adapter: &wgpu::Adapter,
) -> Result<(wgpu::TextureFormat, wgpu::CompositeAlphaMode), RendererError> {
	let cap = surface.get_capabilities(adapter);
	let format = cap
		.formats
		.iter()
		.find(|f| matches!(*f, wgpu::TextureFormat::Rgba8Unorm))
		.or_else(|| cap.formats.first())
		.cloned()
		.ok_or(RendererError::NoTextureFormat)?;
	let alpha_mode = cap
		.alpha_modes
		.first()
		.cloned()
		.ok_or(RendererError::NoAlphaMode)?;
	Ok((format, alpha_mode))
}
