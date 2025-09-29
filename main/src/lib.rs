#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]

mod renderer;
mod scribe;
mod ui;

use std::time::Duration;
use std::time::Instant;

use egui::Vec2;
use log::error;
use log::info;
use log::trace;
use log::warn;
use winit::error::EventLoopError;
use winit::event::WindowEvent;
use winit::event_loop::EventLoopProxy;
#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

use winit::application::ApplicationHandler;
use winit::event_loop::EventLoop;
use winit::window::Window;

use crate::renderer::Renderer;
use crate::scribe::BookId;
use crate::scribe::Scribe;
use crate::ui::BookCard;
use crate::ui::ListView;
use crate::ui::MainView;

pub use crate::scribe::Settings;

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
	renderer: Option<Renderer<'window>>,
	scribe: Scribe,
	view: MainView,
	poke_stick: AppPokeStick,
	egui_ctx: egui::Context,
	fps: FpsCalculator,
	request_redraw: Instant,
}

impl App<'_> {
	const ACTIVE_TICK: u64 = 32;
	const SLEEP_TIMEOUT: u64 = 64;

	fn request_redraw(&mut self) {
		trace!("Request redraw");
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
					renderer.window.request_redraw();
				}
			}
			winit::event::StartCause::ResumeTimeReached {
				requested_resume, ..
			} => {
				trace!("Resume time reached");
				let since_redraw_request = requested_resume
					.duration_since(self.request_redraw)
					.as_millis() as u64;
				if since_redraw_request < Self::SLEEP_TIMEOUT {
					trace!("Render full speed: {}", since_redraw_request);
					if let Some(renderer) = self.renderer.as_mut() {
						trace!("Render");
						renderer.window.request_redraw();
					}
					let next_tick = Instant::now() + Duration::from_millis(Self::ACTIVE_TICK);
					event_loop
						.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_tick));
				} else {
					trace!("Render sleep");
					event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait)
				}
			}
			winit::event::StartCause::WaitCancelled {
				requested_resume, ..
			} => {
				if requested_resume.is_none()
					&& let Some(renderer) = self.renderer.as_mut()
				{
					trace!("Wait cancelled from sleep");
					renderer.window.request_redraw();
					let next_tick = Instant::now() + Duration::from_millis(Self::ACTIVE_TICK);
					event_loop
						.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_tick));
				}
			}
			_ => {}
		};
	}

	fn resumed(&mut self, event_loop: &egui_winit::winit::event_loop::ActiveEventLoop) {
		trace!("resumed");
		let window = event_loop
			.create_window(Window::default_attributes())
			.unwrap();
		window.set_title("Scribble-reader");
		let renderer = match pollster::block_on(Renderer::create(window, &self.egui_ctx)) {
			Ok(renderer) => renderer,
			Err(e) => {
				error!("Failed to resume renderer: {e}");
				panic!("Failed to resume renderer: {e}");
			}
		};
		self.renderer = Some(renderer);
	}

	fn suspended(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
		info!("suspended");
		self.renderer = None;
	}

	fn user_event(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop, event: AppPoke) {
		trace!("user event: {event:?}");
		match event {
			AppPoke::LibraryLoad | AppPoke::LibrarySorted => {
				log::info!("Event {event:?}");
				let mut books = self.scribe.library().books(0..ListView::SIZE);
				self.scribe.assistant().poke_list(&books);
				let cards = std::array::from_fn(|_| {
					books.pop().map(|b| BookCard {
						id: b.id,
						title: b.title,
						author: b.author,
					})
				});
				self.view.list = Some(ListView { page: 0, cards });
			}
			AppPoke::BookUpdated(id) => {
				if let Some(list) = self.view.list.as_mut() {
					let mut updated = false;
					for card in list.cards.iter_mut().flatten() {
						if card.id == id
							&& let Some(book) = self.scribe.library().book(id)
						{
							card.title = book.title;
							card.author = book.author;
							log::info!("Updated book {id:?}");
							updated = true;
							break;
						}
					}
					if !updated {
						log::info!("No updated needed for {id:?}");
					}
				}
			}
			AppPoke::NextPage => {
				if let Some(list) = self.view.list.as_mut() {
					let page = list.page + 1;
					let r = (page * ListView::SIZE)..(page * ListView::SIZE + ListView::SIZE);
					let mut books = self.scribe.library().books(r);
					self.scribe.assistant().poke_list(&books);
					let cards = std::array::from_fn(|_| {
						books.pop().map(|b| BookCard {
							id: b.id,
							title: b.title,
							author: b.author,
						})
					});
					list.page = page;
					list.cards = cards;
				}
			}
			AppPoke::PreviousPage => {
				if let Some(list) = self.view.list.as_mut() {
					let page = list.page.saturating_sub(1);
					let r = (page * ListView::SIZE)..(page * ListView::SIZE + ListView::SIZE);
					let mut books = self.scribe.library().books(r);
					self.scribe.assistant().poke_list(&books);
					let cards = std::array::from_fn(|_| {
						books.pop().map(|b| BookCard {
							id: b.id,
							title: b.title,
							author: b.author,
						})
					});
					list.page = page;
					list.cards = cards;
				}
			}
		}
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

				match event {
					WindowEvent::Resized(physical_size) => {
						renderer.resize(physical_size);
						self.request_redraw();
					}
					WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
						renderer.rescale(scale_factor);
						self.request_redraw();
					}
					WindowEvent::RedrawRequested => {
						self.fps.tick();

						match renderer.render(&mut self.view, &self.poke_stick) {
							Ok(_) => {}
							Err(e) => {
								error!("Failure during render: {e:?}");
								event_loop.exit();
							}
						}
					}
					_ => {
						let response = renderer.gui_renderer.handle_event(&renderer.window, &event);
						if response.repaint {
							self.request_redraw();
						}
					}
				};
			}
		}
	}
}

#[derive(Debug)]
pub enum AppPoke {
	LibraryLoad,
	LibrarySorted,
	BookUpdated(BookId),
	NextPage,
	PreviousPage,
}

struct AppPokeStick {
	event_loop: EventLoopProxy<AppPoke>,
	assistant: scribe::ScribeAssistant,
}

impl AppPokeStick {
	fn new(event_loop: EventLoopProxy<AppPoke>, assistant: scribe::ScribeAssistant) -> Self {
		Self {
			event_loop,
			assistant,
		}
	}
}

impl ui::MainPokeStick for AppPokeStick {
	fn scan_library(&self) {
		self.assistant.send(scribe::ScribeRequest::Scan);
	}

	fn next_page(&self) {
		self.event_loop.send_event(AppPoke::NextPage).unwrap();
	}

	fn previous_page(&self) {
		self.event_loop.send_event(AppPoke::PreviousPage).unwrap();
	}
}

impl scribe::ScribeBell for EventLoopProxy<AppPoke> {
	fn library_loaded(&self) {
		self.send_event(AppPoke::LibraryLoad).unwrap();
	}

	fn library_sorted(&self) {
		self.send_event(AppPoke::LibrarySorted).unwrap();
	}

	fn book_updated(&self, id: BookId) {
		self.send_event(AppPoke::BookUpdated(id)).unwrap();
	}

	fn fail(&self, error: String) {
		log::error!("Error in scribe: {error}");
	}
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	EventLoop(#[from] EventLoopError),
	#[error(transparent)]
	ScribeCreate(#[from] scribe::ScribeCreateError),
}

pub fn start(event_loop: EventLoop<AppPoke>, settings: Settings) -> Result<(), Error> {
	let scribe = Scribe::create(event_loop.create_proxy(), settings)?;
	let view = MainView::default();
	let poke_stick = AppPokeStick::new(event_loop.create_proxy(), scribe.assistant());

	let egui_ctx = egui::Context::default();
	egui_extras::install_image_loaders(&egui_ctx);
	egui_ctx.add_font(egui::epaint::text::FontInsert::new(
		"lucide-icons",
		egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES),
		vec![egui::epaint::text::InsertFontFamily {
			family: ui::ICON_FONT_FAMILY.clone(),
			priority: egui::epaint::text::FontPriority::Lowest,
		}],
	));
	egui_ctx.style_mut(|style| {
		style.animation_time = 0.0;
		style.spacing.item_spacing = Vec2::new(5.0, 5.0);
	});
	let fps = FpsCalculator::new();

	let mut app = App {
		renderer: None,
		scribe,
		view,
		poke_stick,
		egui_ctx,
		fps,
		request_redraw: Instant::now(),
	};

	event_loop.run_app(&mut app)?;

	Ok(())
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
	use android_logger::Config;
	use winit::platform::android::EventLoopBuilderExtAndroid;

	android_logger::init_once(Config::default().with_max_level(log::LevelFilter::Info));
	let event_loop = EventLoop::with_user_event()
		.with_android_app(app)
		.build()
		.unwrap();
	log::info!("Hello from android!");
	start(event_loop);
}
