use egui::TexturesDelta;
use winit::dpi::PhysicalSize;

use egui_wgpu::wgpu::{
	self,
};
use std::sync::Arc;
use winit::event::WindowEvent;
use winit::window::Window;

use crate::ui::GuiView;
use crate::ui::PokeStick;

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
	screen: egui_wgpu::ScreenDescriptor,
	egui_state: egui_winit::State,
}

impl<'window> SurfaceState<'window> {
	fn setup_swapchain(&self, device: &wgpu::Device, size: PhysicalSize<u32>) {
		let surface_configuration = wgpu::SurfaceConfiguration {
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
			format: self.format,
			width: size.width,
			height: size.height,
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
	gui_renderer: egui_wgpu::Renderer,
	paint_jobs: Vec<egui::ClippedPrimitive>,
	textures: egui::TexturesDelta,
	surface_state: Option<SurfaceState<'window>>,
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
				required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
				memory_hints: wgpu::MemoryHints::MemoryUsage,
				trace: wgpu::Trace::Off,
			})
			.await?;

		let cap = surface.get_capabilities(&adapter);
		let format = cap
			.formats
			.first()
			.cloned()
			.ok_or(RendererError::NoTextureFormat)?;
		let alpha_mode = cap
			.alpha_modes
			.first()
			.cloned()
			.ok_or(RendererError::NoAlphaMode)?;
		let gui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
		let textures = TexturesDelta::default();
		let paint_jobs = vec![];

		let size = window.inner_size();
		let scale_factor = window.scale_factor();

		let egui_state = egui_winit::State::new(
			egui_ctx.clone(),
			egui::ViewportId::ROOT,
			&window,
			Some(window.scale_factor() as f32),
			None,
			Some(device.limits().max_texture_dimension_2d as usize),
		);
		let screen = egui_wgpu::ScreenDescriptor {
			size_in_pixels: [size.width, size.height],
			pixels_per_point: scale_factor as f32,
		};

		let surface_state = SurfaceState {
			window,
			surface,
			format,
			alpha_mode,
			screen,
			egui_state,
		};
		surface_state.setup_swapchain(&device, size);

		Ok(Renderer {
			instance,
			adapter,
			device,
			queue,
			gui_renderer,
			textures,
			paint_jobs,
			surface_state: Some(surface_state),
		})
	}

	pub(crate) fn resume(
		&mut self,
		window: Window,
		egui_ctx: &egui::Context,
	) -> Result<(), RendererError> {
		if self.surface_state.is_some() {
			// Already initailized
			return Ok(());
		}

		let window = Arc::new(window);
		let size = window.inner_size();
		let scale_factor = window.scale_factor();
		let surface = self.instance.create_surface(window.clone())?;

		let cap = surface.get_capabilities(&self.adapter);
		let format = cap
			.formats
			.first()
			.cloned()
			.ok_or(RendererError::NoTextureFormat)?;
		let alpha_mode = cap
			.alpha_modes
			.first()
			.cloned()
			.ok_or(RendererError::NoAlphaMode)?;

		let egui_state = egui_winit::State::new(
			egui_ctx.clone(),
			egui::ViewportId::ROOT,
			&window,
			Some(window.scale_factor() as f32),
			None,
			Some(self.device.limits().max_texture_dimension_2d as usize),
		);
		let screen = egui_wgpu::ScreenDescriptor {
			size_in_pixels: [size.width, size.height],
			pixels_per_point: scale_factor as f32,
		};

		let surface_state = SurfaceState {
			window,
			surface,
			format,
			alpha_mode,
			screen,
			egui_state,
		};
		surface_state.setup_swapchain(&self.device, size);
		self.surface_state = Some(surface_state);

		Ok(())
	}

	pub(crate) fn suspend(&mut self) {
		self.surface_state.take();
	}

	pub(crate) fn handle_gui_event(&mut self, event: &WindowEvent) -> egui_winit::EventResponse {
		if let Some(surface_state) = self.surface_state.as_mut() {
			surface_state
				.egui_state
				.on_window_event(&surface_state.window, event)
		} else {
			egui_winit::EventResponse {
				repaint: false,
				consumed: false,
			}
		}
	}

	pub(crate) fn resize(&mut self, size: PhysicalSize<u32>) {
		if let Some(surface_state) = self.surface_state.as_mut() {
			surface_state.screen.size_in_pixels = [size.width, size.height];
			surface_state.setup_swapchain(&self.device, size);
		}
	}

	pub(crate) fn rescale(&mut self, scale_factor: f64) {
		if let Some(surface_state) = self.surface_state.as_mut() {
			surface_state.screen.pixels_per_point = scale_factor as f32;
		}
	}

	pub(crate) fn request_redraw(&self) {
		if let Some(surface_state) = self.surface_state.as_ref() {
			surface_state.window.request_redraw();
		}
	}

	pub(crate) fn prepare(
		&mut self,
		ctx: &egui::Context,
		poke_stick: &impl PokeStick,
		view: &mut impl GuiView,
	) -> Result<(), RendererError> {
		let surface_state = self
			.surface_state
			.as_mut()
			.ok_or(RendererError::SurfaceNotAvailable)?;
		let window = &surface_state.window;
		let state = &mut surface_state.egui_state;

		let raw_input = state.take_egui_input(window);
		let output = ctx.run(raw_input, |egui_ctx| view.draw(egui_ctx, poke_stick));
		state.handle_platform_output(window, output.platform_output);

		self.textures.append(output.textures_delta);
		self.paint_jobs = ctx.tessellate(output.shapes, window.scale_factor() as f32);

		Ok(())
	}

	pub(crate) fn render(&mut self) -> Result<(), RendererError> {
		let surface_state = self
			.surface_state
			.as_ref()
			.ok_or(RendererError::SurfaceNotAvailable)?;
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

				{
					for (id, image_delta) in &self.textures.set {
						self.gui_renderer.update_texture(
							&self.device,
							&self.queue,
							*id,
							image_delta,
						);
					}

					self.gui_renderer.update_buffers(
						&self.device,
						&self.queue,
						&mut encoder,
						&self.paint_jobs,
						&surface_state.screen,
					);

					let mut rpass = encoder
						.begin_render_pass(&wgpu::RenderPassDescriptor {
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
						})
						.forget_lifetime();

					self.gui_renderer
						.render(&mut rpass, &self.paint_jobs, &surface_state.screen);
				}

				let textures = std::mem::take(&mut self.textures);
				for id in &textures.free {
					self.gui_renderer.free_texture(id);
				}

				self.queue.submit(Some(encoder.finish()));

				frame.present();
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
