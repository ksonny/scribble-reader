#[derive(Debug, thiserror::Error)]
pub enum SculpterError {
	#[error(transparent)]
	FaceParsing(#[from] ttf_parser::FaceParsingError),
}

#[derive(Debug, thiserror::Error)]
pub enum SculpterShapeError {
	#[error("Unknown shape plan: {0:?}")]
	UnknownShapePlan(crate::shape::ShapeFaceRef),
}

#[derive(Debug, thiserror::Error)]
pub enum SculpterScaleError {
	#[error("Missing scale entry for shape plan: {0:?}")]
	MissingEntryForPlanRef(crate::shape::ShapeFaceRef),
}
