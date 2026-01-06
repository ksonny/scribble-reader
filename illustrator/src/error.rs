use std::fmt;

use crate::html_parser;

#[derive(Debug, thiserror::Error)]
pub enum IllustratorError {
	#[error("record keeper error: {0}")]
	RecordKeeper(#[from] scribe::record_keeper::RecordKeeperError),
	#[error("tree builder error: {0}")]
	TreeBuilder(#[from] html_parser::TreeBuilderError),
	#[error("epub error: {0}")]
	Epub(#[from] epub::doc::DocError),
	#[error("zip error: {0}")]
	Zip(#[from] zip::result::ZipError),
	#[error("xml error: {0}")]
	QuickXml(#[from] quick_xml::de::DeError),
	#[error("render error: {0}")]
	Render(#[from] IllustratorRenderError),
	#[error("io error at {1}: {0}")]
	Io(std::io::Error, &'static std::panic::Location<'static>),
	#[error("config error: {0}")]
	Config(#[from] config::ConfigError),
	#[error(transparent)]
	SculpterCreate(#[from] sculpter::error::SculpterCreateError),
	#[error("Missing resource {0}")]
	MissingResource(String),
}

impl From<std::io::Error> for IllustratorError {
	#[track_caller]
	fn from(err: std::io::Error) -> Self {
		Self::Io(err, std::panic::Location::caller())
	}
}

#[derive(Debug, thiserror::Error)]
pub enum IllustratorRenderError {
	#[error(transparent)]
	TreeBuilder(#[from] html_parser::TreeBuilderError),
	#[error(transparent)]
	Zip(#[from] zip::result::ZipError),
	#[error(transparent)]
	Taffy(#[from] taffy::TaffyError),
	#[error(transparent)]
	Svg(#[from] IllustratorSvgError),
	#[error("Unexpected extra close")]
	UnexpectedExtraClose,
	#[error("Missing body")]
	MissingBody,
	#[error("Scale svg failed")]
	ScaleSvgFailed,
}

#[derive(Debug, thiserror::Error)]
pub enum IllustratorSvgError {
	#[error(transparent)]
	Usvg(#[from] resvg::usvg::Error),
	#[error(transparent)]
	Write(#[from] fmt::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum IllustratorRequestError {
	#[error("Illustrator not running")]
	NotRunning,
}

#[derive(Debug, thiserror::Error)]
pub enum IllustratorSpawnError {
	#[error(transparent)]
	RecordKeeper(#[from] scribe::record_keeper::RecordKeeperError),
}
