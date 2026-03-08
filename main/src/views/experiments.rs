use std::iter;
use std::sync::Arc;

use egui::TopBottomPanel;
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
use crate::renderer::pixmap_renderer::Pixmap;
use crate::renderer::pixmap_renderer::PixmapInput;
use crate::renderer::pixmap_renderer::PixmapTargetInput;
use crate::ui::MainMenuBar;
use crate::ui::MenuItem;
use crate::ui::OnAction;
use crate::ui::ToolBar;
use crate::ui::ToolItem;
use crate::views::EventResult;
use crate::views::GestureResult;
use crate::views::ViewHandle;

pub(crate) struct ExperimentsView {
	bell: AppBell,
	screen_width: u32,
	screen_height: u32,
	scale_factor: f32,
	fonts: Arc<SculpterFonts>,
	render_items: Vec<(AtlasImage, Vec<DisplayGlyph>)>,
	show_atlas: bool,
}

impl ExperimentsView {
	pub(crate) fn create(
		bell: AppBell,
		fonts: Arc<SculpterFonts>,
		screen_width: u32,
		screen_height: u32,
		scale_factor: f32,
	) -> Self {
		Self {
			bell,
			screen_width,
			screen_height,
			scale_factor,
			fonts,
			render_items: Vec::new(),
			show_atlas: false,
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
				if self.render_items.is_empty() {
					let scale_factor = Fixed::from_num(self.scale_factor);

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
							self.screen_width - 200,
							self.screen_height,
							Fixed::lit("24."),
						)
						.inspect_err(|err| log::error!("Error: {err}"))
						.unwrap();

					let mut atlas = AtlasImage::default();
					sculpter
						.write_glyph_atlas(&mut atlas)
						.inspect_err(|err| log::error!("Error: {err}"))
						.unwrap();

					self.render_items.push((atlas, block.glyphs));
				} else {
					self.render_items.clear();
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
			.draw_pixmap(
				iter::once(PixmapInput {
					pixmap: Pixmap::RgbA(
						ImageBuffer::from_pixel(32, 32, image::Rgba([128u8, 0, 0, 128])).as_raw(),
					),
					pixmap_dim: [32; 2],
					offset_pos: [50; 2],
					targets: vec![PixmapTargetInput {
						pos: [0.; 2],
						dim: [
							self.screen_width as f32 / 2.,
							self.screen_height as f32 / 2.,
						],
						tex_pos: [0; 2],
						tex_dim: [32; 2],
					}],
				})
				.chain(self.render_items.iter().map(|(atlas, glyphs)| PixmapInput {
					pixmap: Pixmap::Luma(atlas.as_raw()),
					pixmap_dim: [atlas.width(), atlas.height()],
					offset_pos: [100; 2],
					targets: if !self.show_atlas {
						glyphs
							.iter()
							.map(|g| PixmapTargetInput {
								pos: g.pos,
								dim: g.size,
								tex_pos: g.uv_pos,
								tex_dim: g.uv_size,
							})
							.collect()
					} else {
						vec![PixmapTargetInput {
							pos: [0.; 2],
							dim: [atlas.width() as f32, atlas.height() as f32],
							tex_pos: [0; 2],
							tex_dim: [atlas.width(), atlas.height()],
						}]
					},
				})),
			)
			.draw_ui(|ctx| {
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

				let top_panel = TopBottomPanel::top("top")
					.show(ctx, |ui| MainMenuBar::new(self, menu_items, false).ui(ui));
				let is_open = top_panel.inner.context_menu_opened();

				TopBottomPanel::bottom("bottom")
					.show(ctx, |ui| ToolBar::new(self, tool_items, is_open).ui(ui));
			});
	}

	fn event(&mut self, _event: &AppEvent) -> EventResult {
		EventResult::None
	}

	fn gesture(&mut self, _event: &GestureEvent) -> GestureResult {
		GestureResult::Unhandled
	}

	fn resize(&mut self, width: u32, height: u32) {
		self.screen_width = width;
		self.screen_height = height;
	}

	fn rescale(&mut self, scale_factor: f32) {
		self.scale_factor = scale_factor;
	}
}
