use resvg::usvg;

use std::fmt;
use std::fmt::Write;
use std::io;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use zip::ZipArchive;

use crate::html_parser;
use crate::html_parser::EdgeRef;
use crate::html_parser::TextWrapper;

#[derive(Debug, thiserror::Error)]
pub enum IllustratorSvgError {
	#[error(transparent)]
	Usvg(#[from] resvg::usvg::Error),
	#[error(transparent)]
	Write(#[from] fmt::Error),
}

pub(crate) struct SvgRender {
	pub(crate) scale: f32,
	pub(crate) svg: usvg::Tree,
}

pub(crate) fn svg_options<'a, R: io::Seek + io::Read + Sync + Send>(
	archive: Mutex<&'a mut ZipArchive<R>>,
	base_path: &'a Path,
) -> usvg::Options<'a> {
	usvg::Options {
		image_href_resolver: usvg::ImageHrefResolver {
			resolve_string: Box::new(move |href: &str, opts: &usvg::Options| {
				let path = Path::new(href);
				let data = {
					let mut archive = archive.lock().unwrap();
					let file = if path.is_absolute() {
						archive.by_path(path)
					} else {
						archive.by_path(base_path.join(path))
					};
					match file {
						Ok(mut f) => {
							let mut content = Vec::new();
							match f.read_to_end(&mut content) {
								Ok(_) => Some(content),
								Err(e) => {
									log::warn!("Failed to load '{href}': {e}");
									None
								}
							}
						}
						Err(e) => {
							log::warn!("Failed to load '{href}': {e}");
							None
						}
					}
				};

				if let Some(data) = data {
					let ext = path.extension().and_then(|e| e.to_str())?.to_lowercase();
					if ext == "svg" || ext == "svgz" {
						loab_sub_svg(data.as_slice(), opts)
					} else {
						match imagesize::image_type(&data) {
							Ok(imagesize::ImageType::Gif) => {
								Some(usvg::ImageKind::GIF(Arc::new(data)))
							}
							Ok(imagesize::ImageType::Png) => {
								Some(usvg::ImageKind::PNG(Arc::new(data)))
							}
							Ok(imagesize::ImageType::Jpeg) => {
								Some(usvg::ImageKind::JPEG(Arc::new(data)))
							}
							Ok(imagesize::ImageType::Webp) => {
								Some(usvg::ImageKind::WEBP(Arc::new(data)))
							}
							Ok(image_type) => {
								log::warn!("unknown image type of '{href}': {image_type:?}");
								None
							}
							Err(e) => {
								log::warn!("error decoding image type of '{href}': {e}");
								None
							}
						}
					}
				} else {
					log::warn!("Not an image file '{href}'");
					None
				}
			}),
			..Default::default()
		},
		..Default::default()
	}
}

// Extracted from usvg/src/parser/image.rs and modified to fit
fn loab_sub_svg(data: &[u8], opts: &usvg::Options<'_>) -> Option<usvg::ImageKind> {
	let sub_opts = usvg::Options {
		resources_dir: None,
		dpi: opts.dpi,
		font_size: opts.font_size,
		shape_rendering: opts.shape_rendering,
		text_rendering: opts.text_rendering,
		image_rendering: opts.image_rendering,
		default_size: opts.default_size,
		// The referenced SVG image cannot have any 'image' elements by itself.
		// Not only recursive. Any. Don't know why.
		image_href_resolver: usvg::ImageHrefResolver {
			resolve_data: Box::new(|_, _, _| None),
			resolve_string: Box::new(|_, _| None),
		},
		..Default::default()
	};

	let tree = usvg::Tree::from_data(data, &sub_opts);
	let tree = match tree {
		Ok(tree) => tree,
		Err(e) => {
			log::warn!("Failed to load subsvg image: {e}");
			return None;
		}
	};

	Some(usvg::ImageKind::SVG(tree))
}

pub(crate) fn read_svg(
	buf: &mut String,
	el: &html_parser::ElementWrapper<'_>,
	node_iter: &mut html_parser::NodeTreeIter<'_>,
	options: &usvg::Options,
) -> Result<usvg::Tree, IllustratorSvgError> {
	buf.clear();
	write_begin_node(buf, el.el)?;
	for edge in node_iter.by_ref() {
		match edge {
			EdgeRef::CloseElement(id, _name) if id == el.id => {
				break;
			}
			EdgeRef::OpenElement(el) => write_begin_node(buf, el.el)?,
			EdgeRef::CloseElement(_id, name) => {
				write_end_node(buf, &name)?;
			}
			EdgeRef::Text(TextWrapper { t, .. }) => {
				write!(buf, "{}", t.t)?;
			}
		}
	}
	write_end_node(buf, el.name())?;
	log::debug!("Svg node '''\n{buf}\n'''");

	let svg = usvg::Tree::from_str(buf.as_str(), options)?;
	Ok(svg)
}

fn write_begin_node<W: fmt::Write>(w: &mut W, el: &html_parser::Element) -> Result<(), fmt::Error> {
	write!(w, "<")?;
	if let Some(prefix) = &el.name.prefix
		&& !prefix.is_empty()
	{
		write!(w, "{}:", prefix)?;
	}
	write!(w, "{}", el.name.local)?;

	for (name, value) in &el.attrs {
		write!(w, " ")?;
		if let Some(prefix) = &name.prefix {
			write!(w, "{}:", prefix)?;
		}
		write!(w, r#"{}="{}""#, name.local, value)?;
	}

	write!(w, ">")
}

fn write_end_node<W: fmt::Write>(w: &mut W, name: &html5ever::QualName) -> Result<(), fmt::Error> {
	write!(w, "</")?;
	if let Some(prefix) = &name.prefix
		&& !prefix.is_empty()
	{
		write!(w, "{}:", prefix)?;
	}
	write!(w, "{}", name.local)?;
	write!(w, ">")
}
