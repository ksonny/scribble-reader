#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]

mod gestures;
mod renderer;
mod ui;

use std::time::Duration;
use std::time::Instant;

use egui::Color32;
use egui::FontFamily;
use egui::FontId;
use egui::Stroke;
use egui::TextStyle;
use egui::Vec2;
use egui::ViewportId;
use illustrator::Illustrator;
use log::error;
use log::info;
use log::trace;
use log::warn;
use scribe::ScribeBell;
use winit::error::EventLoopError;
use winit::event::WindowEvent;
use winit::event_loop::EventLoopProxy;
#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

use winit::application::ApplicationHandler;
use winit::event_loop::EventLoop;
use winit::window::Window;

use crate::gestures::Direction;
use crate::gestures::Gesture;
use crate::gestures::GestureTracker;
use crate::renderer::Renderer;
use crate::ui::BookCard;
use crate::ui::FeatureView;
use crate::ui::GuiView;
use crate::ui::ListView;
use crate::ui::MainView;
use crate::ui::theme;
use scribe::Scribe;
use scribe::ScribeAssistant;
use scribe::library;
use scribe::library::BookId;

use crate::ui::PokeStick;

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

struct AppInput {
	start_time: Instant,
	pixels_per_point: f32,
	egui_input: egui::RawInput,
}

impl AppInput {
	fn resume(&mut self, size: winit::dpi::PhysicalSize<u32>, scale_factor: f32) {
		self.pixels_per_point = scale_factor;
		self.egui_input.screen_rect = Some(egui::Rect::from_min_size(
			Default::default(),
			egui::vec2(size.width as f32, size.height as f32) / self.pixels_per_point,
		));
		self.egui_input.viewport_id = ViewportId::ROOT;
		self.egui_input
			.viewports
			.entry(ViewportId::ROOT)
			.or_default()
			.native_pixels_per_point = Some(scale_factor);
	}

	fn handle_gesture(&mut self, event: &gestures::GestureEvent) {
		let pos = egui::pos2(event.loc.x as f32, event.loc.y as f32) / self.pixels_per_point;
		if event.gesture == Gesture::Tap {
			self.egui_input.events.push(egui::Event::PointerButton {
				pos,
				button: egui::PointerButton::Primary,
				pressed: true,
				modifiers: egui::Modifiers::default(),
			});
			self.egui_input.events.push(egui::Event::PointerButton {
				pos,
				button: egui::PointerButton::Primary,
				pressed: false,
				modifiers: egui::Modifiers::default(),
			});
		}
	}

	fn handle_move(&mut self, pos: winit::dpi::PhysicalPosition<f64>) {
		let vec = egui::vec2(pos.x as f32, pos.y as f32) / self.pixels_per_point;
		self.egui_input.events.push(egui::Event::MouseMoved(vec));
	}

	fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
		self.egui_input.screen_rect = Some(egui::Rect::from_min_size(
			Default::default(),
			egui::vec2(size.width as f32, size.height as f32) / self.pixels_per_point,
		));
	}

	fn rescale(&mut self, scale_factor: f32) {
		self.pixels_per_point = scale_factor;
		self.egui_input
			.viewports
			.entry(ViewportId::ROOT)
			.or_default()
			.native_pixels_per_point = Some(scale_factor);
	}

	fn tick(&mut self) -> egui::RawInput {
		self.egui_input.time = Some(Instant::now().duration_since(self.start_time).as_secs_f64());
		self.egui_input.take()
	}
}

struct App<'window> {
	input: AppInput,
	renderer: Option<Renderer<'window>>,
	scribe: Scribe,
	view: MainView,
	poke_stick: AppPokeStick,
	egui_ctx: egui::Context,
	fps: FpsCalculator,
	request_redraw: Instant,
	gestures: GestureTracker<10>,
	illustrator: Option<Illustrator>,
	settings: scribe::Settings,
}

impl App<'_> {
	const ACTIVE_TICK: u64 = 32;
	const SLEEP_TIMEOUT: u64 = 256;

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
					renderer.request_redraw();
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
						renderer.request_redraw();
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
		info!("resumed");
		let window = event_loop
			.create_window(Window::default_attributes())
			.unwrap();
		window.set_title("Scribble-reader");

		let size = window.inner_size();
		let scale_factor = window.scale_factor() as f32;
		self.input.resume(size, scale_factor);
		self.gestures
			.set_min_distance_by_screen(size.width, size.height);

		if let Some(renderer) = self.renderer.as_mut() {
			match renderer.resume(window) {
				Ok(_) => {}
				Err(e) => {
					error!("Failed to resume renderer: {e}");
					panic!("Failed to resume renderer: {e}");
				}
			};
		} else {
			match pollster::block_on(Renderer::create(window, &self.egui_ctx)) {
				Ok(renderer) => self.renderer = Some(renderer),
				Err(e) => {
					error!("Failed to create renderer: {e}");
					panic!("Failed to create renderer: {e}");
				}
			}
		};
	}

	fn suspended(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
		info!("suspended");
		if let Some(renderer) = self.renderer.as_mut() {
			renderer.suspend()
		}
	}

	fn user_event(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop, event: AppPoke) {
		trace!("user event: {event:?}");
		match event {
			AppPoke::ScanLibrary => {
				self.view.working = self.scribe.poke_scan();
			}
			AppPoke::LibraryLoad | AppPoke::LibrarySorted => match &mut self.view.feature {
				ui::FeatureView::Empty => {}
				ui::FeatureView::List(list) => {
					let books = self.scribe.library().books(0..ListView::SIZE);
					self.view.working = self.scribe.poke_list(&books);
					let mut books_iter = books.into_iter().map(|b| {
						let id = b.id;
						(b, self.scribe.library().thumbnail(id))
					});
					list.cards = std::array::from_fn(|_| books_iter.next().map(create_card));
					list.page = 0;
					self.request_redraw();
				}
			},
			AppPoke::OpenLibrary => {
				let books = self.scribe.library().books(0..ListView::SIZE);
				self.view.working = self.scribe.poke_list(&books);
				let mut books_iter = books.into_iter().map(|b| {
					let id = b.id;
					(b, self.scribe.library().thumbnail(id))
				});
				self.view.invisible = false;
				self.view.feature = FeatureView::List(Box::new(ListView {
					cards: std::array::from_fn(|_| books_iter.next().map(create_card)),
					page: 0,
				}));
				self.request_redraw();
			}
			AppPoke::BookUpdated(id) => match &mut self.view.feature {
				ui::FeatureView::Empty => {}
				ui::FeatureView::List(list) => {
					let card = list.cards.iter_mut().flatten().find(|c| c.id == id);
					if let Some(card) = card
						&& let Some(book) = self.scribe.library().book(id)
					{
						*card = create_card((book, self.scribe.library().thumbnail(id)));
						log::trace!("Updated book {id:?}");
						self.request_redraw();
					}
				}
			},
			AppPoke::NextPage => match &mut self.view.feature {
				ui::FeatureView::Empty => {
					// TODO: Send to reader
				}
				ui::FeatureView::List(list) => {
					let page = list.page + 1;
					let r = (page * ListView::SIZE)..(page * ListView::SIZE + ListView::SIZE);
					let books = self.scribe.library().books(r);
					if !books.is_empty() {
						self.view.working = self.scribe.poke_list(&books);
						let mut books_iter = books.into_iter().map(|b| {
							let id = b.id;
							(b, self.scribe.library().thumbnail(id))
						});
						list.cards = std::array::from_fn(|_| books_iter.next().map(create_card));
						list.page = page;
						self.request_redraw();
					}
				}
			},
			AppPoke::PreviousPage => match &mut self.view.feature {
				ui::FeatureView::Empty => {
					// TODO: Send to reader
				}
				ui::FeatureView::List(list) => {
					let page = list.page.saturating_sub(1);
					let r = (page * ListView::SIZE)..(page * ListView::SIZE + ListView::SIZE);
					let books = self.scribe.library().books(r);
					self.view.working = self.scribe.poke_list(&books);
					let mut books_iter = books.into_iter().map(|b| {
						let id = b.id;
						(b, self.scribe.library().thumbnail(id))
					});
					list.cards = std::array::from_fn(|_| books_iter.next().map(create_card));
					list.page = page;
					self.request_redraw();
				}
			},
			AppPoke::OpenBook(id) => {
				self.view.invisible = true;
				self.view.feature = FeatureView::Empty;

				let state_db_path = self.settings.data_path.join("state.db");
				let records = scribe::record_keeper::create(&state_db_path).unwrap();
				self.illustrator = Some(illustrator::spawn_illustrator(records, id));

				self.request_redraw();
			}
			AppPoke::Completed(ticket) => {
				self.view.working = self.scribe.complete_ticket(ticket);
			}
		}
	}

	fn window_event(
		&mut self,
		event_loop: &egui_winit::winit::event_loop::ActiveEventLoop,
		_window_id: egui_winit::winit::window::WindowId,
		event: egui_winit::winit::event::WindowEvent,
	) {
		match event {
			WindowEvent::CloseRequested => {
				info!("close requested");
				self.renderer.take();
				event_loop.exit();
				return;
			}
			WindowEvent::Destroyed => {
				info!("destroyed");
				self.renderer.take();
				return;
			}
			_ => {}
		};

		let gesture_ret = self.gestures.handle_window_event(&event);
		if gesture_ret.frame_ended {
			for event in self.gestures.events() {
				match event.gesture {
					Gesture::Swipe(Direction::Right, _) => {
						self.poke_stick.previous_page();
					}
					Gesture::Swipe(Direction::Left, _) => {
						self.poke_stick.next_page();
					}
					_ => {
						self.input.handle_gesture(&event);
					}
				}
			}
			self.gestures.reset();
			self.request_redraw();
		}

		log::trace!("event: {event:?}");
		match event {
			WindowEvent::CursorMoved { position, .. } if !gesture_ret.consumed => {
				self.input.handle_move(position);
				self.request_redraw();
			}
			WindowEvent::Resized(size) => {
				self.gestures
					.set_min_distance_by_screen(size.width, size.height);
				self.input.resize(size);
				if let Some(renderer) = self.renderer.as_mut() {
					renderer.resize(size)
				}
				self.request_redraw();
			}
			WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
				self.input.rescale(scale_factor as f32);
				if let Some(renderer) = self.renderer.as_mut() {
					renderer.rescale(scale_factor)
				}
				self.request_redraw();
			}
			WindowEvent::RedrawRequested => {
				let Some(renderer) = self.renderer.as_mut() else {
					warn!("Renderer not initialized, abort event {event:?}");
					return;
				};
				self.fps.tick();
				let input = self.input.tick();
				let output = self
					.egui_ctx
					.run(input, |ctx| self.view.draw(ctx, &self.poke_stick));

				renderer.prepare_ui(output);

				match renderer.render() {
					Ok(_) => {}
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

fn create_card(entry: (library::Book, Option<library::Thumbnail>)) -> BookCard {
	let (b, tn) = entry;
	BookCard {
		id: b.id,
		title: b.title,
		author: b.author,
		modified_at: b.modified_at,
		words_total: b.words_total,
		words_position: b.words_position,
		thumbnail: tn.and_then(|tn| match tn {
			library::Thumbnail::Bytes { bytes } => Some(ui::Thumbnail { bytes }),
			library::Thumbnail::None => None,
		}),
	}
}

#[derive(Debug)]
pub enum AppPoke {
	LibraryLoad,
	LibrarySorted,
	BookUpdated(BookId),
	NextPage,
	PreviousPage,
	OpenBook(BookId),
	OpenLibrary,
	ScanLibrary,
	Completed(scribe::ScribeTicket),
}

struct AppPokeStick {
	event_loop: EventLoopProxy<AppPoke>,
}

impl AppPokeStick {
	fn new(event_loop: EventLoopProxy<AppPoke>) -> Self {
		Self { event_loop }
	}
}

impl ui::PokeStick for AppPokeStick {
	fn scan_library(&self) {
		self.event_loop.send_event(AppPoke::ScanLibrary).unwrap();
	}

	fn next_page(&self) {
		self.event_loop.send_event(AppPoke::NextPage).unwrap();
	}

	fn previous_page(&self) {
		self.event_loop.send_event(AppPoke::PreviousPage).unwrap();
	}

	fn open_book(&self, id: BookId) {
		self.event_loop.send_event(AppPoke::OpenBook(id)).unwrap();
	}

	fn open_library(&self) {
		self.event_loop.send_event(AppPoke::OpenLibrary).unwrap();
	}
}

struct EventLoopBell(EventLoopProxy<AppPoke>);

impl ScribeBell for EventLoopBell {
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
		proxy.send_event(AppPoke::BookUpdated(id)).unwrap();
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
	let scribe = Scribe::create(bell, &settings)?;
	let view = MainView::default();
	let poke_stick = AppPokeStick::new(event_loop.create_proxy());

	let egui_ctx = egui::Context::default();
	egui_extras::install_image_loaders(&egui_ctx);

	egui_ctx.add_font(egui::epaint::text::FontInsert::new(
		"lucide-icons",
		egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES),
		vec![egui::epaint::text::InsertFontFamily {
			family: theme::ICON_FONT_FAMILY.clone(),
			priority: egui::epaint::text::FontPriority::Lowest,
		}],
	));
	egui_ctx.set_theme(egui::Theme::Light);
	egui_ctx.style_mut(|style| {
		style.animation_time = 0.0;
		style.spacing.item_spacing = Vec2::new(5.0, 5.0);
		style.wrap_mode = Some(egui::TextWrapMode::Truncate);
		style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.open.weak_bg_fill = Color32::TRANSPARENT;
		style.visuals.widgets.open.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.open.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
		style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.active.expansion = 0.0;
		style.visuals.widgets.active.weak_bg_fill = Color32::LIGHT_GRAY;
		style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.hovered.expansion = 2.0;
		style.visuals.widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
		style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::BLACK);
		style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::BLACK);

		style.text_styles = [
			(
				TextStyle::Heading,
				FontId::new(25.0, FontFamily::Proportional),
			),
			(
				theme::HEADING2.clone(),
				FontId::new(theme::M_SIZE, FontFamily::Proportional),
			),
			(
				TextStyle::Body,
				FontId::new(theme::DEFAULT_SIZE, FontFamily::Proportional),
			),
			(
				TextStyle::Monospace,
				FontId::new(theme::DEFAULT_SIZE, FontFamily::Monospace),
			),
			(
				TextStyle::Button,
				FontId::new(theme::M_SIZE, FontFamily::Proportional),
			),
			(
				TextStyle::Small,
				FontId::new(theme::S_SIZE, FontFamily::Proportional),
			),
			(
				theme::ICON_STYLE.clone(),
				FontId::new(theme::DEFAULT_SIZE, theme::ICON_FONT_FAMILY.clone()),
			),
			(
				theme::ICON_L_STYLE.clone(),
				FontId::new(theme::L_SIZE, theme::ICON_FONT_FAMILY.clone()),
			),
			(
				theme::ICON_XL_STYLE.clone(),
				FontId::new(theme::XL_SIZE, theme::ICON_FONT_FAMILY.clone()),
			),
		]
		.into();
	});
	let input = AppInput {
		start_time: Instant::now(),
		egui_input: egui::RawInput::default(),
		pixels_per_point: 1.0,
	};
	let fps = FpsCalculator::new();
	let gestures = GestureTracker::<_>::new();

	let mut app = App {
		settings,
		input,
		renderer: None,
		scribe,
		view,
		poke_stick,
		egui_ctx,
		fps,
		request_redraw: Instant::now(),
		gestures,
		illustrator: None,
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
