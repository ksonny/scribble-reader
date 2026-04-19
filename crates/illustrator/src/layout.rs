use std::io;
use std::mem;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use fixed::types::U26F6;
use html5ever::LocalName;
use html5ever::local_name;
use image::RgbaImage;
use pixelator::PixelatorAssistant;
use pixelator::PixelatorTextures;
use pixelator::PixmapData;
use resvg::tiny_skia;
use resvg::usvg;
use scribe::config::FontConfig;
use scribe::config::IllustratorProfile;
use sculpter::AtlasImage;
use sculpter::Axis;
use sculpter::Family;
use sculpter::Fixed;
use sculpter::FontOptions;
use sculpter::FontStyle;
use sculpter::Sculpter;
use sculpter::SculpterHandle;
use sculpter::SculpterInput;
use sculpter::SculpterPrinterError;
use sculpter::Variation;
use taffy::prelude::*;
use zip::ZipArchive;

use crate::DisplayContent;
use crate::DisplayItem;
use crate::DisplayPixmap;
use crate::PageContent;
use crate::PageFlags;
use crate::Params;
use crate::html_parser::EdgeRef;
use crate::html_parser::NodeTreeBuilder;
use crate::html_parser::Text;
use crate::html_parser::TextWrapper;
use crate::html_parser::TreeBuilderError;
use crate::svg::HORIZONTAL_RULER_SVG;
use crate::svg::IllustratorSvgError;
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
	Image(#[from] image::ImageError),
	#[error(transparent)]
	SculpterShape(#[from] sculpter::SculpterShapeError),
	#[error(transparent)]
	SculpterPrinter(#[from] sculpter::SculpterPrinterError),
	#[error(transparent)]
	Usvg(#[from] resvg::usvg::Error),
	#[error("Unexpected extra close")]
	UnexpectedExtraClose,
	#[error("Missing body")]
	MissingBody,
	#[error("Scale svg failed: {0}")]
	ScaleSvgFailed(f32),
	#[error("Missing content for text node")]
	MissingTextContent(NodeId),
	#[error("Missing content for svg node")]
	MissingSvgContent(NodeId),
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
	profile: &'a IllustratorProfile,

	font_regular: FontOptions<'a>,
	font_italic: FontOptions<'a>,
	font_bold: FontOptions<'a>,

	scale: f32,
	page_width: u32,
	page_height: u32,
}

impl<'a> StyleSettings<'a> {
	pub(crate) fn new(profile: &'a IllustratorProfile, params: &Params) -> Self {
		let font_regular = into_font_options(&profile.font_regular);
		let font_italic = into_font_options(&profile.font_italic);
		let font_bold = into_font_options(&profile.font_bold);

		Self {
			profile,

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
		let line_height_em = Fixed::from_num(self.profile.line_height);

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
				font_size: font_size * Fixed::from_num(self.profile.h1.font_size_em),
				line_height_em,
			},
			TextStyle::H2 => FontStyle {
				font_opts: &self.font_regular,
				font_size: font_size * Fixed::from_num(self.profile.h2.font_size_em),
				line_height_em,
			},
			TextStyle::H3 => FontStyle {
				font_opts: &self.font_regular,
				font_size: font_size * Fixed::from_num(self.profile.h3.font_size_em),
				line_height_em,
			},
			TextStyle::H4 => FontStyle {
				font_opts: &self.font_regular,
				font_size: font_size * Fixed::from_num(self.profile.h4.font_size_em),
				line_height_em,
			},
			TextStyle::H5 => FontStyle {
				font_opts: &self.font_regular,
				font_size: font_size * Fixed::from_num(self.profile.h5.font_size_em),
				line_height_em,
			},
		}
	}

	fn font_size(&self) -> f32 {
		self.profile.font_size * self.scale
	}

	fn em_to_px(&self, n: f32) -> f32 {
		n * self.profile.font_size * self.scale
	}

	fn min_line_height(&self) -> f32 {
		self.em_to_px(self.profile.line_height)
	}

	fn page_height_padded(&self) -> f32 {
		(self.page_height as f32
			- self.em_to_px(self.profile.padding.top_em)
			- self.em_to_px(self.profile.padding.bottom_em))
		.floor()
	}
	fn page_width_padded(&self) -> f32 {
		(self.page_width as f32
			- self.em_to_px(self.profile.padding.left_em)
			- self.em_to_px(self.profile.padding.right_em))
		.floor()
	}

	fn padding_top(&self) -> f32 {
		self.em_to_px(self.profile.padding.top_em)
	}

	fn padding_left(&self) -> f32 {
		self.em_to_px(self.profile.padding.left_em)
	}

	fn element_style(&self, name: &LocalName) -> Style {
		match *name {
			local_name!("h1") => Style {
				display: Display::Block,
				box_sizing: BoxSizing::ContentBox,
				padding: Rect {
					top: zero(),
					bottom: length(self.em_to_px(self.profile.h1.padding_em)),
					left: zero(),
					right: zero(),
				},
				..Style::default()
			},
			local_name!("h2") => Style {
				display: Display::Block,
				box_sizing: BoxSizing::ContentBox,
				padding: Rect {
					top: zero(),
					bottom: length(self.em_to_px(self.profile.h2.padding_em)),
					left: zero(),
					right: zero(),
				},
				..Style::default()
			},
			local_name!("h3") => Style {
				display: Display::Block,
				box_sizing: BoxSizing::ContentBox,
				padding: Rect {
					top: zero(),
					bottom: length(self.em_to_px(self.profile.h3.padding_em)),
					left: zero(),
					right: zero(),
				},
				..Style::default()
			},
			local_name!("h4") => Style {
				display: Display::Block,
				box_sizing: BoxSizing::ContentBox,
				padding: Rect {
					top: zero(),
					bottom: length(self.em_to_px(self.profile.h4.padding_em)),
					left: zero(),
					right: zero(),
				},
				..Style::default()
			},
			local_name!("h5") => Style {
				display: Display::Block,
				box_sizing: BoxSizing::ContentBox,
				padding: Rect {
					top: zero(),
					bottom: length(self.em_to_px(self.profile.h5.padding_em)),
					left: zero(),
					right: zero(),
				},
				..Style::default()
			},
			local_name!("p") => Style {
				display: Display::Block,
				box_sizing: BoxSizing::ContentBox,
				padding: Rect {
					top: zero(),
					bottom: length(self.em_to_px(self.profile.padding.paragraph_em)),
					left: zero(),
					right: zero(),
				},
				..Style::default()
			},
			local_name!("hr") => Style {
				display: Display::Block,
				size: taffy::Size {
					width: auto(),
					height: length(self.em_to_px(1.2)),
				},
				margin: Rect {
					top: zero(),
					bottom: length(self.em_to_px(self.profile.padding.paragraph_em)),
					left: zero(),
					right: zero(),
				},
				..Style::default()
			},
			local_name!("img") => Style {
				display: Display::Block,
				margin: Rect {
					top: zero(),
					bottom: length(self.em_to_px(self.profile.padding.paragraph_em)),
					left: zero(),
					right: zero(),
				},
				..Style::default()
			},
			_ => Style {
				display: Display::Block,
				..Style::default()
			},
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
	Text(SculpterHandle),
	Svg(Arc<usvg::Tree>),
	Image(Arc<RgbaImage>),
}

#[derive(Debug)]
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

	fn text(element: u32, handle: SculpterHandle) -> Self {
		Self {
			element,
			content: NodeContent::Text(handle),
		}
	}

	fn svg(element: u32, tree: Arc<usvg::Tree>) -> Self {
		Self {
			element,
			content: NodeContent::Svg(tree),
		}
	}

	fn image(element: u32, image: Arc<RgbaImage>) -> Self {
		Self {
			element,
			content: NodeContent::Image(image),
		}
	}
}

pub(crate) struct PageLayouterEmpty;
pub(crate) struct PageLayouterLoaded {
	content_id: NodeId,
}

pub(crate) struct PageLayouter<'a, TState = PageLayouterEmpty> {
	builder: NodeTreeBuilder,
	buffer: Vec<u8>,
	taffy_tree: taffy::TaffyTree<NodeContext>,
	sculpter: Sculpter<'a>,
	state: TState,
}

impl<'layout> PageLayouter<'layout, PageLayouterEmpty> {
	pub(crate) fn new(sculpter: Sculpter<'layout>) -> Self {
		Self {
			builder: NodeTreeBuilder::new(),
			buffer: Vec::new(),
			taffy_tree: taffy::TaffyTree::new(),
			sculpter,
			state: PageLayouterEmpty,
		}
	}

	pub(crate) fn load_archive<'settings, R: io::Seek + io::Read + Sync + Send>(
		self,
		archive: &mut ZipArchive<R>,
		root: &Path,
		path: &Path,
		settings: &StyleSettings<'settings>,
	) -> Result<PageLayouter<'layout, PageLayouterLoaded>, IllustratorLayoutError> {
		let Self {
			builder,
			buffer,
			mut taffy_tree,
			mut sculpter,
			..
		} = self;

		let node_tree = {
			let file = archive.by_path(path)?;
			builder.read_from(file)?
		};
		let svg_options = svg_options(Mutex::new(archive), root);

		let page_width = settings.page_width_padded();
		let page_height = settings.page_height_padded();
		let min_line_height = Fixed::from_num(settings.min_line_height());

		let content_id = taffy_tree.new_leaf(Style {
			display: Display::Block,
			size: taffy::Size {
				width: length(page_width),
				height: auto(),
			},
			..Style::default()
		})?;

		let src_attr_name =
			html5ever::QualName::new(None, html5ever::ns!(), html5ever::local_name!("src"));

		let mut current = content_id;

		let mut styles = Vec::new();
		let mut inputs = Vec::new();
		let mut svg_buf = String::new();

		#[cfg(debug_assertions)]
		let mut max_el_id = 0;

		let mut node_iter = node_tree
			.body_iter()
			.ok_or(IllustratorLayoutError::MissingBody)?;
		while let Some(edge) = node_iter.next() {
			match edge {
				EdgeRef::OpenElement(el) if el.local_name() == &local_name!("svg") => {
					let svg = read_svg(&mut svg_buf, &el, &mut node_iter)?;

					let container = taffy_tree.new_leaf_with_context(
						Style {
							display: Display::Flex,
							justify_content: Some(AlignContent::Center),
							..Style::default()
						},
						NodeContext::block(el.id.value()),
					)?;
					taffy_tree.add_child(current, container)?;

					let tree = usvg::Tree::from_str(svg, &svg_options)?;
					let node = taffy_tree.new_leaf_with_context(
						settings.element_style(el.local_name()),
						NodeContext::svg(el.id.value(), Arc::new(tree)),
					)?;
					taffy_tree.add_child(container, node)?;
				}
				EdgeRef::OpenElement(el) if el.local_name() == &local_name!("hr") => {
					take_until_closed(&mut node_iter, el.id);

					let container = taffy_tree.new_leaf_with_context(
						Style {
							display: Display::Flex,
							justify_content: Some(AlignContent::Center),
							..Style::default()
						},
						NodeContext::block(el.id.value()),
					)?;
					taffy_tree.add_child(current, container)?;

					let node = taffy_tree.new_leaf_with_context(
						settings.element_style(el.local_name()),
						NodeContext::svg(el.id.value(), HORIZONTAL_RULER_SVG.clone()),
					)?;
					taffy_tree.add_child(container, node)?;
				}
				EdgeRef::OpenElement(el) if el.local_name() == &local_name!("img") => {
					take_until_closed(&mut node_iter, el.id);

					if let Some(src) = el.el.attrs.get(&src_attr_name)
						&& let Some(image) =
							(svg_options.image_href_resolver.resolve_string)(src, &svg_options)
					{
						match image {
							usvg::ImageKind::JPEG(data)
							| usvg::ImageKind::PNG(data)
							| usvg::ImageKind::GIF(data)
							| usvg::ImageKind::WEBP(data) => {
								if let Some(image) = image::load_from_memory(data.as_slice())
									.inspect_err(|e| log::error!("Failed to load image {src}: {e}"))
									.ok()
									.map(|image| image.into_rgba8())
								{
									let node = taffy_tree.new_leaf_with_context(
										Style {
											size: Size {
												width: length(image.width() as f32),
												height: length(image.height() as f32),
											},
											..settings.element_style(el.local_name())
										},
										NodeContext::image(el.id.value(), Arc::new(image)),
									)?;
									taffy_tree.add_child(current, node)?;
								}
							}
							usvg::ImageKind::SVG(tree) => {
								let container = taffy_tree.new_leaf_with_context(
									Style {
										display: Display::Flex,
										justify_content: Some(AlignContent::Center),
										..Style::default()
									},
									NodeContext::block(el.id.value()),
								)?;
								taffy_tree.add_child(current, container)?;

								let node = taffy_tree.new_leaf_with_context(
									settings.element_style(el.local_name()),
									NodeContext::svg(el.id.value(), Arc::new(tree)),
								)?;
								taffy_tree.add_child(container, node)?;
							}
						};
					}
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

					let text_el_id = inputs
						.first()
						.map(|(el_id, _, _)| crate::html_parser::NodeId::value(el_id));
					if let Some(el_id) = text_el_id {
						#[cfg(debug_assertions)]
						{
							debug_assert!(
								max_el_id < el_id,
								"Non sequential element id in content"
							);
							max_el_id = el_id;
						}
						let handle =
							sculpter.shape(inputs.drain(..).map(|(_, tendril, style)| {
								SculpterInput {
									style: settings.text_style(style),
									input: tendril,
								}
							}))?;
						let node = taffy_tree.new_leaf_with_context(
							Style::default(),
							NodeContext::text(el_id, handle),
						)?;
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

					let text_el_id = inputs
						.first()
						.map(|(el_id, _, _)| crate::html_parser::NodeId::value(el_id));
					if let Some(el_id) = text_el_id {
						#[cfg(debug_assertions)]
						{
							debug_assert!(
								max_el_id < el_id,
								"Non sequential element id in content"
							);
							max_el_id = el_id;
						}
						let handle =
							sculpter.shape(inputs.drain(..).map(|(_, tendril, style)| {
								SculpterInput {
									style: settings.text_style(style),
									input: tendril,
								}
							}))?;
						let node = taffy_tree.new_leaf_with_context(
							Style::default(),
							NodeContext::text(el_id, handle),
						)?;
						taffy_tree.add_child(current, node)?;
					}

					current = taffy_tree
						.parent(current)
						.ok_or(IllustratorLayoutError::UnexpectedExtraClose)?;
				}
				EdgeRef::Text(TextWrapper { t: Text { t }, id }) => {
					let text_style = styles.last().map(|(_, s)| *s).unwrap_or_default();
					inputs.push((id, t, text_style));
				}
			}
		}

		debug_assert!(inputs.is_empty());
		debug_assert!(styles.is_empty());
		drop(inputs);
		drop(styles);
		let builder = node_tree.into_builder();

		taffy_tree.compute_layout_with_measure(
			content_id,
			taffy::Size::MAX_CONTENT,
			|known_dimensions, available_space, _node_id, node_context, _style| {
				if let Size {
					width: Some(width),
					height: Some(height),
				} = known_dimensions
				{
					return Size { width, height };
				}
				let Some(node_context) = node_context else {
					return taffy::Size::ZERO;
				};

				let max_width = known_dimensions.width.or(match available_space.width {
					AvailableSpace::MinContent => None,
					AvailableSpace::MaxContent => Some(page_width),
					AvailableSpace::Definite(width) => Some(width),
				});
				let max_height = known_dimensions.height.or(match available_space.width {
					AvailableSpace::MinContent => None,
					AvailableSpace::MaxContent => Some(page_height),
					AvailableSpace::Definite(height) => Some(height),
				});

				match node_context.content {
					NodeContent::Text(ref handle) => {
						let max_width = max_width.unwrap_or(page_width);
						let result = sculpter.measure(handle, max_width as u32, min_line_height);
						taffy::Size {
							width: max_width,
							height: result.height.to_num::<f32>().ceil(),
						}
					}
					NodeContent::Svg(ref tree) => {
						let size = tree.size();
						let scale = scale_to_fit(
							size.width(),
							size.height(),
							max_width.unwrap_or(size.width()).min(page_width),
							max_height.unwrap_or(size.height()).min(page_height),
						);
						taffy::Size {
							width: size.width() * scale,
							height: size.height() * scale,
						}
					}
					NodeContent::Image(ref image) => {
						let width = image.width() as f32;
						let height = image.height() as f32;
						let scale = scale_to_fit(
							width,
							height,
							max_width.unwrap_or(width).min(page_width),
							max_height.unwrap_or(height).min(page_height),
						);
						taffy::Size {
							width: width * scale,
							height: height * scale,
						}
					}
					NodeContent::Block => taffy::Size::ZERO,
				}
			},
		)?;

		Ok(PageLayouter {
			builder,
			buffer,
			taffy_tree,
			sculpter,
			state: PageLayouterLoaded { content_id },
		})
	}
}

fn take_until_closed(
	node_iter: &mut crate::html_parser::NodeTreeIter<'_>,
	el_id: crate::html_parser::NodeId,
) {
	for edge in node_iter.by_ref() {
		if let EdgeRef::CloseElement(id, _name) = edge
			&& id == el_id
		{
			break;
		}
	}
}

struct PageBreaker {
	padding_left: f32,
	padding_top: f32,
	page_height: f32,

	page_offset: f32,
	page: PageContent,
	pages: Vec<PageContent>,
}

impl PageBreaker {
	fn new(settings: &StyleSettings<'_>) -> Self {
		let padding_left = settings.padding_left();
		let padding_top = settings.padding_top();
		let page_height = settings.page_height_padded();

		Self {
			padding_left,
			padding_top,
			page_height,
			page_offset: 0.,
			page: PageContent {
				flags: PageFlags::First,
				elements: U26F6::ZERO..U26F6::ZERO,
				items: Vec::new(),
			},
			pages: Vec::new(),
		}
	}

	fn page_remaining(&self, y: f32) -> f32 {
		self.page_height - (y - self.page_offset)
	}

	fn add_content<TContent: Into<DisplayContent>>(
		&mut self,
		el: U26F6,
		pos: taffy::Point<f32>,
		size: taffy::Size<f32>,
		content: TContent,
	) {
		debug_assert!(
			self.page_height >= size.height,
			"Tried adding content larger than page height s{} p{} t{}",
			size.height,
			self.page_height,
			std::any::type_name::<TContent>()
		);
		let page_rem = self.page_remaining(pos.y);
		if page_rem < size.height {
			log::debug!("Add page {}", self.pages.len());
			self.add_page(pos.y);
		}

		self.page.items.push(DisplayItem {
			pos: crate::Position {
				x: pos.x + self.padding_left,
				y: pos.y - self.page_offset + self.padding_top,
			},
			size: size.into(),
			content: content.into(),
		});
		self.page.elements.end = el;
	}

	fn add_page(&mut self, y: f32) {
		let element = self.page.elements.end;
		let page = mem::replace(
			&mut self.page,
			PageContent {
				flags: PageFlags::empty(),
				elements: element..element,
				items: Vec::new(),
			},
		);
		self.pages.push(page);
		self.page_offset = y;
	}

	fn finish(self) -> Vec<PageContent> {
		let Self {
			mut page,
			mut pages,
			..
		} = self;

		if !page.items.is_empty() || pages.is_empty() {
			page.flags.set(PageFlags::Last, true);
			pages.push(page);
		} else if let Some(last) = pages.last_mut() {
			last.flags.set(PageFlags::Last, true);
		}

		pages
	}
}

impl<'layout> PageLayouter<'layout, PageLayouterLoaded> {
	pub(crate) fn layout<'settings>(
		self,
		pixelator: &PixelatorAssistant,
		settings: &StyleSettings<'settings>,
	) -> Result<(PageLayouter<'layout, PageLayouterEmpty>, Vec<PageContent>), IllustratorLayoutError>
	{
		let Self {
			builder,
			mut buffer,
			mut taffy_tree,
			mut sculpter,
			state: PageLayouterLoaded { content_id },
		} = self;

		let min_line_height = Fixed::from_num(settings.min_line_height());

		let mut breaker = PageBreaker::new(settings);
		let mut cursor = taffy::Point::ZERO;

		for edge in TaffyTreeIter::new(&taffy_tree, content_id) {
			match edge {
				Edge::Open(id) => {
					let l = taffy_tree.layout(id)?;
					cursor = taffy::Point {
						x: cursor.x + l.location.x,
						y: cursor.y + l.location.y,
					};

					let Some(ctx) = taffy_tree.get_node_context(id) else {
						continue;
					};
					match &ctx.content {
						NodeContent::Text(handle) => {
							let mut text = handle.clone();
							let el = U26F6::from_num(ctx.element);
							let glyph_len = U26F6::from_num(text.glyph_range().len());

							let mut page_added = false;
							let mut offset = 0.;
							while !text.is_empty() {
								debug_assert!(
									offset <= l.size.height,
									"Accumulated block height exceeded measured height {} <= {}",
									offset,
									l.size.height
								);

								let glyph_rem = text.glyph_range().len();

								let pos = cursor + taffy::Point { x: 0., y: offset };
								let page_rem = breaker.page_remaining(pos.y);
								let render = sculpter.render_block(
									&mut text,
									l.size.width as u32,
									page_rem as u32,
									min_line_height,
								)?;
								if render.block_height > Fixed::ZERO {
									let block_height = render.block_height.to_num::<f32>();
									debug_assert!(
										block_height <= page_rem,
										"Block exceeds remaining space {block_height} >= {page_rem}"
									);

									let part_el =
										U26F6::ONE - (U26F6::from_num(glyph_rem) / glyph_len);
									breaker.add_content(
										el + part_el,
										pos,
										taffy::Size {
											width: l.size.width,
											height: block_height,
										},
										render,
									);
									offset += block_height;
									page_added = false;
								} else if !page_added {
									// Wasn't enough to fit any lines in, add page
									breaker.add_page(pos.y);
									page_added = true;
								} else {
									log::warn!(
										"Failed to fit single line on page, skip section {el}"
									);
									break;
								}
							}
						}
						NodeContent::Svg(tree) => {
							let size = tree.size();
							let scale = scale_to_fit(
								size.width(),
								size.height(),
								l.size.width,
								l.size.height,
							);
							let pixmap_size = tree
								.size()
								.to_int_size()
								.scale_by(scale)
								.ok_or(IllustratorLayoutError::ScaleSvgFailed(scale))?;

							let byte_size = pixmap_size.width() as usize
								* pixmap_size.height() as usize
								* tiny_skia::BYTES_PER_PIXEL;
							buffer.resize(byte_size, 0u8);

							let mut target = tiny_skia::PixmapMut::from_bytes(
								&mut buffer,
								pixmap_size.width(),
								pixmap_size.height(),
							)
							.unwrap();
							let transform = tiny_skia::Transform::from_scale(scale, scale);
							resvg::render(tree, transform, &mut target);

							let pixmap = pixelator.create(
								[target.width(), target.height()].into(),
								PixmapData::RgbA(target.data_mut()),
							);

							breaker.add_content(
								U26F6::from_num(ctx.element),
								cursor,
								taffy::Size {
									width: size.width() * scale,
									height: size.height() * scale,
								},
								DisplayPixmap {
									pixmap,
									pixmap_width: target.width(),
									pixmap_height: target.height(),
								},
							);
						}
						NodeContent::Image(image) => {
							let width = image.width();
							let height = image.height();

							let pixmap = pixelator
								.create([width, height].into(), PixmapData::RgbA(image.as_raw()));

							breaker.add_content(
								U26F6::from_num(ctx.element),
								cursor,
								taffy::Size {
									width: width as f32,
									height: height as f32,
								},
								DisplayPixmap {
									pixmap,
									pixmap_width: width,
									pixmap_height: height,
								},
							);
						}
						NodeContent::Block => {}
					};
				}
				Edge::Close(id) => {
					let l = taffy_tree.layout(id)?;
					cursor = taffy::Point {
						x: cursor.x - l.location.x,
						y: cursor.y - l.location.y,
					};
				}
			}
		}

		taffy_tree.clear();
		let sculpter = sculpter.clear_glyphs();

		let layouter = PageLayouter {
			builder,
			buffer,
			taffy_tree,
			sculpter,
			state: PageLayouterEmpty,
		};

		Ok((layouter, breaker.finish()))
	}
}

impl<TState> PageLayouter<'_, TState> {
	pub fn write_glyph_atlas(
		&mut self,
		atlas: &mut AtlasImage,
	) -> Result<(), SculpterPrinterError> {
		self.sculpter.write_glyph_atlas(atlas)
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
	let ws = max_width / width;
	let hs = max_height / height;
	ws.min(hs)
}
