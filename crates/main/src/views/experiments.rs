use egui::Panel;
use image::ImageBuffer;
use lucide_icons::Icon;
use sculpter::AtlasImage;
use sculpter::Axis;
use sculpter::DisplayGlyph;
use sculpter::Fixed;
use sculpter::FontOptions;
use sculpter::FontStyle;
use sculpter::SculpterFonts;
use sculpter::SculpterInput;
use sculpter::SculpterOptions;
use sculpter::create_sculpter;

use crate::AppBell;
use crate::AppEvent;
use crate::gestures::GestureEvent;
use crate::renderer::Painter;
use crate::renderer::pixmap_renderer::PixmapData;
use crate::renderer::pixmap_renderer::PixmapInstance;
use crate::renderer::pixmap_renderer::PixmapRef;
use crate::ui::MainMenuBar;
use crate::ui::MenuItem;
use crate::ui::OnAction;
use crate::ui::ToolBar;
use crate::ui::ToolItem;
use crate::views::EventResult;
use crate::views::GestureResult;
use crate::views::ViewHandle;
use crate::views::Viewport;

pub(crate) struct ExperimentsView {
	bell: AppBell,

	viewport: Viewport,
	fonts: SculpterFonts,
	show_atlas: bool,

	block_pixmap: Option<PixmapRef>,
	atlas_pixmap: Option<PixmapRef>,
	render_items: Option<(AtlasImage, Vec<DisplayGlyph>)>,
}

impl ExperimentsView {
	pub(crate) fn create(bell: AppBell, fonts: SculpterFonts, viewport: Viewport) -> Self {
		Self {
			bell,

			viewport,
			fonts,
			show_atlas: false,

			block_pixmap: None,
			atlas_pixmap: None,
			render_items: None,
		}
	}
}

#[derive(Clone, Copy)]
enum MenuAction {
	Exit,
	OpenLibrary,
}

#[derive(Clone, Copy)]
enum ToolAction {
	TestA,
	TestB,
	TestC,
}

impl OnAction<MenuAction> for ExperimentsView {
	fn on_action(&mut self, action: MenuAction) {
		match action {
			MenuAction::Exit => {
				self.bell.send_event(AppEvent::Exit);
			}
			MenuAction::OpenLibrary => {
				self.bell.send_event(AppEvent::OpenLibrary);
			}
		}
	}
}

impl OnAction<ToolAction> for ExperimentsView {
	fn on_action(&mut self, action: ToolAction) {
		match action {
			ToolAction::TestA => {}
			ToolAction::TestB => {
				if self.render_items.is_none() {
					let scale_factor = Fixed::from_num(self.viewport.scale_factor);

					let font_regular = FontOptions {
						family: sculpter::Family::SansSerif,
						variations: vec![sculpter::Variation {
							axis: Axis::Wght,
							value: Fixed::lit("400.0"),
						}],
					};
					let font_bold = FontOptions {
						family: sculpter::Family::SansSerif,
						variations: vec![sculpter::Variation {
							axis: Axis::Wght,
							value: Fixed::lit("700.0"),
						}],
					};
					let mut sculpter = create_sculpter(
						&self.fonts,
						&[&font_regular, &font_bold],
						SculpterOptions::default(),
					)
					.inspect_err(|err| log::error!("Error: {err}"))
					.unwrap();

					let inputs = [SculpterInput {
						style: FontStyle {
							font_opts: &font_regular,
							font_size: Fixed::lit("18.0") * scale_factor,
							line_height_em: Fixed::lit("1.25"),
						},
						// input: "Lorem ipsum \ndolor sit amet,\n consectetur adipiscing elit,\n sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.\n😀 😻 💩",
						input: "traffic of thousands of commuters",
					}];
					let mut handle = sculpter
						.shape(inputs.into_iter())
						.inspect_err(|err| log::error!("Error: {err}"))
						.unwrap();

					let block = sculpter
						.render_block(
							&mut handle,
							self.viewport.screen_width - 200,
							self.viewport.screen_height,
							Fixed::lit("24."),
						)
						.inspect_err(|err| log::error!("Error: {err}"))
						.unwrap();

					let mut atlas = AtlasImage::default();
					sculpter
						.write_glyph_atlas(&mut atlas)
						.inspect_err(|err| log::error!("Error: {err}"))
						.unwrap();

					self.render_items = Some((atlas, block.glyphs));
				} else {
					self.render_items.take();
				}
			}
			ToolAction::TestC => {
				self.show_atlas = !self.show_atlas;
			}
		}
	}
}

impl ViewHandle for ExperimentsView {
	fn draw(&mut self, painter: Painter<'_>) {
		painter
			.draw_pixmap(|brush| {
				let pixmap = if let Some(pixmap) = self.block_pixmap.take() {
					pixmap
				} else {
					let image = ImageBuffer::from_pixel(32, 32, image::Rgba([128u8, 0, 0, 128]));
					brush.create([32; 2].into(), PixmapData::RgbA(image.as_raw()))
				};
				brush.draw(
					&pixmap,
					[50.; 2].into(),
					[PixmapInstance {
						pos: [0.; 2],
						dim: [
							self.viewport.screen_width as f32 / 2.,
							self.viewport.screen_height as f32 / 2.,
						],
						uv_pos: [0; 2],
						uv_dim: [32; 2],
					}],
				);
				self.block_pixmap = Some(pixmap);

				if let Some((atlas, glyphs)) = &self.render_items {
					let pixmap = if let Some(pixmap) = self.atlas_pixmap.take() {
						pixmap
					} else {
						brush.create(
							[atlas.width(), atlas.height()].into(),
							PixmapData::Luma(atlas.as_raw()),
						)
					};
					if self.show_atlas {
						brush.draw(
							&pixmap,
							[100.; 2].into(),
							[PixmapInstance {
								pos: [0.; 2],
								dim: [atlas.width() as f32, atlas.height() as f32],
								uv_pos: [0; 2],
								uv_dim: [atlas.width(), atlas.height()],
							}],
						);
					} else {
						brush.draw(
							&pixmap,
							[100.; 2].into(),
							glyphs.iter().map(|g| PixmapInstance {
								pos: g.pos,
								dim: g.dim,
								uv_pos: g.uv_pos,
								uv_dim: g.uv_dim,
							}),
						);
					}
					self.atlas_pixmap = Some(pixmap);
				}
			})
			.draw_ui(|ui| {
				let menu_items = &[
					MenuItem {
						icon: Icon::Library,
						description: "Library",
						active: false,
						action: MenuAction::OpenLibrary,
					},
					MenuItem {
						icon: Icon::LogOut,
						description: "Exit",
						active: false,
						action: MenuAction::Exit,
					},
				];
				let tool_items = &[
					None,
					Some(ToolItem {
						icon: Icon::TestTube,
						description: "Test A",
						active: false,
						action: ToolAction::TestA,
					}),
					None,
					Some(ToolItem {
						icon: Icon::TestTube,
						description: "Test B",
						active: false,
						action: ToolAction::TestB,
					}),
					Some(ToolItem {
						icon: Icon::TestTube,
						description: "Test C",
						active: false,
						action: ToolAction::TestC,
					}),
					None,
				];

				let top_panel = Panel::top("top")
					.show_inside(ui, |ui| MainMenuBar::new(self, menu_items).ui(ui));
				let is_open = top_panel.inner.context_menu_opened();

				Panel::bottom("bottom")
					.show_inside(ui, |ui| ToolBar::new(self, tool_items, is_open).ui(ui));
			});
	}

	fn event(&mut self, _event: &AppEvent) -> EventResult {
		EventResult::None
	}

	fn gesture(&mut self, _event: &GestureEvent) -> GestureResult {
		GestureResult::Unhandled
	}

	fn resize(&mut self, width: u32, height: u32) {
		self.viewport.screen_width = width;
		self.viewport.screen_height = height;
	}

	fn rescale(&mut self, scale_factor: f32) {
		self.viewport.scale_factor = scale_factor;
	}
}
