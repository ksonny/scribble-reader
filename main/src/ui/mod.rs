use egui::Color32;
use egui::Context;
use egui::FontFamily;
use lazy_static::lazy_static;

lazy_static! {
	pub static ref ICON_FONT_FAMILY: FontFamily = FontFamily::Name("lucide-icons".into());
}

pub trait GuiView {
	fn draw(&mut self, ctx: &Context);
}

#[derive(Debug, Default)]
pub struct MainView {
	is_window_open: bool,
	fps: u64,
}

impl MainView {
	pub fn set_fps(&mut self, fps: u64) {
		self.fps = fps;
	}

}

impl GuiView for MainView {
	fn draw(&mut self, ctx: &Context) {
		egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
			ui.label(egui::RichText::new(format!("FPS: {0}", self.fps)).color(Color32::RED));
		});

		egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
			egui::MenuBar::new().ui(ui, |ui| {
				ui.label(egui::RichText::new(format!("FPS: {0}", self.fps)).color(Color32::RED));
				ui.menu_button("File", |ui| {
					if ui.button("About...").clicked() {
						self.is_window_open = true;
						ui.close();
					}
				});
			});
		});

		egui::Window::new("Hello, winit-wgpu-egui")
			.open(&mut self.is_window_open)
			.show(ctx, |ui| {
				ui.label(
					"This is the most basic example of how to use winit, wgpu and egui together.",
				);
				ui.label("Mandatory heart: ♥");

				ui.separator();

				ui.horizontal(|ui| {
					ui.spacing_mut().item_spacing.x /= 2.0;
					ui.label("Learn more about wgpu at");
					ui.hyperlink("https://docs.rs/winit");
				});
				ui.horizontal(|ui| {
					ui.spacing_mut().item_spacing.x /= 2.0;
					ui.label("Learn more about winit at");
					ui.hyperlink("https://docs.rs/wgpu");
				});
				ui.horizontal(|ui| {
					ui.spacing_mut().item_spacing.x /= 2.0;
					ui.label("Learn more about egui at");
					ui.hyperlink("https://docs.rs/egui");
				});
			});
	}
}
