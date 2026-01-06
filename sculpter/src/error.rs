#[derive(Debug, thiserror::Error)]
pub enum SculpterError {
	#[error(transparent)]
	FaceParsing(#[from] ttf_parser::FaceParsingError),
}
