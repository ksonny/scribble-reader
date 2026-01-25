#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]

mod fps_calculator;
mod gestures;
mod renderer;
mod ui;
mod views;

use std::time::Duration;
use std::time::Instant;

use illustrator::Illustrator;
use illustrator::spawn_illustrator;
use scribe::ScribeConfig;
use scribe::library::Location;
use winit::application::ApplicationHandler;
use winit::error::EventLoopError;
use winit::event::WindowEvent;
use winit::event_loop::EventLoop;
#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;
use winit::window::Window;

use crate::fps_calculator::FpsCalculator;
use crate::gestures::Gesture;
use crate::gestures::GestureTracker;
use crate::renderer::Renderer;
use crate::renderer::RendererError;
use crate::ui::UiInput;
use crate::views::AppView;
use crate::views::EventResult;
use crate::views::ViewHandle;
use scribe::Scribe;
use scribe::library;
use scribe::library::BookId;

struct App<'window> {
	input: UiInput,
	renderer: Option<Renderer<'window>>,
	scribe: Scribe,
	view: AppView,
	bell: AppBell,
	egui_ctx: egui::Context,
	fps: FpsCalculator,
	request_redraw: Instant,
	gestures: GestureTracker<10>,
	illustrator: Illustrator,
}

impl App<'_> {
	const ACTIVE_TICK: u64 = 32;
	const SLEEP_TIMEOUT: u64 = 256;

	fn request_redraw(&mut self) {
		log::trace!("Request redraw");
		self.request_redraw = Instant::now();
	}
}

impl<'window> ApplicationHandler<AppEvent> for App<'window> {
	fn new_events(
		&mut self,
		event_loop: &winit::event_loop::ActiveEventLoop,
		cause: winit::event::StartCause,
	) {
		match cause {
			winit::event::StartCause::Init => {
				if let Some(renderer) = self.renderer.as_mut() {
					renderer.request_redraw();
				}
			}
			winit::event::StartCause::ResumeTimeReached {
				requested_resume, ..
			} => {
				log::trace!("Resume time reached");
				let since_redraw_request = requested_resume
					.duration_since(self.request_redraw)
					.as_millis() as u64;
				if since_redraw_request < Self::SLEEP_TIMEOUT {
					log::trace!("Render full speed: {}", since_redraw_request);
					if let Some(renderer) = self.renderer.as_mut() {
						log::trace!("Render");
						renderer.request_redraw();
					}
					let next_tick = Instant::now() + Duration::from_millis(Self::ACTIVE_TICK);
					event_loop
						.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_tick));
				} else {
					log::trace!("Render sleep");
					event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
				}
			}
			winit::event::StartCause::WaitCancelled {
				requested_resume, ..
			} => {
				if requested_resume.is_none()
					&& let Some(renderer) = self.renderer.as_mut()
				{
					log::trace!("Wait cancelled from sleep");
					renderer.request_redraw();
					let next_tick = Instant::now() + Duration::from_millis(Self::ACTIVE_TICK);
					event_loop
						.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_tick));
				}
			}
			_ => {}
		};
	}

	fn resumed(&mut self, event_loop: &egui_winit::winit::event_loop::ActiveEventLoop) {
		log::info!("resumed");
		let window = event_loop
			.create_window(Window::default_attributes())
			.unwrap();
		window.set_title("Scribble-reader");

		let size = window.inner_size();
		let scale_factor = window.scale_factor() as f32;
		self.input.resume(size, scale_factor);
		self.gestures
			.set_min_distance_by_screen(size.width, size.height);
		self.illustrator.resize(size.width, size.height);
		self.illustrator.rescale(scale_factor);
		self.view.resize(size.width, size.height);
		self.view.rescale(scale_factor);

		if let Some(renderer) = self.renderer.as_mut() {
			match renderer.resume(window) {
				Ok(_) => {}
				Err(e) => {
					log::error!("Failed to resume renderer: {e}");
					panic!("Failed to resume renderer: {e}");
				}
			};
		} else {
			match pollster::block_on(Renderer::create(window, &self.egui_ctx)) {
				Ok(renderer) => self.renderer = Some(renderer),
				Err(e) => {
					log::error!("Failed to create renderer: {e}");
					panic!("Failed to create renderer: {e}");
				}
			}
		};
	}

	fn suspended(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
		log::info!("suspended");
		if let Some(renderer) = self.renderer.as_mut() {
			renderer.suspend()
		}
	}

	fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: AppEvent) {
		match event {
			AppEvent::OpenLibrary => {
				log::debug!("Open library");
				self.view.library(self.scribe.assistant());
			}
			AppEvent::OpenReader(book_id) => {
				log::debug!("Open book {book_id:?}");
				match spawn_illustrator(&mut self.illustrator, self.bell.clone(), book_id) {
					Ok(handle) => {
						self.view.reader(handle);
					}
					Err(e) => {
						log::error!("Error spawning illustrator: {e}");
					}
				};
			}
			AppEvent::OpenExperiments => {
				log::debug!("Open experiments");
				self.view.experiments();
			}
			AppEvent::Exit => {
				log::debug!("Exit");
				event_loop.exit();
			}
			event => {
				log::trace!("Forward user event: {event:?}");
				let result = self.view.event(&event);
				if matches!(result, EventResult::RequestRedraw) {
					self.request_redraw();
				}
			}
		}
	}

	fn window_event(
		&mut self,
		event_loop: &winit::event_loop::ActiveEventLoop,
		_window_id: winit::window::WindowId,
		event: winit::event::WindowEvent,
	) {
		match event {
			WindowEvent::CloseRequested => {
				log::info!("close requested");
				self.renderer.take();
				event_loop.exit();
				return;
			}
			WindowEvent::Destroyed => {
				log::info!("destroyed");
				self.renderer.take();
				return;
			}
			_ => {}
		};

		let result = self.gestures.handle_window_event(&event);
		if result.frame_ended {
			for event in self.gestures.events() {
				match event.gesture {
					Gesture::Tap => match self.view.gesture(&event) {
						views::GestureResult::Consumed => {}
						views::GestureResult::Unhandled => {
							self.input.handle_gesture(&event);
						}
					},
					_ => {
						self.view.gesture(&event);
					}
				}
			}
			self.gestures.reset();
			self.request_redraw();
		}

		log::trace!("event: {event:?}");
		match event {
			WindowEvent::Resized(size) => {
				if let Some(renderer) = self.renderer.as_mut() {
					renderer.resize(size)
				}
				self.gestures
					.set_min_distance_by_screen(size.width, size.height);
				self.input.resize(size);
				self.illustrator.resize(size.width, size.height);
				self.view.resize(size.width, size.height);
				self.request_redraw();
			}
			WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
				if let Some(renderer) = self.renderer.as_mut() {
					renderer.rescale(scale_factor)
				}
				self.input.rescale(scale_factor as f32);
				self.illustrator.rescale(scale_factor as f32);
				self.view.rescale(scale_factor as f32);
				self.request_redraw();
			}
			WindowEvent::RedrawRequested => {
				let Some(renderer) = self.renderer.as_mut() else {
					log::warn!("Renderer not initialized, abort event {event:?}");
					return;
				};

				let painter = renderer.painter(&mut self.input);
				self.view.draw(painter);

				match renderer.render() {
					Ok(()) => {
						self.fps.tick();
					}
					Err(e @ RendererError::SurfaceNotAvailable) => {
						log::warn!("Failure render: {e}");
					}
					Err(e) => {
						log::error!("Failure render: {e}");
						event_loop.exit();
					}
				}
			}
			_ => {}
		};
	}
}

#[derive(Debug, Clone, Copy)]
pub enum AppEvent {
	OpenLibrary,
	OpenExperiments,
	OpenReader(BookId),
	LibraryUpdated,
	LibraryBookUpdated(BookId),
	BookContentReady(BookId, Location),
	Exit,
}

#[derive(Clone)]
struct AppBell {
	proxy: winit::event_loop::EventLoopProxy<AppEvent>,
}

impl AppBell {
	fn new(proxy: winit::event_loop::EventLoopProxy<AppEvent>) -> Self {
		Self { proxy }
	}

	fn send_event(&self, event: AppEvent) {
		self.proxy.send_event(event).unwrap();
	}
}

impl illustrator::Bell for AppBell {
	fn content_ready(&self, id: library::BookId, loc: Location) {
		self.send_event(AppEvent::BookContentReady(id, loc))
	}
}

impl scribe::Bell for AppBell {
	fn library_updated(&self, book_id: Option<BookId>) {
		if let Some(book_id) = book_id {
			self.send_event(AppEvent::LibraryBookUpdated(book_id));
		} else {
			self.send_event(AppEvent::LibraryUpdated);
		}
	}
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	EventLoop(#[from] EventLoopError),
	#[error(transparent)]
	ScribeCreate(#[from] scribe::ScribeCreateError),
	#[error(transparent)]
	Scribe(#[from] scribe::ScribeError),
}

pub fn start(event_loop: EventLoop<AppEvent>, config: ScribeConfig) -> Result<(), Error> {
	let bell = AppBell::new(event_loop.create_proxy());
	let view = AppView::new(bell.clone());

	let egui_ctx = ui::create_egui_ctx();
	let input = UiInput::new(egui_ctx.clone());
	let fps = FpsCalculator::new();
	let gestures = GestureTracker::<_>::new();

	let scribe = Scribe::create(bell.clone(), config.clone())?;
	let illustrator = Illustrator::create(config);

	let mut app = App {
		input,
		renderer: None,
		scribe,
		view,
		bell,
		egui_ctx,
		fps,
		request_redraw: Instant::now(),
		gestures,
		illustrator,
	};

	event_loop.run_app(&mut app)?;

	app.scribe.quit()?;

	Ok(())
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
	use android_logger::Config;
	use scribe::settings::Paths;
	use winit::platform::android::EventLoopBuilderExtAndroid;

	android_logger::init_once(
		Config::default()
			.with_tag("scribble-reader")
			.with_max_level(log::LevelFilter::Info),
	);

	let ext_data_path = app.external_data_path().unwrap();
	let paths = Paths {
		cache_path: ext_data_path.parent().unwrap().join("cache"),
		config_path: ext_data_path.join("config"),
		data_path: ext_data_path.join("data"),
	};
	let config = ScribeConfig::new(paths);

	let event_loop = EventLoop::with_user_event()
		.with_android_app(app)
		.build()
		.unwrap();
	match start(event_loop, config) {
		Ok(_) => {}
		Err(e) => log::error!("Error: {e}"),
	}
}
