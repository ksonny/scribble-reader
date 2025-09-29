#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]

mod renderer;
mod ui;

use std::time::Duration;
use std::time::Instant;

use log::error;
use log::info;
use log::trace;
use log::warn;
use winit::error::EventLoopError;
use winit::event::WindowEvent;
#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

use winit::application::ApplicationHandler;
use winit::event_loop::EventLoop;
use winit::window::Window;

use crate::renderer::Renderer;
use crate::ui::MainView;

struct FpsCalculator {
	last_frame: Instant,
	total_ms: u64,
}

impl FpsCalculator {
	const FRAME_MS: u64 = 16;
	const DIVIDER_2: u64 = 5;

	fn new() -> FpsCalculator {
		FpsCalculator {
			last_frame: Instant::now(),
			total_ms: 0,
		}
	}

	fn tick(&mut self) {
		let instant = Instant::now();
		let frame = instant.duration_since(self.last_frame).as_millis() as u64;
		let avg = self.total_ms >> Self::DIVIDER_2;
		self.total_ms = self.total_ms + frame - avg;
		self.last_frame = instant;
	}

	fn next_frame(&self) -> Instant {
		self.last_frame + Duration::from_millis(Self::FRAME_MS)
	}

	fn fps(&self) -> u64 {
		(1000_u64 << Self::DIVIDER_2).checked_div(self.total_ms).unwrap_or(0_u64)
	}
}

struct App<'window> {
	renderer: Option<Renderer<'window>>,
	view: MainView,
	egui_ctx: egui::Context,
	fps: FpsCalculator,
	request_redraw: bool,
	wait_cancelled: bool,
}

impl<'window> ApplicationHandler for App<'window> {
	fn new_events(
		&mut self,
		_event_loop: &winit::event_loop::ActiveEventLoop,
		cause: winit::event::StartCause,
	) {
		self.wait_cancelled = matches!(cause, winit::event::StartCause::WaitCancelled { .. });
	}

	fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
		if self.request_redraw
			&& !self.wait_cancelled
			&& let Some(renderer) = self.renderer.as_mut()
		{
			renderer.window.request_redraw();
		}

		if !self.wait_cancelled {
			event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
				self.fps.next_frame(),
			));
		}
	}

	fn resumed(&mut self, event_loop: &egui_winit::winit::event_loop::ActiveEventLoop) {
		trace!("Window resumed");
		let window = event_loop
			.create_window(Window::default_attributes())
			.unwrap();
		let renderer = match pollster::block_on(Renderer::create(window, &self.egui_ctx)) {
			Ok(renderer) => renderer,
			Err(e) => {
				error!("Failed to resume renderer: {e}");
				panic!("Failed to resume renderer: {e}");
			}
		};
		self.renderer = Some(renderer);
	}

	fn window_event(
		&mut self,
		event_loop: &egui_winit::winit::event_loop::ActiveEventLoop,
		window_id: egui_winit::winit::window::WindowId,
		event: egui_winit::winit::event::WindowEvent,
	) {
		match event {
			WindowEvent::CloseRequested => {
				info!("close requested");
				self.renderer = None;
				event_loop.exit();
			}
			event => {
				let Some(renderer) = self.renderer.as_mut() else {
					warn!("renderer not initialized");
					return;
				};

				if renderer.window.id() != window_id {
					trace!("event ignored, wrong window");
					return;
				}

				trace!("event: {event:?}");

				let response = renderer.gui_renderer.handle_event(&renderer.window, &event);
				if response.repaint {
					self.request_redraw = true;
				}

				match event {
					WindowEvent::Resized(physical_size) => {
						renderer.resize(physical_size);
						self.request_redraw = true;
					}
					WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
						renderer.rescale(scale_factor);
						self.request_redraw = true;
					}
					WindowEvent::RedrawRequested => {
						self.fps.tick();
						self.view.set_fps(self.fps.fps());

						match renderer.render(&mut self.view) {
							Ok(_) => {}
							Err(e) => {
								error!("Failure during render: {e:?}");
								event_loop.exit();
							}
						}
					}
					_ => {}
				};
			}
		}
	}
}

pub fn start(event_loop: EventLoop<()>) -> Result<(), EventLoopError> {
	let view = MainView::default();
	let egui_ctx = egui::Context::default();

	egui_extras::install_image_loaders(&egui_ctx);
	egui_ctx.add_font(egui::epaint::text::FontInsert::new(
		"lucide-icons",
		egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES),
		vec![
			egui::epaint::text::InsertFontFamily {
				family: ui::ICON_FONT_FAMILY.clone(),
				priority: egui::epaint::text::FontPriority::Lowest,
			}
		],
	));
	let fps = FpsCalculator::new();

	let mut app = App {
		renderer: None,
		view,
		egui_ctx,
		fps,
		request_redraw: false,
		wait_cancelled: false,
	};

	event_loop.run_app(&mut app)
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: AndroidApp) {
	use android_logger::Config;
	use winit::event_loop::EventLoopBuilder;
	use winit::platform::android::EventLoopBuilderExtAndroid;

	android_logger::init_once(Config::default().with_max_level(log::LevelFilter::Info));
	let event_loop = EventLoopBuilder::new()
		.with_android_app(app)
		.build()
		.unwrap();
	log::info!("Hello from android!");
	start(event_loop);
}
