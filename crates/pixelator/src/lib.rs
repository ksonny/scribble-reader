mod renderer;
mod textures;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::Weak;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use bitflags::bitflags;

pub use crate::renderer::PixmapBrush;
pub use crate::renderer::Renderer;
pub use crate::textures::PixelatorPatchError;
use crate::textures::PixelatorTextureSupport;
pub use crate::textures::PixelatorTextures;

bitflags! {
	#[repr(C)]
	#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
	struct Flags: u32 {
		const GRAYSCALE = 0b00000001;
	}
}

#[derive(Debug)]
pub struct PixmapTexture {
	weak_ref: Weak<PixmapId>,
	pixmap_dim: PixmapDimensions,
	flags: Flags,
	texture: wgpu::Texture,
}

impl PixmapTexture {
	pub fn texture(&self) -> &wgpu::Texture {
		&self.texture
	}

	pub fn pixmap_dims(&self) -> &PixmapDimensions {
		&self.pixmap_dim
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PixmapId(u64);

impl PixmapId {
	fn take() -> PixmapId {
		static COUNTER: AtomicU64 = AtomicU64::new(0);
		Self(COUNTER.fetch_add(1, Ordering::AcqRel))
	}
}

#[derive(Debug)]
pub enum PixmapFormat {
	RgbA,
	Luma,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixmapRef(Arc<PixmapId>);

impl PixmapRef {
	pub fn downgrade(&self) -> Weak<PixmapId> {
		let Self(inner) = self;
		std::sync::Arc::downgrade(inner)
	}
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
pub struct PixmapOrigin([u32; 2]);

impl PixmapOrigin {
	pub(crate) fn x(&self) -> u32 {
		self.0[0]
	}

	pub(crate) fn y(&self) -> u32 {
		self.0[1]
	}
}

impl From<[u32; 2]> for PixmapOrigin {
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

#[derive(Clone)]
pub struct PixelatorAssistant {
	device: wgpu::Device,
	queue: wgpu::Queue,
	textures: Arc<Mutex<BTreeMap<PixmapId, PixmapTexture>>>,
}

impl PixelatorTextureSupport for PixelatorAssistant {
	fn device(&self) -> &wgpu::Device {
		&self.device
	}

	fn queue(&self) -> &wgpu::Queue {
		&self.queue
	}

	fn lock_textures(&self) -> MutexGuard<'_, BTreeMap<PixmapId, PixmapTexture>> {
		self.textures.lock().unwrap()
	}
}

impl PixelatorTextures for PixelatorAssistant {}
