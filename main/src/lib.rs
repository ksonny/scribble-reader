#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]

mod renderer;
mod ui;

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

#[derive(Default)]
struct App<'window> {
	renderer: Option<Renderer<'window>>,
	view: MainView,
}

impl<'window> ApplicationHandler for App<'window> {
	fn resumed(&mut self, event_loop: &egui_winit::winit::event_loop::ActiveEventLoop) {
		info!("Window resumed");
		let window = event_loop
			.create_window(Window::default_attributes())
			.unwrap();
		let renderer = match pollster::block_on(Renderer::create(window)) {
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

				match event {
					WindowEvent::Resized(physical_size) => {
						renderer.resize(physical_size);
					}
					WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
						renderer.rescale(scale_factor);
					}
					WindowEvent::RedrawRequested => {
						self.view.set_fps(renderer.fps());

						match renderer.render(&mut self.view) {
							Ok(_) => {}
							Err(e) => {
								error!("Failure during render: {e:?}");
								event_loop.exit();
							}
						}
					}
					_ => {
						if response.repaint {
							renderer.window.request_redraw();
						}
					}
				};
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
