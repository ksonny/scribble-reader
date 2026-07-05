pub(crate) mod gui_renderer;
pub(crate) mod painter;

use pixelator::PixelatorAssistant;
use winit::dpi::PhysicalSize;
use winit::event_loop::OwnedDisplayHandle;

use egui_wgpu::wgpu::{
	self,
};
use std::sync::Arc;
use winit::window::Window;

pub(crate) use crate::renderer::painter::Painter;
use crate::ui::UiInput;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RendererError {
	#[error(transparent)]
	CreateSurface(#[from] wgpu::CreateSurfaceError),
	#[error(transparent)]
	RequestDevice(#[from] wgpu::RequestDeviceError),
	#[error(transparent)]
	RequestAdapter(#[from] wgpu::wgt::RequestAdapterError),
	#[error("Failed to get surface format")]
	NoTextureFormat,
	#[error("Surface not available, probably suspended")]
	SurfaceNotAvailable,
	#[error("Surface lost and failed to recreate")]
	SurfaceLostUnrecoverably,
}

struct SurfaceState<'window> {
	window: Arc<Window>,
	format: wgpu::TextureFormat,
	surface: wgpu::Surface<'window>,
}

impl<'window> SurfaceState<'window> {
	fn configure_surface(
		&self,
		adapter: &wgpu::Adapter,
		device: &wgpu::Device,
		width: u32,
		height: u32,
	) {
		let surface_configuration = wgpu::SurfaceConfiguration {
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
			format: self.format,
			view_formats: vec![self.format],
			..self
				.surface
				.get_default_config(adapter, width, height)
				.expect("This surface isn't supported by adapter")
		};
		self.surface.configure(device, &surface_configuration);
	}
}

pub(crate) enum RenderResult {
	Success,
	Reconfigured,
	FrameSkipped,
}

pub(crate) struct Renderer<'window> {
	instance: wgpu::Instance,
	adapter: wgpu::Adapter,
	device: wgpu::Device,
	queue: wgpu::Queue,
	pixmap_renderer: pixelator::Renderer,
	gui_renderer: gui_renderer::Renderer,
	surface_state: Option<SurfaceState<'window>>,
	resized: Option<PhysicalSize<u32>>,
}

impl Renderer<'_> {
	pub(crate) async fn create(
		display: OwnedDisplayHandle,
		window: Window,
		egui_ctx: &egui::Context,
	) -> Result<Self, RendererError> {
		let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
			Box::new(display),
		));
		let adapter = instance
			.request_adapter(&wgpu::RequestAdapterOptions::default())
			.await?;
		let (device, queue) = adapter
			.request_device(&wgpu::DeviceDescriptor::default())
			.await?;

		let window = Arc::new(window);
		let surface = instance.create_surface(window.clone())?;
		let format = surface_format(surface.get_capabilities(&adapter))?;
		let size = window.inner_size();

		let pixmap_renderer = pixelator::Renderer::new(
			device.clone(),
			queue.clone(),
			format,
			size.width,
			size.height,
		);

		let mut gui_renderer =
			gui_renderer::Renderer::new(device.clone(), queue.clone(), format, egui_ctx.clone());
		gui_renderer.resume(window.clone());

		let surface_state = SurfaceState {
			window,
			format,
			surface,
		};
		surface_state.configure_surface(&adapter, &device, size.width, size.height);

		Ok(Renderer {
			instance,
			adapter,
			device,
			queue,
			pixmap_renderer,
			gui_renderer,
			resized: None,
			surface_state: Some(surface_state),
		})
	}

	pub(crate) fn resume(&mut self, window: Window) -> Result<(), RendererError> {
		if self.surface_state.is_some() {
			// Already initailized
			return Ok(());
		}

		let window = Arc::new(window);
		let size = window.inner_size();
		let surface = self.instance.create_surface(window.clone())?;
		let format = surface_format(surface.get_capabilities(&self.adapter))?;

		self.pixmap_renderer.resize(size.width, size.height);
		self.gui_renderer.resume(window.clone());

		let surface_state = SurfaceState {
			window,
			format,
			surface,
		};
		surface_state.configure_surface(&self.adapter, &self.device, size.width, size.height);
		self.surface_state = Some(surface_state);

		Ok(())
	}

	pub(crate) fn suspend(&mut self) {
		self.gui_renderer.suspend();
		self.surface_state.take();
	}

	pub(crate) fn resize(&mut self, size: PhysicalSize<u32>) {
		self.resized = Some(size);

		self.gui_renderer.resize(size.width, size.height);
		self.pixmap_renderer.resize(size.width, size.height);
	}

	pub(crate) fn rescale(&mut self, scale_factor: f64) {
		self.gui_renderer.rescale(scale_factor);
	}

	pub(crate) fn request_redraw(&self) {
		if let Some(surface_state) = self.surface_state.as_ref() {
			surface_state.window.request_redraw();
		}
	}

	pub(crate) fn render(&mut self) -> Result<RenderResult, RendererError> {
		let surface_state = self
			.surface_state
			.as_mut()
			.ok_or(RendererError::SurfaceNotAvailable)?;
		if let Some(size) = self.resized {
			surface_state.configure_surface(&self.adapter, &self.device, size.width, size.height);
		}

		let frame = match surface_state.surface.get_current_texture() {
			wgpu::CurrentSurfaceTexture::Success(frame) => frame,
			wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
				let size = surface_state.window.inner_size();
				surface_state.configure_surface(
					&self.adapter,
					&self.device,
					size.width,
					size.height,
				);
				frame
			}
			wgpu::CurrentSurfaceTexture::Outdated => {
				let size = surface_state.window.inner_size();
				surface_state.configure_surface(
					&self.adapter,
					&self.device,
					size.width,
					size.height,
				);
				return Ok(RenderResult::Reconfigured);
			}
			wgpu::CurrentSurfaceTexture::Lost => {
				if let Some(SurfaceState { window, format, .. }) = self.surface_state.take() {
					let size = window.inner_size();
					let surface = self.instance.create_surface(window.clone())?;
					let surface_state = SurfaceState {
						window,
						format,
						surface,
					};
					surface_state.configure_surface(
						&self.adapter,
						&self.device,
						size.width,
						size.height,
					);
					self.surface_state = Some(surface_state);
					return Ok(RenderResult::Reconfigured);
				} else {
					// Create surface from scratch
					return Err(RendererError::SurfaceLostUnrecoverably);
				}
			}
			wgpu::CurrentSurfaceTexture::Timeout
			| wgpu::CurrentSurfaceTexture::Occluded
			| wgpu::CurrentSurfaceTexture::Validation => return Ok(RenderResult::FrameSkipped),
		};

		let view = frame
			.texture
			.create_view(&wgpu::TextureViewDescriptor::default());

		let mut encoder =
			self.device
				.create_command_encoder(&wgpu::wgt::CommandEncoderDescriptor {
					label: Some("render encoder"),
				});

		self.gui_renderer
			.update_buffers(&self.device, &self.queue, &mut encoder);

		let mut rpass = encoder
			.begin_render_pass(&wgpu::RenderPassDescriptor {
				label: Some("main render pass"),
				color_attachments: &[Some(wgpu::RenderPassColorAttachment {
					view: &view,
					depth_slice: None,
					resolve_target: None,
					ops: wgpu::Operations {
						load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
						store: wgpu::StoreOp::Store,
					},
				})],
				depth_stencil_attachment: None,
				timestamp_writes: None,
				occlusion_query_set: None,
				multiview_mask: None,
			})
			.forget_lifetime();

		self.pixmap_renderer.render(&mut rpass);
		self.gui_renderer.render(&mut rpass);

		drop(rpass);
		self.queue.submit(Some(encoder.finish()));
		frame.present();

		self.gui_renderer.cleanup();

		Ok(RenderResult::Success)
	}

	pub(crate) fn painter<'a>(&'a mut self, ui_input: &'a mut UiInput) -> Painter<'a> {
		Painter::new(ui_input, &mut self.gui_renderer, &mut self.pixmap_renderer)
	}

	pub(crate) fn pixelator(&self) -> PixelatorAssistant {
		self.pixmap_renderer.assistant()
	}
}

fn surface_format(cap: wgpu::SurfaceCapabilities) -> Result<wgpu::TextureFormat, RendererError> {
	let format = cap
		.formats
		.iter()
		.find(|f| matches!(f, wgpu::TextureFormat::Rgba8Unorm))
		.or_else(|| cap.formats.first())
		.cloned()
		.ok_or(RendererError::NoTextureFormat)?;
	Ok(format)
}
