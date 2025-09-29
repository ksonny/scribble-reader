use std::sync::Arc;

use egui::Color32;
use egui::Context;
use egui::FontFamily;
use egui::FontId;
use egui::Layout;
use egui::RichText;
use egui::Style;
use egui::TextFormat;
use egui::TextStyle;
use egui::text::LayoutJob;
use lucide_icons::Icon;

use crate::scribe::BookId;

pub mod theme {
	use egui::FontFamily;
	use egui::FontId;
	use egui::TextStyle;
	use lazy_static::lazy_static;

	pub const DEFAULT_SIZE: f32 = 14.0;
	pub const S_SIZE: f32 = 12.0;
	pub const M_SIZE: f32 = 18.0;
	pub const L_SIZE: f32 = 24.0;
	pub const XL_SIZE: f32 = 48.0;

	lazy_static! {
		pub static ref ICON_FONT_FAMILY: FontFamily = FontFamily::Name("lucide-icons".into());
		pub static ref ICON_FONT: FontId = FontId::new(DEFAULT_SIZE, ICON_FONT_FAMILY.clone());
		pub static ref ICON_L_FONT: FontId = FontId::new(L_SIZE, ICON_FONT_FAMILY.clone());
		pub static ref ICON_XL_FONT: FontId = FontId::new(XL_SIZE, ICON_FONT_FAMILY.clone());
		pub static ref ICON_STYLE: TextStyle = TextStyle::Name("ICON_STYLE".into());
		pub static ref ICON_L_STYLE: TextStyle = TextStyle::Name("ICON_L_STYLE".into());
		pub static ref ICON_XL_STYLE: TextStyle = TextStyle::Name("ICON_XL_STYLE".into());
		pub static ref HEADING2: TextStyle = TextStyle::Name("HEADING2".into());
	}
}

pub trait GuiView {
	fn draw(&mut self, ctx: &Context, poke_stick: &impl MainPokeStick);
}

pub(crate) struct BookCard {
	pub(crate) id: BookId,
	pub(crate) title: Option<Arc<String>>,
	pub(crate) author: Option<Arc<String>>,
}

impl BookCard {
	fn draw(&self, ui: &mut egui::Ui) {
		ui.group(|ui| {
			ui.set_min_size(ui.available_size());
			let height = ui.available_height();
			let width = ui.available_width();
			ui.horizontal(|ui| {
				ui.set_height(height);
				ui.set_width(width);
				ui.label(UiIcon::new(Icon::Book).size(height * 0.75).color(Color32::GRAY).build());
				ui.vertical(|ui| {
					let title = self.title.as_ref().map(|t| t.as_str()).unwrap_or("Unknown");
					ui.label(RichText::new(title).text_style(TextStyle::Heading));
					ui.end_row();
					let author = self
						.author
						.as_ref()
						.map(|t| t.as_str())
						.unwrap_or("Unknown");
					ui.label(author);
				});
			});
		});
	}
}

pub(crate) struct ListView {
	pub(crate) page: u32,
	pub(crate) cards: [Option<BookCard>; 5],
}

impl ListView {
	pub const SIZE: u32 = 5;

	fn draw(&self, ui: &mut egui::Ui) {
		let height = ui.available_height() - Self::SIZE as f32 * ui.spacing().item_spacing.y;
		let card_height = height / 5.0;

		ui.vertical(|ui| {
			for card in self.cards.iter().flatten() {
				ui.allocate_ui([ui.available_width(), card_height].into(), |ui| {
					card.draw(ui)
				});
			}
		});
	}
}

pub trait MainPokeStick {
	fn scan_library(&self);

	fn next_page(&self);

	fn previous_page(&self);
}

#[derive(Default)]
pub enum FeatureView {
	#[default]
	Empty,
	List(ListView),
}

#[derive(Default)]
pub struct MainView {
	pub feature: FeatureView,
}

impl GuiView for MainView {
	fn draw(&mut self, ctx: &Context, poke_stick: &impl MainPokeStick) {
		egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
			egui::MenuBar::new()
				.style(|style: &mut Style| {
					style.visuals.weak_text_alpha = 0.0;
				})
				.ui(ui, |ui| {
					ui.menu_button(
						UiIcon::new(Icon::Menu)
							.large()
							.build(),
						|ui| {
							if ui
								.button(
									UiIcon::new(Icon::RefreshCw)
										.text("Rescan library")
										.large()
										.build(),
								)
								.clicked()
							{
								poke_stick.scan_library();
							}
							if ui
								.button(UiIcon::new(Icon::DoorOpen).text("Quit").large().build())
								.clicked()
							{
								ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
							}
						},
					);
					ui.label(RichText::new("Scribble reader").size(theme::L_SIZE));
				});
		});

		egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
			ui.vertical(|ui| {
				ui.add_space(5.0);
				ui.horizontal(|ui| {
					ui.columns(7, |columns| {
						columns[1].with_layout(
							Layout::centered_and_justified(egui::Direction::LeftToRight),
							|ui| {
								let width = ui.available_width();
								ui.set_height(width * 0.5);
								if ui
									.button(UiIcon::new(Icon::MoveLeft).xlarge().build())
									.clicked()
								{
									poke_stick.previous_page();
								}
							},
						);
						columns[5].with_layout(
							Layout::centered_and_justified(egui::Direction::RightToLeft),
							|ui| {
								ui.set_height(ui.available_width() * 0.5);
								if ui
									.button(UiIcon::new(Icon::MoveRight).xlarge().build())
									.clicked()
								{
									poke_stick.next_page();
								}
							},
						);
					});
				});
				ui.add_space(3.0);
			});
		});

		match &self.feature {
			FeatureView::Empty => {}
			FeatureView::List(list) => {
				egui::CentralPanel::default().show(ctx, |ui| list.draw(ui));
			}
		}
	}
}

struct UiIcon<'a> {
	color: Color32,
	icon_font: FontId,
	icon: Icon,
	text_font: FontId,
	text: Option<&'a str>,
}

impl UiIcon<'_> {
	fn new(icon: Icon) -> Self {
		UiIcon {
			color: Color32::BLACK,
			icon_font: theme::ICON_FONT.clone(),
			icon,
			text_font: FontId::new(theme::DEFAULT_SIZE, FontFamily::Proportional),
			text: None,
		}
	}

	fn color(self, color: Color32) -> Self {
		Self { color, ..self }
	}

	fn text<'a>(self, text: &'a str) -> UiIcon<'a> {
		UiIcon {
			text: Some(text),
			..self
		}
	}

	fn size(self, size: f32) -> Self {
		Self {
			icon_font: FontId::new(size, theme::ICON_FONT_FAMILY.clone()),
			text_font: FontId::new(size, FontFamily::Proportional),
			..self
		}
	}

	fn large(self) -> Self {
		Self {
			icon_font: theme::ICON_L_FONT.clone(),
			text_font: FontId::new(theme::L_SIZE, FontFamily::Proportional),
			..self
		}
	}

	fn xlarge(self) -> Self {
		Self {
			icon_font: theme::ICON_XL_FONT.clone(),
			text_font: FontId::new(theme::XL_SIZE, FontFamily::Proportional),
			..self
		}
	}

	fn build(self) -> egui::text::LayoutJob {
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
