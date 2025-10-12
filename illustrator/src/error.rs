use crate::html_parser;

#[derive(Debug, thiserror::Error)]
pub enum IllustratorError {
	#[error(transparent)]
	RecordKeeper(#[from] scribe::record_keeper::RecordKeeperError),
	#[error(transparent)]
	TreeBuilder(#[from] html_parser::TreeBuilderError),
	#[error(transparent)]
	Epub(#[from] epub::doc::DocError),
	#[error(transparent)]
	Zip(#[from] zip::result::ZipError),
	#[error(transparent)]
	Render(#[from] IllustratorRenderError),
	#[error("at {1}: {0}")]
	Io(std::io::Error, &'static std::panic::Location<'static>),
	#[error("Spineless book: {0}")]
	SpinelessBook(crate::library::Location),
	#[error("Missing resource {0}")]
	MissingResource(String),
	#[error("Impossible missing cache")]
	ImpossibleMissingCache,
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
	#[error("No text buffer for node {0:?}")]
	NoTextBuffer(taffy::NodeId),
	#[error("Missing body element")]
	MissingBodyElement,
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
