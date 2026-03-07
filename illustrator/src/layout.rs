use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use html5ever::LocalName;
use html5ever::local_name;
use resvg::tiny_skia;
use scribe::settings::FontConfig;
use sculpter::Axis;
use sculpter::Family;
use sculpter::Fixed;
use sculpter::FontOptions;
use sculpter::FontStyle;
use sculpter::Sculpter;
use sculpter::SculpterHandle;
use sculpter::SculpterInput;
use sculpter::Variation;
use taffy::prelude::*;
use zip::ZipArchive;

use crate::DisplayContent;
use crate::DisplayItem;
use crate::DisplayPixmap;
use crate::PageContent;
use crate::PagePosition;
use crate::Params;
use crate::html_parser::EdgeRef;
use crate::html_parser::NodeTreeBuilder;
use crate::html_parser::Text;
use crate::html_parser::TextWrapper;
use crate::html_parser::TreeBuilderError;
use crate::svg::IllustratorSvgError;
use crate::svg::SvgRender;
use crate::svg::read_svg;
use crate::svg::svg_options;

#[derive(Debug, thiserror::Error)]
pub enum IllustratorLayoutError {
	#[error(transparent)]
	TreeBuilder(#[from] TreeBuilderError),
	#[error(transparent)]
	Zip(#[from] zip::result::ZipError),
	#[error(transparent)]
	Taffy(#[from] taffy::TaffyError),
	#[error(transparent)]
	Svg(#[from] IllustratorSvgError),
	#[error(transparent)]
	SculpterShape(#[from] sculpter::SculpterShapeError),
	#[error(transparent)]
	SculpterPrinter(#[from] sculpter::SculpterPrinterError),
	#[error("Unexpected extra close")]
	UnexpectedExtraClose,
	#[error("Missing body")]
	MissingBody,
	#[error("Scale svg failed")]
	ScaleSvgFailed,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) enum TextStyle {
	#[default]
	Body,
	Bold,
	Italic,
	H1,
	H2,
	H3,
	H4,
	H5,
}

impl TextStyle {
	fn try_from(name: &LocalName) -> Option<TextStyle> {
		match *name {
			local_name!("b") | local_name!("strong") => Some(TextStyle::Bold),
			local_name!("i") | local_name!("em") => Some(TextStyle::Italic),
			local_name!("h1") => Some(TextStyle::H1),
			local_name!("h2") => Some(TextStyle::H2),
			local_name!("h3") => Some(TextStyle::H3),
			local_name!("h4") => Some(TextStyle::H4),
			local_name!("h5") => Some(TextStyle::H5),
			_ => None,
		}
	}
}

pub(crate) struct StyleSettings<'a> {
	config: &'a scribe::settings::Illustrator,

	font_regular: FontOptions<'a>,
	font_italic: FontOptions<'a>,
	font_bold: FontOptions<'a>,

	scale: f32,
	page_width: u32,
	page_height: u32,
}

impl<'a> StyleSettings<'a> {
	pub(crate) fn new(config: &'a scribe::settings::Illustrator, params: &Params) -> Self {
		let font_regular = into_font_options(&config.font_regular);
		let font_italic = into_font_options(&config.font_italic);
		let font_bold = into_font_options(&config.font_bold);

		Self {
			config,

			font_regular,
			font_italic,
			font_bold,

			scale: params.scale,
			page_width: params.page_width,
			page_height: params.page_height,
		}
	}

	fn text_style(&'a self, style: TextStyle) -> FontStyle<'a> {
		use sculpter::Fixed;

		let font_size = Fixed::from_num(self.font_size());
		let line_height_em = Fixed::from_num(self.config.line_height);

		match style {
			TextStyle::Body => FontStyle {
				font_opts: &self.font_regular,
				font_size,
				line_height_em,
			},
			TextStyle::Bold => FontStyle {
				font_opts: &self.font_bold,
				font_size,
				line_height_em,
			},
			TextStyle::Italic => FontStyle {
				font_opts: &self.font_italic,
				font_size,
				line_height_em,
			},
			TextStyle::H1 => FontStyle {
				font_opts: &self.font_regular,
				font_size: font_size * Fixed::from_num(self.config.h1.font_size_em),
				line_height_em,
			},
			TextStyle::H2 => FontStyle {
				font_opts: &self.font_regular,
				font_size: font_size * Fixed::from_num(self.config.h2.font_size_em),
				line_height_em,
			},
			TextStyle::H3 => FontStyle {
				font_opts: &self.font_regular,
				font_size: font_size * Fixed::from_num(self.config.h3.font_size_em),
				line_height_em,
			},
			TextStyle::H4 => FontStyle {
				font_opts: &self.font_regular,
				font_size: font_size * Fixed::from_num(self.config.h4.font_size_em),
				line_height_em,
			},
			TextStyle::H5 => FontStyle {
				font_opts: &self.font_regular,
				font_size: font_size * Fixed::from_num(self.config.h5.font_size_em),
				line_height_em,
			},
		}
	}

	fn font_size(&self) -> f32 {
		self.config.font_size * self.scale
	}

	fn line_height(&self) -> f32 {
		self.config.line_height * self.config.font_size * self.scale
	}

	fn page_height_padded(&self) -> f32 {
		self.page_height as f32
			- self.config.padding.top_em * self.font_size()
			- self.config.padding.bottom_em * self.font_size()
	}
	fn page_width_padded(&self) -> f32 {
		self.page_width as f32
			- self.config.padding.left_em * self.font_size()
			- self.config.padding.right_em * self.font_size()
	}

	fn padding_top(&self) -> f32 {
		self.config.padding.top_em * self.font_size()
	}

	fn padding_left(&self) -> f32 {
		self.config.padding.left_em * self.font_size()
	}

	fn paragraph_padding(&self) -> f32 {
		self.config.padding.paragraph_em * self.font_size()
	}

	fn element_style(&self, name: &LocalName) -> Style {
		match *name {
			local_name!("p") => Style {
				padding: Rect {
					top: zero(),
					bottom: length(self.paragraph_padding()),
					left: zero(),
					right: zero(),
				},
				..Style::default()
			},
			_ => Style::default(),
		}
	}
}

pub(crate) fn into_font_options<'a>(value: &'a FontConfig) -> FontOptions<'a> {
	let family = match value.family.as_str() {
		"serif" => Family::Serif,
		"sans-serif" => Family::SansSerif,
		family => Family::Name(family),
	};
	let variations = [
		value
			.variation
			.wght
			.map(|v| Variation::new(Axis::Wght, Fixed::from_num(v))),
		value
			.variation
			.wdth
			.map(|v| Variation::new(Axis::Wdth, Fixed::from_num(v))),
		value
			.variation
			.ital
			.map(|v| Variation::new(Axis::Ital, Fixed::from_num(v))),
		value
			.variation
			.slnt
			.map(|v| Variation::new(Axis::Slnt, Fixed::from_num(v))),
		value
			.variation
			.opzs
			.map(|v| Variation::new(Axis::Opzs, Fixed::from_num(v))),
	]
	.into_iter()
	.flatten()
	.collect();

	FontOptions::new(family, variations)
}

enum Edge {
	Open(NodeId),
	Close(NodeId),
}

struct TaffyTreeIter<'a, C = ()> {
	tree: &'a TaffyTree<C>,
	stack: Vec<Edge>,
}

impl<'a, C> TaffyTreeIter<'a, C> {
	pub fn new(tree: &'a TaffyTree<C>, id: NodeId) -> Self {
		let stack = tree
			.children(id)
			.unwrap_or_default()
			.into_iter()
			.rev()
			.map(Edge::Open)
			.collect();
		Self { tree, stack }
	}
}

impl<'a, C> Iterator for TaffyTreeIter<'a, C> {
	type Item = Edge;

	fn next(&mut self) -> Option<Self::Item> {
		let node = self.stack.pop()?;
		if let Edge::Open(id) = node {
			self.stack.push(Edge::Close(id));
			let children = self
				.tree
				.children(id)
				.unwrap_or_default()
				.into_iter()
				.rev()
				.map(Edge::Open);
			self.stack.extend(children);
		}
		Some(node)
	}
}

#[derive(Debug)]
enum NodeContent {
	Block,
	Text,
	Svg,
}

#[derive(Debug)]
#[allow(unused)]
struct NodeContext {
	element: u32,
	content: NodeContent,
}

impl NodeContext {
	fn block(element: u32) -> Self {
		Self {
			element,
			content: NodeContent::Block,
		}
	}

	fn text(element: u32) -> Self {
		Self {
			element,
			content: NodeContent::Text,
		}
	}

	fn svg(element: u32) -> Self {
		Self {
			element,
			content: NodeContent::Svg,
		}
	}
}

pub(crate) struct PageLayouterEmpty;
pub(crate) struct PageLayouterLoaded<'handle> {
	content: NodeId,
	texts: HashMap<NodeId, SculpterHandle<'handle>>,
	svgs: HashMap<NodeId, SvgRender>,
}

pub(crate) struct PageLayouter<'a, TState = PageLayouterEmpty> {
	builder: NodeTreeBuilder,
	taffy_tree: taffy::TaffyTree<NodeContext>,
	pub(crate) sculpter: Sculpter<'a>,
	state: TState,
}

impl<'a> PageLayouter<'a, PageLayouterEmpty> {
	pub(crate) fn new(sculpter: Sculpter<'a>) -> Self {
		Self {
			builder: NodeTreeBuilder::new(),
			taffy_tree: taffy::TaffyTree::new(),
			sculpter,
			state: PageLayouterEmpty,
		}
	}

	pub(crate) fn load<'handle, R: io::Seek + io::Read + Sync + Send>(
		self,
		archive: &mut ZipArchive<R>,
		path: &Path,
		settings: &StyleSettings<'a>,
	) -> Result<PageLayouter<'a, PageLayouterLoaded<'handle>>, IllustratorLayoutError> {
		let Self {
			builder,
			mut taffy_tree,
			mut sculpter,
			..
		} = self;

		let node_tree = {
			let file = archive.by_path(path)?;
			builder.read_from(file)?
		};
		let svg_options = svg_options(
			Mutex::new(archive),
			path.parent().unwrap_or(Path::new("OEBPS/")),
		);

		let page_height = settings.page_height_padded();
		let page_width = settings.page_width_padded();

		let content: NodeId = taffy_tree.new_leaf(Style {
			size: taffy::Size {
				width: length(page_width),
				height: auto(),
			},
			..Default::default()
		})?;

		let mut current = content;

		let mut styles = Vec::new();
		let mut inputs = Vec::new();
		let mut texts = HashMap::new();

		let mut svg_buf = String::new();
		let mut svgs = HashMap::new();

		let mut node_iter = node_tree
			.body_iter()
			.ok_or(IllustratorLayoutError::MissingBody)?;
		while let Some(edge) = node_iter.next() {
			match edge {
				EdgeRef::OpenElement(el) if el.local_name() == &local_name!("svg") => {
					let svg = read_svg(&mut svg_buf, &el, &mut node_iter, &svg_options)?;
					let size = svg.size();
					let scale = scale_to_fit(size.width(), size.height(), page_width, page_height);
					let style = Style {
						size: taffy::Size::from_lengths(
							size.width() * scale,
							size.height() * scale,
						),
						..Default::default()
					};
					let node =
						taffy_tree.new_leaf_with_context(style, NodeContext::svg(el.id.value()))?;
					svgs.insert(node, SvgRender { scale, svg });
					taffy_tree.add_child(current, node)?;
				}
				EdgeRef::OpenElement(el) if is_inline(el.local_name()) => {
					if let Some(text_style) = TextStyle::try_from(el.local_name()) {
						styles.push((el.id, text_style))
					}
				}
				EdgeRef::CloseElement(id, name) if is_inline(&name.local) => {
					if styles.last().is_some_and(|(el_id, _)| *el_id == id) {
						styles.pop();
					}
				}
				EdgeRef::OpenElement(el) => {
					if let Some(text_style) = TextStyle::try_from(el.local_name()) {
						styles.push((el.id, text_style))
					}
					if !inputs.is_empty() {
						let node = taffy_tree.new_leaf_with_context(
							Style::default(),
							NodeContext::text(el.id.value()),
						)?;
						let handle = sculpter.shape(inputs.drain(..).map(|(tendril, style)| {
							SculpterInput {
								style: settings.text_style(style),
								input: tendril,
							}
						}))?;
						texts.insert(node, handle);
						taffy_tree.add_child(current, node)?;
					}
					let node = taffy_tree.new_leaf_with_context(
						settings.element_style(el.local_name()),
						NodeContext::block(el.id.value()),
					)?;
					taffy_tree.add_child(current, node)?;
					current = node;
				}
				EdgeRef::CloseElement(id, _name) => {
					if styles.last().is_some_and(|(el_id, _)| *el_id == id) {
						styles.pop();
					}
					if !inputs.is_empty() {
						let node = taffy_tree.new_leaf_with_context(
							Style::default(),
							NodeContext::text(id.value()),
						)?;
						let handle = sculpter.shape(inputs.drain(..).map(|(tendril, style)| {
							SculpterInput {
								style: settings.text_style(style),
								input: tendril,
							}
						}))?;
						texts.insert(node, handle);
						taffy_tree.add_child(current, node)?;
					}
					current = taffy_tree
						.parent(current)
						.ok_or(IllustratorLayoutError::UnexpectedExtraClose)?;
				}
				EdgeRef::Text(TextWrapper { t: Text { t }, .. }) => {
					let text_style = styles.last().map(|(_, s)| *s).unwrap_or_default();
					inputs.push((t, text_style));
				}
			}
		}

		debug_assert!(inputs.is_empty());
		debug_assert!(styles.is_empty());
		drop(inputs);
		drop(styles);
		let builder = node_tree.into_builder();

		taffy_tree.compute_layout_with_measure(
			content,
			taffy::Size::MAX_CONTENT,
			|known_dimensions, available_space, node_id, _node_context, _style| match texts
				.get(&node_id)
			{
				Some(handle) => {
					let max_width = known_dimensions
						.width
						.unwrap_or(match available_space.width {
							AvailableSpace::MinContent => 0.0,
							AvailableSpace::MaxContent => page_width,
							AvailableSpace::Definite(width) => width,
						});
					let result = sculpter.measure(handle, max_width as u32);
					taffy::Size {
						width: max_width,
						height: result.height as f32,
					}
				}
				None => taffy::Size::ZERO,
			},
		)?;

		Ok(PageLayouter {
			builder,
			taffy_tree,
			sculpter,
			state: PageLayouterLoaded {
				content,
				texts,
				svgs,
			},
		})
	}
}

impl<'a> PageLayouter<'a, PageLayouterLoaded<'_>> {
	pub(crate) fn pages(
		&mut self,
		settings: &StyleSettings<'a>,
	) -> Result<Vec<PageContent>, IllustratorLayoutError> {
		let page_height = settings.page_height_padded();
		let padding_top = settings.padding_top();
		let padding_left = settings.padding_left();

		let mut page_end = 0.;
		let mut offset = taffy::Point::<f32>::zero();
		let mut pages = Vec::new();
		let mut page = PageContent {
			position: PagePosition::First,
			index: 0,
			elements: 0..0,
			items: Vec::new(),
		};

		let tree = &self.taffy_tree;
		for edge in TaffyTreeIter::new(tree, self.state.content) {
			match edge {
				Edge::Open(id) => {
					let l = tree.layout(id)?;
					offset = taffy::Point {
						x: offset.x + l.location.x,
						y: offset.y + l.location.y,
					};

					if let Some(ctx) = tree.get_node_context(id) {
						page.elements.end = ctx.element;
						let content = match ctx.content {
							NodeContent::Text => {
								if let Some(text) = self.state.texts.get_mut(&id) {
									// TODO: Break block
									let render = self.sculpter.render_block(
										text,
										l.size.width as u32,
										l.size.height as u32,
										settings.line_height() as u32,
									)?;
									Some(DisplayContent::Text(render))
								} else {
									None
								}
							}
							NodeContent::Svg => {
								if let Some(SvgRender { scale, svg }) = self.state.svgs.get(&id) {
									let pixmap_size = svg
										.size()
										.to_int_size()
										.scale_by(*scale)
										.ok_or(IllustratorLayoutError::ScaleSvgFailed)?;
									let transform =
										tiny_skia::Transform::from_scale(*scale, *scale);
									let mut pixmap = tiny_skia::Pixmap::new(
										pixmap_size.width(),
										pixmap_size.height(),
									)
									.unwrap();
									resvg::render(svg, transform, &mut pixmap.as_mut());

									Some(DisplayContent::Pixmap(DisplayPixmap {
										pixmap_width: pixmap.width(),
										pixmap_height: pixmap.height(),
										pixmap_rgba: pixmap.take(),
									}))
								} else {
									None
								}
							}
							NodeContent::Block => None,
						};

						if let Some(content) = content {
							let content_end = offset.y + l.size.height;
							if content_end - page_end > page_height {
								log::debug!("Page break at el {}", page.elements.end,);

								let index = page.index + 1;
								let elements_end = page.elements.end;
								pages.push(page);
								page = PageContent {
									position: PagePosition::empty(),
									index,
									elements: elements_end..elements_end,
									items: Vec::new(),
								};
								page_end = offset.y;
							}

							page.items.push(DisplayItem {
								pos: taffy::Point {
									x: padding_left + offset.x,
									y: padding_top + offset.y - page_end,
								}
								.into(),
								size: l.size.into(),
								content,
							});
						}
					}
				}
				Edge::Close(id) => {
					let l = tree.layout(id)?;
					offset = taffy::Point {
						x: offset.x - l.location.x,
						y: offset.y - l.location.y,
					};
				}
			}
		}
		if !page.items.is_empty() || pages.is_empty() {
			page.position.set(PagePosition::Last, true);
			pages.push(page);
		} else if let Some(last) = pages.last_mut() {
			last.position.set(PagePosition::Last, true);
		}
		log::trace!("Generated {} pages", pages.len());
		debug_assert!(!pages.is_empty(), "Must have at least one page per chapter");

		Ok(pages)
	}

	pub(crate) fn reset(self) -> PageLayouter<'a, PageLayouterEmpty> {
		let Self {
			builder,
			taffy_tree,
			sculpter,
			..
		} = self;
		PageLayouter {
			builder,
			taffy_tree,
			sculpter,
			state: PageLayouterEmpty,
		}
	}
}

fn is_inline(name: &LocalName) -> bool {
	name == &local_name!("strong")
		|| name == &local_name!("b")
		|| name == &local_name!("em")
		|| name == &local_name!("i")
		|| name == &local_name!("span")
}

fn scale_to_fit(width: f32, height: f32, max_width: f32, max_height: f32) -> f32 {
	if width < max_width && height < max_height {
		1.0
	} else {
		let ws = max_width / width;
		let hs = max_height / height;
		ws.min(hs)
	}
}
