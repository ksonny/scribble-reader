mod fonts;
mod fps_calculator;
mod gestures;
mod renderer;
mod ui;
mod views;

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use config::ConfigError;
use scribe::BookId;
use scribe::LibraryBell;
use scribe::LibraryScribe;
use scribe::LibraryScribeAssistant;
use scribe::Location;
use scribe::RecordKeeper;
use scribe::RecordKeeperError;
use scribe::config::ScribeConfig;
use sculpter::SculpterFontErrors;
use sculpter::SculpterFonts;
use sculpter::SculpterFontsBuilder;
use winit::application::ApplicationHandler;
use winit::error::EventLoopError;
use winit::event::WindowEvent;
use winit::event_loop::EventLoop;
use winit::window::Window;
use wrangler::WranglerSystem;
use wrangler::content::ContentWrangler;
use wrangler::content::ContentWranglerAssistant;

use crate::fps_calculator::FpsCalculator;
use crate::gestures::Gesture;
use crate::gestures::GestureTracker;
use crate::renderer::Renderer;
use crate::renderer::RendererError;
use crate::ui::UiInput;
use crate::views::AppView;
use crate::views::EventResult;
use crate::views::ViewHandle;

struct App<'window> {
	input: UiInput,
	renderer: Option<Renderer<'window>>,
	view: AppView,
	bell: AppBell,
	egui_ctx: egui::Context,
	fps: FpsCalculator,
	request_redraw: Instant,
	gestures: GestureTracker<10>,
	config: ScribeConfig,
	fonts: SculpterFonts,
	keeper: RecordKeeper,
	scribe: LibraryScribeAssistant,
	content: ContentWranglerAssistant,
}

impl App<'_> {
	const ACTIVE_TICK: Duration = Duration::from_millis(32);
	const SLEEP_TICK: Duration = Duration::from_secs(10);
	const SLEEP_THRESHOLD: Duration = Duration::from_millis(256);

	fn request_redraw(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
		log::trace!("Request redraw");

		let now = Instant::now();
		self.request_redraw = now;

		// Wake the loop if needed
		let next_tick = now + Self::ACTIVE_TICK;
		match event_loop.control_flow() {
			winit::event_loop::ControlFlow::Poll | winit::event_loop::ControlFlow::Wait => {
				event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_tick));
			}
			winit::event_loop::ControlFlow::WaitUntil(instant) => {
				if instant > next_tick {
					event_loop
						.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_tick));
				}
			}
		}
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
				let next_tick = Instant::now() + Self::ACTIVE_TICK;
				event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_tick));
			}
			winit::event::StartCause::ResumeTimeReached {
				requested_resume, ..
			} => {
				let since_redraw_request = requested_resume.duration_since(self.request_redraw);
				if since_redraw_request < Self::SLEEP_THRESHOLD {
					log::trace!("Render active");
					if let Some(renderer) = self.renderer.as_mut() {
						renderer.request_redraw();
					}
					let next_tick = Instant::now() + Self::ACTIVE_TICK;
					event_loop
						.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_tick));
				} else {
					log::trace!("Render sleep");
					if let Some(renderer) = self.renderer.as_mut() {
						renderer.request_redraw();
					}
					let next_tick = Instant::now() + Self::SLEEP_TICK;
					event_loop
						.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_tick));
				}
			}
			_ => {}
		};
	}

	fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
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
			let display = event_loop.owned_display_handle();
			match pollster::block_on(Renderer::create(display, window, &self.egui_ctx)) {
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
				self.view.library(self.keeper.clone(), self.scribe.clone());
			}
			AppEvent::OpenReader(book_id) => {
				log::debug!("Open book {book_id:?}");
				self.view.reader(
					self.config.illustrator.clone(),
					self.keeper.clone(),
					self.fonts.clone(),
					self.content.clone(),
					self.bell.clone(),
					book_id,
				);
			}
			AppEvent::OpenExperiments => {
				log::debug!("Open experiments");
				self.view.experiments(self.fonts.clone());
			}
			AppEvent::Exit => {
				log::debug!("Exit");
				self.view.close();
				event_loop.exit();
			}
			event => {
				log::trace!("Forward user event: {event:?}");
				let result = self.view.event(&event);
				if matches!(result, EventResult::RequestRedraw) {
					self.request_redraw(event_loop);
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
				log::info!("Window close requested");
				self.renderer.take();
				event_loop.exit();
				return;
			}
			WindowEvent::Destroyed => {
				log::info!("Window destroyed");
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
			self.request_redraw(event_loop);
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
				self.view.resize(size.width, size.height);
				self.request_redraw(event_loop);
			}
			WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
				if let Some(renderer) = self.renderer.as_mut() {
					renderer.rescale(scale_factor)
				}
				self.input.rescale(scale_factor as f32);
				self.view.rescale(scale_factor as f32);
				self.request_redraw(event_loop);
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
					Err(e @ RendererError::SurfaceLost) => {
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
	BookUpdated(BookId),
	BookContentReady(BookId, Location),
	KeyUp,
	KeyDown,
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
	fn content_ready(&self, id: BookId, loc: Location) {
		self.send_event(AppEvent::BookContentReady(id, loc))
	}
}

impl LibraryBell for AppBell {
	fn book_updated(&self, book_id: BookId) {
		self.send_event(AppEvent::BookUpdated(book_id));
	}
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	EventLoop(#[from] EventLoopError),
	#[error(transparent)]
	SculpterFonts(#[from] SculpterFontErrors),
	#[error(transparent)]
	RecordKeeper(#[from] RecordKeeperError),
	#[error(transparent)]
	Config(#[from] ConfigError),
}

#[derive(Debug)]
pub struct Paths {
	pub cache_path: PathBuf,
	pub config_path: PathBuf,
	pub data_path: PathBuf,
}

pub fn start(
	paths: Paths,
	system: WranglerSystem,
	event_loop: EventLoop<AppEvent>,
) -> Result<(), Error> {
	fs::create_dir_all(paths.cache_path.as_path())?;
	fs::create_dir_all(paths.config_path.as_path())?;
	fs::create_dir_all(paths.data_path.as_path())?;

	let bell = AppBell::new(event_loop.create_proxy());
	let view = AppView::new(bell.clone());
	let egui_ctx = ui::create_egui_ctx();
	let input = UiInput::new(egui_ctx.clone());
	let fps = FpsCalculator::new();
	let gestures = GestureTracker::<_>::new();
	let config = ScribeConfig::load(paths.config_path.as_path())?;
	let keeper = RecordKeeper::new(paths.data_path.as_path());
	let scribe = LibraryScribe::create(
		system.clone(),
		bell.clone(),
		keeper.assistant()?,
		paths.cache_path.as_path(),
	);
	let content = ContentWrangler::create(system);

	let fonts = SculpterFontsBuilder::new("Literata", "Open Sans")
		.add_font(fonts::EB_GARAMOND_VF_TTF)?
		.add_font(fonts::EB_GARAMOND_ITALIC_VF_TTF)?
		.add_font(fonts::OPEN_SANS_VF_TTF)?
		.add_font(fonts::OPEN_SANS_ITALIC_VF_TTF)?
		.add_font(fonts::LITERATA_VF_TTF)?
		.add_font(fonts::LITERATA_ITALIC_VF_TTF)?
		.add_fallback(fonts::NOTO_EMOJI_VF_TTF)?
		.add_fallback(fonts::NOTO_SANS_MATH_TTF)?
		.add_fallback(fonts::NOTO_SANS_SYMBOLS_VF_TTF)?
		.build();

	bell.send_event(AppEvent::OpenLibrary);

	let mut app = App {
		input,
		renderer: None,
		view,
		bell,
		egui_ctx,
		fps,
		request_redraw: Instant::now(),
		gestures,
		config,
		fonts,
		keeper,
		scribe,
		content,
	};

	event_loop.run_app(&mut app)?;

	Ok(())
}
