#![allow(dead_code)]
use fixed::types::I26F6;
use image::DynamicImage;

pub struct Reader {
	dpi: I26F6,
	pixels: DynamicImage,
}
