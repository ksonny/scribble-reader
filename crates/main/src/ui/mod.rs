pub(crate) mod theme;

use std::time::Instant;

use chrono::Local;
use egui::Align;
use egui::Color32;
use egui::Context;
use egui::CornerRadius;
use egui::FontFamily;
use egui::FontId;
use egui::Layout;
use egui::RichText;
use egui::Stroke;
use egui::TextFormat;
use egui::TextStyle;
use egui::TextWrapMode;
use egui::Vec2;
use egui::ViewportId;
use egui::epaint::text::FontInsert;
use egui::style::WidgetVisuals;
use egui::text::LayoutJob;
use lucide_icons::Icon;

use crate::gestures;

pub fn create_egui_ctx() -> Context {
	let egui_ctx = Context::default();
	egui_extras::install_image_loaders(&egui_ctx);

	egui_ctx.add_font(FontInsert::new(
		"lucide-icons",
		egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES),
		vec![egui::epaint::text::InsertFontFamily {
			family: theme::ICON_FONT_FAMILY.clone(),
			priority: egui::epaint::text::FontPriority::Lowest,
		}],
	));
	egui_ctx.set_theme(egui::Theme::Light);
	egui_ctx.global_style_mut(|style| {
		style.animation_time = 0.0;
		style.spacing.interact_size = Vec2::ZERO;
		style.spacing.item_spacing = Vec2::new(5.0, 5.0);
		style.spacing.button_padding = Vec2::new(2.0, 2.0);
		style.wrap_mode = Some(TextWrapMode::Truncate);
		style.visuals.widgets.noninteractive = WidgetVisuals {
			bg_fill: Color32::BLACK,
			weak_bg_fill: Color32::TRANSPARENT,
			bg_stroke: Stroke::new(1.0, Color32::BLACK),
			corner_radius: CornerRadius::from(3.0),
			fg_stroke: Stroke::new(1.0, Color32::BLACK),
			expansion: 0.0,
		};
		style.visuals.widgets.open = WidgetVisuals {
			bg_fill: Color32::BLACK,
			weak_bg_fill: Color32::TRANSPARENT,
			bg_stroke: Stroke::new(1.0, Color32::BLACK),
			corner_radius: CornerRadius::from(3.0),
			fg_stroke: Stroke::new(1.0, Color32::BLACK),
			expansion: 0.0,
		};
		style.visuals.widgets.inactive = WidgetVisuals {
			bg_fill: Color32::BLACK,
			weak_bg_fill: Color32::TRANSPARENT,
			bg_stroke: Stroke::new(1.0, Color32::BLACK),
			corner_radius: CornerRadius::from(3.0),
			fg_stroke: Stroke::new(1.0, Color32::BLACK),
			expansion: 0.0,
		};
		style.visuals.widgets.active = WidgetVisuals {
			bg_fill: Color32::LIGHT_GRAY,
			weak_bg_fill: Color32::TRANSPARENT,
			bg_stroke: Stroke::new(1.0, Color32::BLACK),
			corner_radius: CornerRadius::from(3.0),
			fg_stroke: Stroke::new(1.0, Color32::BLACK),
			expansion: 0.0,
		};
		style.visuals.widgets.hovered = WidgetVisuals {
			bg_fill: Color32::BLACK,
			weak_bg_fill: Color32::TRANSPARENT,
			bg_stroke: Stroke::new(1.0, Color32::BLACK),
			corner_radius: CornerRadius::from(3.0),
			fg_stroke: Stroke::new(1.0, Color32::BLACK),
			expansion: 0.0,
		};

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
	egui_ctx
}

pub struct UiInput {
	pub(crate) start_time: Instant,
	pub(crate) pixels_per_point: f32,
	egui_ctx: Context,
	egui_input: egui::RawInput,
}

impl UiInput {
	pub fn new(egui_ctx: egui::Context) -> Self {
		Self {
			start_time: Instant::now(),
			pixels_per_point: 1.0,
			egui_ctx,
			egui_input: egui::RawInput::default(),
		}
	}

	pub fn resume(&mut self, size: winit::dpi::PhysicalSize<u32>, scale_factor: f32) {
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

	pub fn handle_gesture(&mut self, event: &gestures::GestureEvent) {
		use gestures::Gesture;
		let pos = self.translate_pos(event.loc);
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

	pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
		self.egui_input.screen_rect = Some(egui::Rect::from_min_size(
			Default::default(),
			egui::vec2(size.width as f32, size.height as f32) / self.pixels_per_point,
		));
	}

	pub fn rescale(&mut self, scale_factor: f32) {
		self.pixels_per_point = scale_factor;
		self.egui_input
			.viewports
			.entry(ViewportId::ROOT)
			.or_default()
			.native_pixels_per_point = Some(scale_factor);
	}

	pub fn translate_pos(&self, loc: gestures::Location) -> egui::Pos2 {
		egui::pos2(loc.x as f32, loc.y as f32) / self.pixels_per_point
	}

	pub fn tick(&mut self) -> egui::RawInput {
		self.egui_input.time = Some(Instant::now().duration_since(self.start_time).as_secs_f64());
		self.egui_input.take()
	}

	pub fn run(&mut self, run_ui: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
		let input = self.tick();
		self.egui_ctx.run_ui(input, run_ui)
	}
}

pub(crate) struct UiIcon<'a> {
	color: Color32,
	icon_font: FontId,
	icon: Icon,
	text_font: FontId,
	text: Option<&'a str>,
}

impl UiIcon<'_> {
	pub(crate) fn new(icon: Icon) -> Self {
		UiIcon {
			color: Color32::BLACK,
			icon_font: theme::ICON_FONT.clone(),
			icon,
			text_font: FontId::new(theme::DEFAULT_SIZE, FontFamily::Proportional),
			text: None,
		}
	}

	pub(crate) fn color(self, color: Color32) -> Self {
		Self { color, ..self }
	}

	pub(crate) fn text<'a>(self, text: &'a str) -> UiIcon<'a> {
		UiIcon {
			text: Some(text),
			..self
		}
	}

	pub(crate) fn size(self, size: f32) -> Self {
		Self {
			icon_font: FontId::new(size, theme::ICON_FONT_FAMILY.clone()),
			text_font: FontId::new(size, FontFamily::Proportional),
			..self
		}
	}

	pub(crate) fn large(self) -> Self {
		Self {
			icon_font: theme::ICON_L_FONT.clone(),
			text_font: FontId::new(theme::L_SIZE, FontFamily::Proportional),
			..self
		}
	}

	pub(crate) fn xlarge(self) -> Self {
		Self {
			icon_font: theme::ICON_XL_FONT.clone(),
			text_font: FontId::new(theme::XL_SIZE, FontFamily::Proportional),
			..self
		}
	}

	pub(crate) fn build(self) -> egui::text::LayoutJob {
		let mut job = LayoutJob::default();
		let mut char_buf = [0; 4];
		job.append(
			self.icon.unicode().encode_utf8(&mut char_buf),
			0.0,
			TextFormat {
				font_id: self.icon_font,
				color: self.color,
				..Default::default()
			},
		);
		if let Some(text) = self.text {
			job.append(
				text,
				5.0,
				TextFormat {
					font_id: self.text_font,
					color: self.color,
					..Default::default()
				},
			);
		}
		job
	}
}

pub(crate) trait OnAction<A> {
	fn on_action(&mut self, action: A);
}

pub(crate) struct MenuItem<'a, A> {
	pub(crate) icon: Icon,
	pub(crate) description: &'a str,
	pub(crate) active: bool,
	pub(crate) action: A,
}

pub(crate) struct MainMenuBar<'a, A, H: OnAction<A>> {
	handler: &'a mut H,
	items: &'a [MenuItem<'a, A>],
	statusline: Option<&'a str>,
	loading: bool,
}

impl<'a, A, H: OnAction<A>> MainMenuBar<'a, A, H> {
	pub(crate) fn new(handler: &'a mut H, items: &'a [MenuItem<A>]) -> Self {
		Self {
			handler,
			items,
			statusline: None,
			loading: false,
		}
	}

	pub(crate) fn with_loading(self, loading: bool) -> Self {
		Self { loading, ..self }
	}

	pub(crate) fn with_status(self, statusline: Option<&'a str>) -> Self {
		Self { statusline, ..self }
	}
}

impl<A: Copy, H: OnAction<A>> MainMenuBar<'_, A, H> {
	pub(crate) fn ui(self, ui: &mut egui::Ui) -> egui::Response {
		egui::MenuBar::new()
			.ui(ui, |ui| {
				ui.vertical(|ui| {
					ui.add_space(6.);
					ui.horizontal(|ui| {
						ui.spacing_mut().item_spacing.x = 10.;

						let menu = ui.menu_button(UiIcon::new(Icon::Menu).large().build(), |ui| {
							for item in self.items {
								let button = ui.selectable_label(
									item.active,
									UiIcon::new(item.icon)
										.large()
										.text(item.description)
										.build(),
								);
								if button.clicked() {
									self.handler.on_action(item.action);
								}
							}
						});

						ui.label(RichText::new("Scribble reader").size(theme::L_SIZE));

						if let Some(statusline) = self.statusline {
							ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
								ui.add_space(2.);
								ui.label(RichText::new(statusline).size(theme::S_SIZE));
							});
						}

						ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
							let now = Local::now();
							ui.label(
								RichText::new(now.format("%H:%M").to_string())
									.monospace()
									.size(theme::CLOCK_SIZE),
							);

							ui.add_space(2.);

							if self.loading {
								ui.label(
									UiIcon::new(Icon::RefreshCw)
										.color(Color32::GRAY)
										.large()
										.build(),
								);
							}
						});
						menu.response
					})
					.inner
				})
				.inner
			})
			.inner
	}
}

pub(crate) struct ToolItem<'a, A> {
	pub(crate) icon: Icon,
	#[allow(unused)]
	pub(crate) description: &'a str,
	pub(crate) active: bool,
	pub(crate) action: A,
}

pub(crate) struct ToolBar<'a, A, H: OnAction<A>> {
	handler: &'a mut H,
	items: &'a [Option<ToolItem<'a, A>>],
	disabled: bool,
}

impl<'a, A, H: OnAction<A>> ToolBar<'a, A, H> {
	pub(crate) fn new(
		handler: &'a mut H,
		items: &'a [Option<ToolItem<A>>],
		disabled: bool,
	) -> Self {
		Self {
			handler,
			items,
			disabled,
		}
	}
}

impl<A: Copy, H: OnAction<A>> ToolBar<'_, A, H> {
	pub(crate) fn ui(self, ui: &mut egui::Ui) -> egui::Response {
		if self.disabled {
			ui.disable();
		}
		ui.vertical(|ui| {
			ui.add_space(5.0);
			ui.horizontal(|ui| {
				ui.columns(self.items.len(), |columns| {
					for (item, ui) in self.items.iter().zip(columns.iter_mut()) {
						if let Some(item) = item {
							ui.with_layout(
								Layout::centered_and_justified(egui::Direction::LeftToRight),
								|ui| {
									ui.set_height(ui.available_width() * 0.5);
									let response = ui.add(
										egui::Button::new(UiIcon::new(item.icon).xlarge().build())
											.selected(item.active)
											.frame(true),
									);
									if response.clicked() {
										self.handler.on_action(item.action);
									}
								},
							);
						}
					}
				});
			});
			ui.add_space(3.0);
		})
		.response
	}
}
