use egui::Color32;
use egui::Context;
use egui::FontFamily;
use egui::FontId;
use egui::Layout;
use lazy_static::lazy_static;
use lucide_icons::Icon;

lazy_static! {
	pub static ref ICON_FONT_FAMILY: FontFamily = FontFamily::Name("lucide-icons".into());
}

pub trait GuiView {
	fn draw(&mut self, ctx: &Context);
}

#[derive(Debug, Default)]
pub struct MainView {
	fps: u64,
}

impl MainView {
	pub fn set_fps(&mut self, fps: u64) {
		self.fps = fps;
	}
}

impl GuiView for MainView {
	fn draw(&mut self, ctx: &Context) {
		let response = egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
			ui.label(egui::RichText::new(format!("FPS: {0}", self.fps)).color(Color32::RED));
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
								ui.add(egui::Button::new(
									egui::RichText::new(Icon::MoveLeft.unicode())
										.font(FontId::new(64.0, ICON_FONT_FAMILY.clone())),
								));
							},
						);
						columns[5].with_layout(
							Layout::centered_and_justified(egui::Direction::RightToLeft),
							|ui| {
								ui.set_height(ui.available_width() * 0.5);
								ui.add(egui::Button::new(
									egui::RichText::new(Icon::MoveRight.unicode())
										.font(FontId::new(64.0, ICON_FONT_FAMILY.clone())),
								));
							},
						);
					});
				});
				ui.add_space(3.0);
			});
		});
		let bb_height = response.response.intrinsic_size.unwrap_or(response.response.rect.size()).y;

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
