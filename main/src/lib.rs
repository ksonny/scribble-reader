#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]

mod active_areas;
mod gestures;
mod renderer;
mod ui;

use std::time::Duration;
use std::time::Instant;

use illustrator::Illustrator;
use illustrator::RenderSettings;
use illustrator::RenderTextSettings;
use illustrator::spawn_illustrator;
use scribe::library::Location;
use winit::application::ApplicationHandler;
use winit::error::EventLoopError;
use winit::event::WindowEvent;
use winit::event_loop::EventLoop;
use winit::event_loop::EventLoopProxy;
#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;
use winit::window::Window;

use crate::active_areas::ActiveAreaAction;
use crate::active_areas::ActiveAreas;
use crate::gestures::Direction;
use crate::gestures::Gesture;
use crate::gestures::GestureTracker;
use crate::renderer::Renderer;
use crate::renderer::RendererError;
use crate::ui::GuiView as _;
use crate::ui::MainView;
use crate::ui::UiInput;
use scribe::Scribe;
use scribe::ScribeAssistant;
use scribe::library;
use scribe::library::BookId;

struct FpsCalculator {
	last_frame: Instant,
	total_ms: u64,
}

impl FpsCalculator {
	const DIVIDER_2: u64 = 3;

	fn new() -> Self {
		Self {
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

	#[allow(unused)]
	fn fps(&self) -> u64 {
		(1000_u64 << Self::DIVIDER_2)
			.checked_div(self.total_ms)
			.unwrap_or(0)
	}
}

struct App<'window> {
	input: UiInput,
	renderer: Option<Renderer<'window>>,
	scribe: Scribe,
	view: MainView,
	bell: EventLoopBell,
	egui_ctx: egui::Context,
	fps: FpsCalculator,
	request_redraw: Instant,
	gestures: GestureTracker<10>,
	illustrator: Illustrator,
	areas: ActiveAreas,
}

impl App<'_> {
	const ACTIVE_TICK: u64 = 32;
	const SLEEP_TIMEOUT: u64 = 256;

	fn request_redraw(&mut self) {
		log::trace!("Request redraw");
		self.request_redraw = Instant::now();
	}
}

impl<'window> ApplicationHandler<AppPoke> for App<'window> {
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

					self.illustrator.refresh_if_needed();
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
		self.areas = ActiveAreas::new(size.width, size.height);

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

	fn user_event(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop, event: AppPoke) {
		log::trace!("user event: {event:?}");
		match event {
			AppPoke::ScanLibrary => {
				self.view.working = self.scribe.poke_scan();
			}
			AppPoke::LibraryLoad | AppPoke::LibrarySorted => {
				self.view.library_loaded(&mut self.scribe);
				self.request_redraw();
			}
			AppPoke::LibraryOpen => {
				self.view.open_library();
				self.view.library_loaded(&mut self.scribe);
				self.request_redraw();
			}
			AppPoke::LibraryUpdated(id) => {
				self.view.book_updated(&self.scribe, id);
				self.request_redraw();
			}
			AppPoke::NextPage => {
				self.view.next_page(&mut self.scribe);
				self.request_redraw();
			}
			AppPoke::PreviousPage => {
				self.view.previous_page(&mut self.scribe);
				self.request_redraw();
			}
			AppPoke::Goto(location) => {
				self.view.goto(location);
				self.request_redraw();
			}
			AppPoke::OpenBook(book_id) => {
				match spawn_illustrator(&mut self.illustrator, self.bell.clone(), book_id) {
					Ok(handle) => {
						self.view.open_book(handle);
						self.request_redraw();
					}
					Err(e) => {
						log::error!("Error spawning illustrator: {e}");
					}
				};
				if let Some(renderer) = &mut self.renderer {
					match renderer.prepare_page(&mut self.illustrator.font_system().unwrap(), []) {
						Ok(_) => {}
						Err(e) => {
							log::error!("Prepare failed: {e}");
						}
					};
				}
			}
			AppPoke::ToggleToC => {
				self.view.toggle_toc();
				self.request_redraw();
			}
			AppPoke::BookContentReady(book_id, loc) => {
				log::trace!("Book content ready {book_id:?} {loc}",);
				if let Some(renderer) = &mut self.renderer {
					let state = self.illustrator.state().unwrap();
					if let Some(page) = state.page(loc) {
						log::trace!("Fond page, render {} items", page.items.len());
						match renderer.prepare_page(
							&mut self.illustrator.font_system().unwrap(),
							page.items.iter().map(|d| match d {
								illustrator::DisplayItem::Text(item) => glyphon::TextArea {
									buffer: &item.buffer,
									left: item.pos.x,
									top: item.pos.y,
									scale: 1.0,
									bounds: glyphon::TextBounds::default(),
									default_color: glyphon::Color::rgb(0, 0, 0),
									custom_glyphs: &[],
								},
							}),
						) {
							Ok(_) => {}
							Err(e) => {
								log::error!("Prepare failed: {e}");
							}
						};
					} else {
						log::trace!("No page found");
					}
				}
				self.request_redraw();
			}
			AppPoke::Completed(ticket) => {
				self.view.working = self.scribe.complete_ticket(ticket);
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
					Gesture::Swipe(Direction::Right, _) => {
						self.view.previous_page(&mut self.scribe);
					}
					Gesture::Swipe(Direction::Left, _) => {
						self.view.next_page(&mut self.scribe);
					}
					Gesture::Tap => {
						let pos = self.input.translate_pos(event.loc);
						if self.view.is_inside_ui_element(pos) {
							self.input.handle_gesture(&event);
						} else if let Some(action) = self.areas.action(event.loc) {
							match action {
								ActiveAreaAction::ToggleUi => self.view.toggle_ui(),
								ActiveAreaAction::NextPage => self.view.next_page(&mut self.scribe),
								ActiveAreaAction::PreviousPage => {
									self.view.previous_page(&mut self.scribe)
								}
							}
						}
					}
					_ => {}
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
				self.areas = ActiveAreas::new(size.width, size.height);
				self.request_redraw();
			}
			WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
				if let Some(renderer) = self.renderer.as_mut() {
					renderer.rescale(scale_factor)
				}
				self.input.rescale(scale_factor as f32);
				self.illustrator.rescale(scale_factor as f32);
				self.request_redraw();
			}
			WindowEvent::RedrawRequested => {
				let Some(renderer) = self.renderer.as_mut() else {
					log::warn!("Renderer not initialized, abort event {event:?}");
					return;
				};
				self.fps.tick();
				let input = self.input.tick();
				let output = self
					.egui_ctx
					.run(input, |ctx| self.view.draw(ctx, &self.bell));
				renderer.prepare_ui(output);
				match renderer.render() {
					Ok(_) => {}
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

#[derive(Debug)]
pub enum AppPoke {
	LibraryLoad,
	LibrarySorted,
	LibraryUpdated(BookId),
	NextPage,
	PreviousPage,
	Goto(Location),
	OpenBook(BookId),
	ToggleToC,
	LibraryOpen,
	ScanLibrary,
	BookContentReady(BookId, Location),
	Completed(scribe::ScribeTicket),
}

#[derive(Clone)]
struct EventLoopBell(EventLoopProxy<AppPoke>);

impl ui::Bell for EventLoopBell {
	fn scan_library(&self) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::ScanLibrary).unwrap();
	}

	fn next_page(&self) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::NextPage).unwrap();
	}

	fn previous_page(&self) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::PreviousPage).unwrap();
	}

	fn goto_location(&self, loc: Location) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::Goto(loc)).unwrap();
	}

	fn open_book(&self, id: BookId) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::OpenBook(id)).unwrap();
	}

	fn open_library(&self) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::LibraryOpen).unwrap();
	}

	fn toggle_chapters(&self) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::ToggleToC).unwrap();
	}
}

impl illustrator::Bell for EventLoopBell {
	fn content_ready(&self, id: library::BookId, loc: Location) {
		let EventLoopBell(proxy) = self;
		proxy
			.send_event(AppPoke::BookContentReady(id, loc))
			.unwrap();
	}
}

impl scribe::Bell for EventLoopBell {
	fn library_loaded(&self) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::LibraryLoad).unwrap();
	}

	fn library_sorted(&self) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::LibrarySorted).unwrap();
	}

	fn book_updated(&self, id: BookId) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::LibraryUpdated(id)).unwrap();
	}

	fn fail(&self, ticket: scribe::ScribeTicket, error: String) {
		log::error!("Error in scribe: {error}");
		let EventLoopBell(proxy) = self;
		// TODO: Maybe other event?
		proxy.send_event(AppPoke::Completed(ticket)).unwrap();
	}

	fn complete(&self, ticket: scribe::ScribeTicket) {
		let EventLoopBell(proxy) = self;
		proxy.send_event(AppPoke::Completed(ticket)).unwrap();
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

pub fn start(event_loop: EventLoop<AppPoke>, settings: scribe::Settings) -> Result<(), Error> {
	let bell = EventLoopBell(event_loop.create_proxy());
	let scribe = Scribe::create(bell.clone(), &settings)?;
	let view = MainView::default();

	let egui_ctx = ui::create_egui_ctx();
	let input = UiInput::new();
	let fps = FpsCalculator::new();
	let gestures = GestureTracker::<_>::new();

	let illustrator = Illustrator::new(
		settings.data_path.join("state.db"),
		RenderSettings {
			page_height: 800,
			page_width: 600,
			scale: 1.0,

			padding_top_em: 2.,
			padding_left_em: 2.,
			padding_right_em: 2.,
			padding_bottom_em: 2.,
			padding_paragraph_em: 0.5,

			body_text: RenderTextSettings {
				font_size: 18.0,
				line_height: 24.0,
				attrs: glyphon::Attrs::new(),
			},
			h1_text: RenderTextSettings {
				font_size: 30.0,
				line_height: 40.0,
				attrs: glyphon::Attrs::new(),
			},
			h2_text: RenderTextSettings {
				font_size: 30.0,
				line_height: 40.0,
				attrs: glyphon::Attrs::new(),
			},
		},
	);
	let areas = ActiveAreas::default();

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
		areas,
	};

	event_loop.run_app(&mut app)?;

	app.scribe.quit()?;

	Ok(())
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
	use android_logger::Config;
	use winit::platform::android::EventLoopBuilderExtAndroid;

	android_logger::init_once(
		Config::default()
			.with_tag("scribble-reader")
			.with_max_level(log::LevelFilter::Info),
	);

	let ext_data_path = app.external_data_path().unwrap();
	let s = scribe::Settings {
		cache_path: ext_data_path.parent().unwrap().join("cache"),
		config_path: ext_data_path.join("config"),
		data_path: ext_data_path.join("data"),
	};

	let event_loop = EventLoop::with_user_event()
		.with_android_app(app)
		.build()
		.unwrap();
	match start(event_loop, s) {
		Ok(_) => {}
		Err(e) => log::error!("Error: {e}"),
	}
}
