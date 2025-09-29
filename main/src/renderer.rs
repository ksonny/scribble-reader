use log::error;
use log::info;
use log::trace;
use log::warn;
use winit::dpi::PhysicalSize;

use egui_wgpu::wgpu::{
	self,
};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use winit::event::WindowEvent;
use winit::event_loop::ControlFlow;
use winit::window::Window;

use crate::gui::Gui;

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
	gui: Gui,
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
			.expect("get preferred format");
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

	pub async fn initialize(window: Window) -> Result<Self, RendererError> {
		let window = Arc::new(window);
		let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
			backends: wgpu::Backends::all(),
			..Default::default()
		});
		let surface = instance.create_surface(window.clone())?;

		let (format, adapter, device, queue) = Self::init_wgpu(&instance, &surface).await?;

		let scale_factor = window.scale_factor();
		let size = window.inner_size();
		surface.configure(
			&device,
			&wgpu::SurfaceConfiguration {
				usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
				format,
				width: size.width,
				height: size.height,
				present_mode: wgpu::PresentMode::AutoVsync,
				alpha_mode: wgpu::CompositeAlphaMode::Auto,
				view_formats: vec![],
				desired_maximum_frame_latency: 10,
			},
		);

		let gui = Gui::new(&window, &device, format);

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
			gui,
		})
	}

	pub fn resize(&mut self) {
		self.surface.configure(
			&self.device,
			&wgpu::SurfaceConfiguration {
				usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
				format: self.format,
				width: self.size.width,
				height: self.size.height,
				present_mode: wgpu::PresentMode::AutoVsync,
				alpha_mode: wgpu::CompositeAlphaMode::Auto,
				view_formats: vec![],
				desired_maximum_frame_latency: 10,
			},
		);
	}

	pub fn render(&mut self) -> Result<(), RendererError> {
		match self.surface.get_current_texture() {
			Ok(frame) => {
				self.gui.prepare(&self.window);

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
									r: 0.5,
									g: 0.76,
									b: 0.5,
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

				self.gui
					.render(&mut encoder, &self.device, &self.queue, &screen, &view);

				self.queue.submit(Some(encoder.finish()));

				frame.present();
			}
			Err(e @ wgpu::SurfaceError::OutOfMemory) => {
				error!("Swapchain error: {e}");
				return Err(e.into());
			}
			Err(e) => {
				warn!("Error, request redraw: {e}");
				self.window.request_redraw();
			}
		}
		Ok(())
	}

	pub fn handle_event(
		&mut self,
		_event_loop: &egui_winit::winit::event_loop::ActiveEventLoop,
		window_id: egui_winit::winit::window::WindowId,
		event: &WindowEvent,
	) {
		if self.window.id() != window_id {
			trace!("event ignored, wrong window");
			return;
		}

		trace!("event: {event:?}");

		let response = self.gui.handle_event(&self.window, event);

		match event {
			WindowEvent::Resized(physical_size) => {
				info!(
					"resized: {} x {}",
					physical_size.width, physical_size.height
				);
				self.did_resize = true;
				self.size = *physical_size;
				self.window.request_redraw();
			}
			WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
				info!("rescale: {}", scale_factor,);
				self.scale_factor = *scale_factor;
				self.window.request_redraw();
			}
			WindowEvent::RedrawRequested => {
				if self.did_resize {
					self.resize();
					self.did_resize = false;
				}

				match self.render() {
					Ok(_) => {}
					Err(e) => {
						error!("Render gui failed: {e}");
						panic!("Render gui failed: {e}");
					}
				};
			}
			_ => {
				if response.repaint {
					trace!("Request redraw by egui: {event:?}");
					self.window.request_redraw();
				}
				if response.consumed {
					trace!("Event consumed by egui: {event:?}");
					// return;
				}
			}
		}
	}
}
