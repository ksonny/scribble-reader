use resvg::usvg;

use std::fmt;
use std::fmt::Write;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::io;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use zip::ZipArchive;

use crate::html_parser;
use crate::html_parser::EdgeRef;
use crate::html_parser::TextWrapper;

pub static HORIZONTAL_RULER_SVG: LazyLock<SvgContent> = LazyLock::new(|| {
	let svg = r##"
<svg width="116.06" height="38.367" version="1.1" viewBox="0 0 30.707 10.151" xmlns="http://www.w3.org/2000/svg">
 <g transform="translate(-143.4 -8.1536)" stroke-width=".26458">
  <g transform="matrix(1.3673 0 0 1.3673 -58.313 -4.8594)">
   <path transform="matrix(1.0132 0 0 1.0132 2.5512 8.4914)" d="m154.17 8.34-1.146-2.5178-2.5178-1.146 2.5178-1.146 1.146-2.5178 1.146 2.5178 2.5178 1.146-2.5178 1.146z"/>
   <path transform="matrix(.28031 0 0 .28031 115.53 11.918)" d="m154.17 9.2082-1.146-3.386-3.386-1.146 3.386-1.146 1.146-3.386 1.146 3.386 3.386 1.146-3.386 1.146z" fill="#fff"/>
  </g>
  <g transform="translate(-11.642)">
   <path transform="matrix(1.0132 0 0 1.0132 2.5512 8.4914)" d="m154.17 8.34-1.146-2.5178-2.5178-1.146 2.5178-1.146 1.146-2.5178 1.146 2.5178 2.5178 1.146-2.5178 1.146zm22.981 0 1.146-2.5178 2.5178-1.146-2.5178-1.146-1.146-2.5178-1.146 2.5178-2.5178 1.146 2.5178 1.146z"/>
   <path transform="matrix(.28031 0 0 .28031 115.53 11.918)" d="m154.17 9.2082-1.146-3.386-3.386-1.146 3.386-1.146 1.146-3.386 1.146 3.386 3.386 1.146-3.386 1.146zm83.063 0 1.146-3.386 3.386-1.146-3.386-1.146-1.146-3.386-1.146 3.386-3.386 1.146 3.386 1.146z" fill="#fff"/>
  </g>
 </g>
</svg>
	"##;

	let hash = {
		let mut s = DefaultHasher::new();
		svg.hash(&mut s);
		s.finish()
	};
	SvgContent {
		hash,
		tree: usvg::Tree::from_str(svg, &usvg::Options::default()).unwrap(),
	}
});

#[derive(Debug, thiserror::Error)]
pub enum IllustratorSvgError {
	#[error(transparent)]
	Write(#[from] fmt::Error),
}

#[derive(Clone)]
pub(crate) struct SvgContent {
	#[allow(unused)]
	pub(crate) hash: u64,
	pub(crate) tree: usvg::Tree,
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

pub(crate) fn read_svg<'a>(
	buf: &'a mut String,
	el: &html_parser::ElementWrapper<'_>,
	node_iter: &mut html_parser::NodeTreeIter<'_>,
) -> Result<&'a str, IllustratorSvgError> {
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

	Ok(buf.as_str())
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
