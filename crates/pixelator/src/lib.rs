mod renderer;

use std::sync::Arc;

use bitflags::bitflags;
use wgpu::Device;
use wgpu::Queue;
use wgpu::Texture;
use wgpu::TextureFormat;

pub use crate::renderer::PixmapBrush;
pub use crate::renderer::Renderer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PixmapId(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixmapRef(Arc<PixmapId>);

#[derive(Debug)]
pub enum PixmapData<'data> {
	RgbA(&'data [u8]),
	Luma(&'data [u8]),
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PixmapInstance {
	pub pos: [f32; 2],
	pub dim: [f32; 2],
	pub uv_pos: [u32; 2],
	pub uv_dim: [u32; 2],
}

impl From<PixmapId> for PixmapRef {
	fn from(value: PixmapId) -> Self {
		Self(Arc::new(value))
	}
}

impl AsRef<PixmapId> for PixmapRef {
	fn as_ref(&self) -> &PixmapId {
		self.0.as_ref()
	}
}

#[derive(Debug, PartialEq, Eq)]
pub struct PixmapDimensions([u32; 2]);

impl PixmapDimensions {
	pub fn width(&self) -> u32 {
		self.0[0]
	}

	pub fn height(&self) -> u32 {
		self.0[1]
	}
}

impl From<[u32; 2]> for PixmapDimensions {
	fn from(value: [u32; 2]) -> Self {
		Self(value)
	}
}

#[derive(Debug, Default)]
pub struct PaintPosition([f32; 2]);

impl PaintPosition {
	pub(crate) fn inner(&self) -> [f32; 2] {
		self.0
	}
}

impl From<[f32; 2]> for PaintPosition {
	fn from(value: [f32; 2]) -> Self {
		Self(value)
	}
}

bitflags! {
	#[repr(C)]
	#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
	struct Flags: u32 {
		const GRAYSCALE = 0b00000001;
	}
}

fn upload_texture(
	device: &Device,
	queue: &Queue,
	dims: &PixmapDimensions,
	data: PixmapData<'_>,
) -> (Texture, Flags) {
	let (format, bytes, flags, data) = match data {
		PixmapData::RgbA(data) => (TextureFormat::Rgba8Unorm, 4, Flags::empty(), data),
		PixmapData::Luma(data) => (TextureFormat::R8Unorm, 1, Flags::GRAYSCALE, data),
	};
	let size = wgpu::Extent3d {
		width: dims.width(),
		height: dims.height(),
		depth_or_array_layers: 1,
	};
	let texture = device.create_texture(&wgpu::TextureDescriptor {
		label: Some("pixmap texture"),
		size,
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format,
		usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
		view_formats: &[],
	});
	queue.write_texture(
		wgpu::TexelCopyTextureInfo {
			texture: &texture,
			mip_level: 0,
			origin: wgpu::Origin3d::ZERO,
			aspect: wgpu::TextureAspect::All,
		},
		data,
		wgpu::TexelCopyBufferLayout {
			offset: 0,
			bytes_per_row: Some(bytes * dims.width()),
			rows_per_image: Some(dims.height()),
		},
		size,
	);
	(texture, flags)
}
