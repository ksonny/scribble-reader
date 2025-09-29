use egui::ClippedPrimitive;
use egui::Color32;
use egui::Context;
use egui::RichText;
use egui::TexturesDelta;
use egui::ViewportId;
use egui_wgpu::Renderer;
use egui_wgpu::ScreenDescriptor;
use egui_wgpu::wgpu;
use egui_winit::EventResponse;
use egui_winit::State;
use winit::event::WindowEvent;
use winit::window::Window;

struct Test {
	is_window_open: bool,
}

impl Test {
	fn new() -> Self {
		Self {
			is_window_open: true,
		}
	}

	fn draw(&mut self, ctx: &Context) {
		egui::TopBottomPanel::top("menubar_container").show(ctx, |ui| {
			egui::MenuBar::new().ui(ui, |ui| {
				ui.label(RichText::new("Hello world").color(Color32::RED));
				ui.menu_button("File", |ui| {
					if ui.button("About...").clicked() {
						self.is_window_open = true;
						ui.close();
					}
				});
			});
		});

		egui::Window::new("Hello, winit-wgpu-egui")
			.open(&mut self.is_window_open)
			.show(ctx, |ui| {
				ui.label(
					"This is the most basic example of how to use winit, wgpu and egui together.",
				);
				ui.label("Mandatory heart: ♥");

				ui.separator();

				ui.horizontal(|ui| {
					ui.spacing_mut().item_spacing.x /= 2.0;
					ui.label("Learn more about wgpu at");
					ui.hyperlink("https://docs.rs/winit");
				});
				ui.horizontal(|ui| {
					ui.spacing_mut().item_spacing.x /= 2.0;
					ui.label("Learn more about winit at");
					ui.hyperlink("https://docs.rs/wgpu");
				});
				ui.horizontal(|ui| {
					ui.spacing_mut().item_spacing.x /= 2.0;
					ui.label("Learn more about egui at");
					ui.hyperlink("https://docs.rs/egui");
				});
			});
	}
}

pub struct Gui {
	ctx: Context,
	state: State,
	renderer: Renderer,
	view: Test,
	paint_jobs: Vec<ClippedPrimitive>,
	textures: TexturesDelta,
}

impl Gui {
	pub fn new(
		window: &Window,
		device: &wgpu::Device,
		texture_format: wgpu::TextureFormat,
	) -> Self {
		let scale_factor = window.scale_factor();
		let max_texture_size = device.limits().max_texture_dimension_2d as usize;

		let egui_ctx = Context::default();
		let egui_state = egui_winit::State::new(
			egui_ctx.clone(),
			ViewportId::ROOT,
			window,
			Some(scale_factor as f32),
			window.theme(),
			Some(max_texture_size),
		);

		let renderer = Renderer::new(device, texture_format, None, 1, false);
		let textures = TexturesDelta::default();

		let view = Test::new();

		Self {
			ctx: egui_ctx,
			state: egui_state,
			renderer,
			view,
			paint_jobs: vec![],
			textures,
		}
	}

	pub(crate) fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> EventResponse {
		let response = self.state.on_window_event(window, event);
		if response.repaint {
			self.prepare(window);
		}
		response
	}

	pub(crate) fn prepare(&mut self, window: &Window) {
		let raw_input = self.state.take_egui_input(window);
		let output = self.ctx.run(raw_input, |egui_ctx| {
			self.view.draw(egui_ctx)
		});

		self.textures.append(output.textures_delta);
		self.state
			.handle_platform_output(window, output.platform_output);
		self.paint_jobs = self
			.ctx
			.tessellate(output.shapes, window.scale_factor() as f32);
	}

	pub(crate) fn render(
		&mut self,
		encoder: &mut wgpu::CommandEncoder,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		screen: &ScreenDescriptor,
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
