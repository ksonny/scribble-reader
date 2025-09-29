#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]

mod gui;
mod renderer;

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

#[derive(Default)]
struct App<'window> {
	renderer: Option<Renderer<'window>>,
}

impl<'window> ApplicationHandler for App<'window> {
	fn resumed(&mut self, event_loop: &egui_winit::winit::event_loop::ActiveEventLoop) {
		info!("Window resumed");
		let window = event_loop
			.create_window(Window::default_attributes())
			.unwrap();
		let renderer = match pollster::block_on(Renderer::initialize(window)) {
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
				renderer.handle_event(
					event_loop,
					window_id,
					&event
				);
			}
		}
	}
}

pub fn start(event_loop: EventLoop<()>) -> Result<(), EventLoopError> {
	event_loop.run_app(&mut App::default())
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
