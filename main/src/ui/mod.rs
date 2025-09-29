use std::sync::Arc;

use egui::Context;
use egui::FontFamily;
use egui::FontId;
use egui::Layout;
use egui::TextFormat;
use egui::text::LayoutJob;
use lazy_static::lazy_static;
use lucide_icons::Icon;

use crate::scribe::ScribeAssistant;

lazy_static! {
	pub static ref ICON_FONT_FAMILY: FontFamily = FontFamily::Name("lucide-icons".into());
}

pub trait GuiView {
	fn draw(&mut self, ctx: &Context);
}

pub struct BookCard {
	name: Arc<String>,
}

pub struct PageView {

}

pub struct MainView {
	scribe: ScribeAssistant,
}

impl MainView {
	pub fn new(scribe: ScribeAssistant) -> Self {
		Self { scribe }
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

impl GuiView for MainView {
	fn draw(&mut self, ctx: &Context) {
		egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
			egui::MenuBar::new().ui(ui, |ui| {
				ui.menu_button(icon_text(Icon::Hamburger, "Scribble reader", 18.0), |ui| {
					if ui
						.button(icon_text(Icon::RefreshCw, "Refresh", 18.0))
						.clicked()
					{
						self.scribe.request(crate::scribe::ScribeRequest::Scan);
					}
					if ui.button(icon_text(Icon::DoorOpen, "Quit", 18.0)).clicked() {
						ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
					}
				});
			});
		});

		let response = egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
			ui.vertical(|ui| {
				ui.add_space(5.0);
				ui.horizontal(|ui| {
					ui.columns(7, |columns| {
						columns[1].with_layout(
							Layout::centered_and_justified(egui::Direction::LeftToRight),
							|ui| {
								let width = ui.available_width();
								ui.set_height(width * 0.5);
								ui.add(egui::Button::new(icon(Icon::MoveLeft, 64.0)));
							},
						);
						columns[5].with_layout(
							Layout::centered_and_justified(egui::Direction::RightToLeft),
							|ui| {
								ui.set_height(ui.available_width() * 0.5);
								ui.add(egui::Button::new(icon(Icon::MoveRight, 64.0)));
							},
						);
					});
				});
				ui.add_space(3.0);
			});
		});
		let bb_height = response
			.response
			.intrinsic_size
			.unwrap_or(response.response.rect.size())
			.y;

		egui::CentralPanel::default().show(ctx, |ui| {
			let height = ui.available_height() - bb_height;
			let card_height = height / 5.0;

			ui.vertical(|ui| {
				for i in 0..5 {
					ui.group(|ui| {
						ui.set_width(ui.available_width());
						ui.set_height(card_height);
						ui.group(|ui| {
							ui.set_width(ui.available_height() * 0.75);
							ui.set_height(ui.available_height());
							ui.label(format!("Book {}", i))
						});
					});
				}
			});
		});
	}
}
