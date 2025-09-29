use egui::TexturesDelta;
use winit::dpi::PhysicalSize;

use egui_wgpu::wgpu::{
	self,
};
use std::sync::Arc;
use winit::event::WindowEvent;
use winit::window::Window;

use crate::ui::{GuiView, MainPokeStick};

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
	#[error("Failed to get renderer format")]
	NoTextureFormat,
}


pub struct GuiRenderer {
	ctx: egui::Context,
	state: egui_winit::State,
	renderer: egui_wgpu::Renderer,
	paint_jobs: Vec<egui::ClippedPrimitive>,
	textures: egui::TexturesDelta,
}

impl GuiRenderer {
	pub fn new(
		window: &Window,
		device: &wgpu::Device,
		texture_format: wgpu::TextureFormat,
		egui_ctx: &egui::Context,
	) -> Self {
		let scale_factor = window.scale_factor();
		let max_texture_size = device.limits().max_texture_dimension_2d as usize;

		let egui_state = egui_winit::State::new(
			egui_ctx.clone(),
			egui::ViewportId::ROOT,
			window,
			Some(scale_factor as f32),
			None,
			Some(max_texture_size),
		);

		let renderer = egui_wgpu::Renderer::new(device, texture_format, None, 1, false);
		let textures = TexturesDelta::default();

		Self {
			ctx: egui_ctx.clone(),
			state: egui_state,
			renderer,
			paint_jobs: vec![],
			textures,
		}
	}

	pub(crate) fn handle_event(
		&mut self,
		window: &Window,
		event: &WindowEvent,
	) -> egui_winit::EventResponse {
		self.state.on_window_event(window, event)
	}

	pub(crate) fn prepare(&mut self, window: &Window, view: &mut impl GuiView, poke_stick: &impl MainPokeStick) {
		let raw_input = self.state.take_egui_input(window);
		let output = self.ctx.run(raw_input, |egui_ctx| view.draw(egui_ctx, poke_stick));

		self.state
			.handle_platform_output(window, output.platform_output);
		self.textures.append(output.textures_delta);
		self.paint_jobs = self
			.ctx
			.tessellate(output.shapes, window.scale_factor() as f32);
	}

	pub(crate) fn render(
		&mut self,
		encoder: &mut wgpu::CommandEncoder,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		screen: &egui_wgpu::ScreenDescriptor,
		view: &wgpu::TextureView,
	) {
		for (id, image_delta) in &self.textures.set {
			self.renderer
				.update_texture(device, queue, *id, image_delta);
		}

		self.renderer
			.update_buffers(device, queue, encoder, &self.paint_jobs, screen);

		{
			let mut rpass = encoder
				.begin_render_pass(&wgpu::RenderPassDescriptor {
					label: Some("egui"),
					color_attachments: &[Some(wgpu::RenderPassColorAttachment {
						view,
						resolve_target: None,
						ops: wgpu::Operations {
							load: wgpu::LoadOp::Load,
							store: wgpu::StoreOp::Store,
						},
					})],
					depth_stencil_attachment: None,
					..Default::default()
				})
				.forget_lifetime();

			self.renderer.render(&mut rpass, &self.paint_jobs, screen);
		}

		let textures = std::mem::take(&mut self.textures);
		for id in &textures.free {
			self.renderer.free_texture(id);
		}
	}
}

#[allow(unused)]
pub(crate) struct Renderer<'window> {
	pub(crate) window: Arc<Window>,
	surface_configured: bool,
	surface: wgpu::Surface<'window>,
	adapter: wgpu::Adapter,
	device: wgpu::Device,
	queue: wgpu::Queue,
	format: wgpu::TextureFormat,
	did_resize: bool,
	size: PhysicalSize<u32>,
	scale_factor: f64,
	pub(crate) gui_renderer: GuiRenderer,
}

impl<'window> Renderer<'window> {
	async fn init_wgpu(
		instance: &wgpu::Instance,
		surface: &wgpu::Surface<'_>,
	) -> Result<
		(
			wgpu::TextureFormat,
			wgpu::Adapter,
			wgpu::Device,
			wgpu::Queue,
		),
		RendererError,
	> {
		let adapter =
			wgpu::util::initialize_adapter_from_env_or_default(instance, Some(surface)).await?;
		let capabilities = surface.get_capabilities(&adapter);
		let format = capabilities
			.formats
			.iter()
			.copied()
			.find(wgpu::TextureFormat::is_srgb)
			.or_else(|| capabilities.formats.first().copied())
			.ok_or(RendererError::NoTextureFormat)?;
		let (device, queue) = adapter
			.request_device(&wgpu::DeviceDescriptor {
				label: None,
				// required_features: adapter.features(),
				required_features: wgpu::Features::empty(),
				required_limits: wgpu::Limits::downlevel_webgl2_defaults()
					.using_resolution(adapter.limits()),
				memory_hints: wgpu::MemoryHints::MemoryUsage,
				trace: wgpu::Trace::Off,
			})
			.await?;

		Ok((format, adapter, device, queue))
	}

	fn surface_config(
		size: PhysicalSize<u32>,
		format: wgpu::TextureFormat,
	) -> wgpu::wgt::SurfaceConfiguration<Vec<wgpu::TextureFormat>> {
		wgpu::SurfaceConfiguration {
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
			format,
			width: size.width,
			height: size.height,
			present_mode: wgpu::PresentMode::AutoVsync,
			alpha_mode: wgpu::CompositeAlphaMode::Auto,
			view_formats: vec![],
			desired_maximum_frame_latency: 2,
		}
	}

	pub(crate) async fn create(window: Window, egui_ctx: &egui::Context) -> Result<Self, RendererError> {
		let window = Arc::new(window);
		let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
			backends: wgpu::Backends::all(),
			..Default::default()
		});
		let surface = instance.create_surface(window.clone())?;

		let (format, adapter, device, queue) = Self::init_wgpu(&instance, &surface).await?;

		let scale_factor = window.scale_factor();
		let size = window.inner_size();
		surface.configure(&device, &Self::surface_config(size, format));

		let gui_renderer = GuiRenderer::new(&window, &device, format, egui_ctx);

		Ok(Renderer {
			window: window.clone(),
			surface_configured: true,
			surface,
			adapter,
			device,
			queue,
			format,
			did_resize: false,
			size,
			scale_factor,
			gui_renderer,
		})
	}

	pub(crate) fn resize(&mut self, physical_size: PhysicalSize<u32>) {
		log::trace!(
			"resized: {} x {}",
			physical_size.width, physical_size.height
		);
		self.did_resize = true;
		self.size = physical_size;
	}

	pub(crate) fn rescale(&mut self, scale_factor: f64) {
		log::trace!("rescale: {}", scale_factor,);
		self.scale_factor = scale_factor;
	}

	pub(crate) fn render(&mut self, gui: &mut impl GuiView, poke_stick: &impl MainPokeStick) -> Result<(), RendererError> {
		if self.did_resize {
			self.surface
				.configure(&self.device, &Self::surface_config(self.size, self.format));
			self.did_resize = false;
		}

		match self.surface.get_current_texture() {
			Ok(frame) => {
				self.gui_renderer.prepare(&self.window, gui, poke_stick);

				let view = frame
					.texture
					.create_view(&wgpu::TextureViewDescriptor::default());

				let screen = egui_wgpu::ScreenDescriptor {
					size_in_pixels: [self.size.width, self.size.height],
					pixels_per_point: self.scale_factor as f32,
				};

				let mut encoder =
					self.device
						.create_command_encoder(&wgpu::wgt::CommandEncoderDescriptor {
							label: Some("Renderer encoder"),
						});

				{
					let _rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
						label: Some("Clear color"),
						color_attachments: &[Some(wgpu::RenderPassColorAttachment {
							view: &view,
							resolve_target: None,
							ops: wgpu::Operations {
								load: wgpu::LoadOp::Clear(wgpu::Color {
									r: 1.0,
									g: 1.0,
									b: 1.0,
									a: 1.0,
								}),
								store: wgpu::StoreOp::Store,
							},
						})],
						depth_stencil_attachment: None,
						timestamp_writes: None,
						occlusion_query_set: None,
					});
				}

				self.gui_renderer
					.render(&mut encoder, &self.device, &self.queue, &screen, &view);

				self.queue.submit(Some(encoder.finish()));

				frame.present();
			}
			Err(e @ wgpu::SurfaceError::OutOfMemory) => {
				log::error!("Swapchain error: {e}");
				return Err(e.into());
			}
			Err(e) => {
				log::warn!("Hopefully recoverable error in render: {e}");
			}
		}
		Ok(())
	}
}


#[cfg(test)]
mod tests {
	#[test]
	fn check_math() {
		let total = 32 * 17;
		let fps_a = (32 * 1000) / total;
		let fps_b = 1000 / (total / 32);

		assert_eq!(fps_a, fps_b);

		let a = 20;
		assert_eq!(a << 6, a * 64);
	}
}
