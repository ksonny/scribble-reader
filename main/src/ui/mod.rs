use std::sync::Arc;

use egui::Context;
use egui::FontFamily;
use egui::FontId;
use egui::Layout;
use egui::TextFormat;
use egui::text::LayoutJob;
use lazy_static::lazy_static;
use lucide_icons::Icon;

use crate::scribe::BookId;

lazy_static! {
	pub static ref ICON_FONT_FAMILY: FontFamily = FontFamily::Name("lucide-icons".into());
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
				ui.group(|ui| {
					ui.set_width(height * 0.5);
					// ui.set_min_size([height, height].into());
					ui.label("Test");
				});
				ui.vertical(|ui| {
					let title = self.title.as_ref().map(|t| t.as_str()).unwrap_or("Unknown");
					ui.label(title);
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
pub struct MainView {
	pub list: Option<ListView>,
}

impl GuiView for MainView {
	fn draw(&mut self, ctx: &Context, poke_stick: &impl MainPokeStick) {
		egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
			egui::MenuBar::new().ui(ui, |ui| {
				ui.menu_button(icon_text(Icon::Hamburger, "Scribble reader", 18.0), |ui| {
					if ui
						.button(icon_text(Icon::RefreshCw, "Refresh", 18.0))
						.clicked()
					{
						poke_stick.scan_library();
					}
					if ui.button(icon_text(Icon::DoorOpen, "Quit", 18.0)).clicked() {
						ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
					}
				});
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
								if ui.button(icon(Icon::MoveLeft, 64.0)).clicked() {
									poke_stick.previous_page();
								}
							},
						);
						columns[5].with_layout(
							Layout::centered_and_justified(egui::Direction::RightToLeft),
							|ui| {
								ui.set_height(ui.available_width() * 0.5);
								if ui.button(icon(Icon::MoveRight, 64.0)).clicked() {
									poke_stick.next_page();
								}
							},
						);
					});
				});
				ui.add_space(3.0);
			});
		});

		if let Some(list) = &self.list {
			egui::CentralPanel::default().show(ctx, |ui| list.draw(ui));
		}
	}
}

fn icon(icon: Icon, size: f32) -> egui::RichText {
	egui::RichText::new(icon.unicode()).font(FontId::new(size, ICON_FONT_FAMILY.clone()))
}

fn icon_text(icon: Icon, text: &str, size: f32) -> egui::text::LayoutJob {
	let mut job = LayoutJob::default();
	job.append(
		&icon.unicode().to_string(),
		0.0,
		TextFormat {
			font_id: FontId::new(size, ICON_FONT_FAMILY.clone()),
			..Default::default()
		},
	);
	job.append(
		text,
		5.0,
		TextFormat {
			font_id: FontId::new(size, FontFamily::Proportional),
			..Default::default()
		},
	);
	job
}
