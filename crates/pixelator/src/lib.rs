mod renderer;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use bitflags::bitflags;
use wgpu::Device;
use wgpu::Extent3d;
use wgpu::Origin3d;
use wgpu::Queue;
use wgpu::TexelCopyBufferLayout;
use wgpu::TexelCopyTextureInfo;
use wgpu::Texture;
use wgpu::TextureAspect;
use wgpu::TextureFormat;

pub use crate::renderer::PixmapBrush;
pub use crate::renderer::Renderer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PixmapId(u64);

impl PixmapId {
	fn take() -> PixmapId {
		static COUNTER: AtomicU64 = AtomicU64::new(0);
		Self(COUNTER.fetch_add(1, Ordering::AcqRel))
	}
}

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

#[derive(Debug)]
struct PixmapTexture {
	weak_ref: Weak<PixmapId>,
	pixmap_dim: PixmapDimensions,
	flags: Flags,
	texture: Texture,
}

#[derive(Clone)]
pub struct PixelatorAssistant {
	device: Device,
	queue: Queue,
	textures: Arc<Mutex<BTreeMap<PixmapId, PixmapTexture>>>,
}

pub trait PixelatorTextures {
	#[must_use = "Output needs to be saved or texture is deallocated"]
	fn create(&self, dims: PixmapDimensions, data: PixmapData) -> PixmapRef;

	#[must_use = "Output needs to be saved or texture is deallocated"]
	fn update(&self, pixmap: PixmapRef, dims: PixmapDimensions, data: PixmapData) -> PixmapRef;
}

impl PixelatorTextures for PixelatorAssistant {
	fn create(&self, dims: PixmapDimensions, data: PixmapData) -> PixmapRef {
		let (texture, flags) = upload_texture(&self.device, &self.queue, &dims, data);
		let pixmap_id = PixmapId::take();
		let pixmap: PixmapRef = pixmap_id.into();
		let weak_ref = Arc::downgrade(&pixmap.0);

		self.textures.lock().unwrap().insert(
			pixmap_id,
			PixmapTexture {
				weak_ref,
				pixmap_dim: dims,
				flags,
				texture,
			},
		);

		pixmap
	}

	fn update(&self, pixmap: PixmapRef, dims: PixmapDimensions, data: PixmapData) -> PixmapRef {
		let format = match data {
			PixmapData::RgbA(_) => TextureFormat::Rgba8Unorm,
			PixmapData::Luma(_) => TextureFormat::R8Unorm,
		};
		let mut textures = self.textures.lock().unwrap();
		if let Some(entry) = textures.get_mut(pixmap.as_ref())
			&& entry.pixmap_dim == dims
			&& entry.texture.format() == format
		{
			// Match, write new data to texture
			let (bytes, flags, data) = match data {
				PixmapData::RgbA(data) => (4, Flags::empty(), data),
				PixmapData::Luma(data) => (1, Flags::GRAYSCALE, data),
			};
			let size = Extent3d {
				width: dims.width(),
				height: dims.height(),
				depth_or_array_layers: 1,
			};
			self.queue.write_texture(
				TexelCopyTextureInfo {
					texture: &entry.texture,
					mip_level: 0,
					origin: Origin3d::ZERO,
					aspect: TextureAspect::All,
				},
				data,
				TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(bytes * dims.width()),
					rows_per_image: Some(dims.height()),
				},
				size,
			);
			entry.flags = flags;
			pixmap
		} else {
			// No match found or different parameters, allocate new texture
			let pixmap_id = PixmapId::take();

			let pixmap: PixmapRef = pixmap_id.into();
			let weak_ref = Arc::downgrade(&pixmap.0);

			let (texture, flags) = upload_texture(&self.device, &self.queue, &dims, data);
			textures.insert(
				pixmap_id,
				PixmapTexture {
					weak_ref,
					pixmap_dim: dims,
					texture,
					flags,
				},
			);
			pixmap
		}
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
