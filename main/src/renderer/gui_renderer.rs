use std::sync::Arc;

use egui_wgpu::wgpu::{
	self,
};
use egui_winit::winit;
use winit::window::Window;

pub struct Renderer {
	ctx: egui::Context,
	gui_renderer: egui_wgpu::Renderer,
	screen: egui_wgpu::ScreenDescriptor,
	paint_jobs: Vec<egui::ClippedPrimitive>,
	textures: egui::TexturesDelta,
	state: Option<(egui_winit::State, Arc<Window>)>,
}

impl Renderer {
	pub(crate) fn new(
		device: &wgpu::Device,
		format: wgpu::TextureFormat,
		ctx: egui::Context,
	) -> Self {
		let gui_renderer = egui_wgpu::Renderer::new(device, format, None, 1, false);
		let screen = egui_wgpu::ScreenDescriptor {
			size_in_pixels: [0, 0],
			pixels_per_point: 1.0,
		};
		let textures = egui::TexturesDelta::default();
		let paint_jobs = vec![];

		Self {
			ctx,
			gui_renderer,
			screen,
			textures,
			paint_jobs,
			state: None,
		}
	}

	pub(crate) fn resume(&mut self, device: &wgpu::Device, window: Arc<Window>) {
		let size = window.inner_size();
		let scale_factor = window.scale_factor();
		self.state = Some((
			egui_winit::State::new(
				self.ctx.clone(),
				egui::ViewportId::ROOT,
				&window,
				Some(scale_factor as f32),
				None,
				Some(device.limits().max_texture_dimension_2d as usize),
			),
			window,
		));
		self.resize(size.width, size.height);
		self.rescale(scale_factor);
	}

	pub(crate) fn suspend(&mut self) {
		self.state.take();
	}

	pub(crate) fn resize(&mut self, width: u32, height: u32) {
		self.screen.size_in_pixels = [width, height];
	}

	pub(crate) fn rescale(&mut self, scale_factor: f64) {
		self.screen.pixels_per_point = scale_factor as f32;
	}

	pub(crate) fn prepare(
		&mut self,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		output: egui::FullOutput,
	) {
		if let Some((state, window)) = self.state.as_mut() {
			state.handle_platform_output(window, output.platform_output);
		}
		self.textures.append(output.textures_delta);
		self.paint_jobs = self
			.ctx
			.tessellate(output.shapes, self.screen.pixels_per_point);
		for (id, image_delta) in &self.textures.set {
			self.gui_renderer
				.update_texture(device, queue, *id, image_delta);
		}
	}

	pub(crate) fn update_buffers(
		&mut self,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		encoder: &mut wgpu::CommandEncoder,
	) {
		self.gui_renderer
			.update_buffers(device, queue, encoder, &self.paint_jobs, &self.screen);
	}

	pub(crate) fn render(&self, rpass: &mut wgpu::RenderPass<'static>) {
		self.gui_renderer
			.render(rpass, &self.paint_jobs, &self.screen);
	}

	pub(crate) fn cleanup(&mut self) {
		let textures = std::mem::take(&mut self.textures);
		for id in &textures.free {
			self.gui_renderer.free_texture(id);
		}
	}
}
