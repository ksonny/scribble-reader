use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::MutexGuard;

use crate::Flags;
use crate::PixmapData;
use crate::PixmapDimensions;
use crate::PixmapFormat;
use crate::PixmapId;
use crate::PixmapOrigin;
use crate::PixmapRef;
use crate::PixmapTexture;

pub(crate) trait PixelatorTextureSupport {
	fn device(&self) -> &wgpu::Device;
	fn queue(&self) -> &wgpu::Queue;
	fn lock_textures(&self) -> MutexGuard<'_, BTreeMap<PixmapId, PixmapTexture>>;
}

#[derive(Debug, thiserror::Error)]
pub enum PixelatorPatchError {
	#[error("Patch target is deallocated")]
	TextureDeallocated,
	#[error("Patch target has different format than patch")]
	TextureFormatMissmatch,
	#[error("Patch position outside target texture")]
	PatchOutsideTexture,
}

#[allow(private_bounds)]
pub trait PixelatorTextures: PixelatorTextureSupport {
	#[must_use = "Output needs to be saved or texture is deallocated"]
	fn create(&self, dims: PixmapDimensions, data: PixmapData) -> PixmapRef {
		let (texture, flags) = upload_texture(self.device(), self.queue(), &dims, data);
		let pixmap_id = PixmapId::take();
		let pixmap: PixmapRef = pixmap_id.into();
		let weak_ref = Arc::downgrade(&pixmap.0);

		self.lock_textures().insert(
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

	#[must_use = "Output needs to be saved or texture is deallocated"]
	fn create_empty(&self, dims: PixmapDimensions, format: PixmapFormat) -> PixmapRef {
		let (format, flags) = match format {
			PixmapFormat::RgbA => (wgpu::TextureFormat::Rgba8Unorm, Flags::empty()),
			PixmapFormat::Luma => (wgpu::TextureFormat::R8Unorm, Flags::GRAYSCALE),
		};
		let size = wgpu::Extent3d {
			width: dims.width(),
			height: dims.height(),
			depth_or_array_layers: 1,
		};
		let texture = self.device().create_texture(&wgpu::TextureDescriptor {
			label: Some("pixmap texture"),
			size,
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format,
			usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
			view_formats: &[],
		});

		let pixmap_id = PixmapId::take();
		let pixmap: PixmapRef = pixmap_id.into();
		let weak_ref = Arc::downgrade(&pixmap.0);

		self.lock_textures().insert(
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

	#[must_use = "Output needs to be saved or texture is deallocated"]
	fn update(&self, pixmap: PixmapRef, dims: PixmapDimensions, data: PixmapData) -> PixmapRef {
		let format = match data {
			PixmapData::RgbA(_) => wgpu::TextureFormat::Rgba8UnormSrgb,
			PixmapData::Luma(_) => wgpu::TextureFormat::R8Unorm,
		};
		let mut textures = self.lock_textures();
		if let Some(entry) = textures.get_mut(pixmap.as_ref())
			&& entry.pixmap_dim == dims
			&& entry.texture.format() == format
		{
			// Match, write new data to texture
			let (bytes, flags, data) = match data {
				PixmapData::RgbA(data) => (4, Flags::empty(), data),
				PixmapData::Luma(data) => (1, Flags::GRAYSCALE, data),
			};
			let size = wgpu::Extent3d {
				width: dims.width(),
				height: dims.height(),
				depth_or_array_layers: 1,
			};
			self.queue().write_texture(
				wgpu::TexelCopyTextureInfo {
					texture: &entry.texture,
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
			entry.flags = flags;
			pixmap
		} else {
			// No match found or different parameters, allocate new texture
			let pixmap_id = PixmapId::take();

			let pixmap: PixmapRef = pixmap_id.into();
			let weak_ref = Arc::downgrade(&pixmap.0);

			let (texture, flags) = upload_texture(self.device(), self.queue(), &dims, data);
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

	fn patch(
		&self,
		pixmap: PixmapRef,
		origin: PixmapOrigin,
		dims: PixmapDimensions,
		data: PixmapData,
	) -> Result<(), PixelatorPatchError> {
		let format = match data {
			PixmapData::RgbA(_) => wgpu::TextureFormat::Rgba8UnormSrgb,
			PixmapData::Luma(_) => wgpu::TextureFormat::R8Unorm,
		};
		let mut textures = self.lock_textures();
		let Some(entry) = textures.get_mut(pixmap.as_ref()) else {
			return Err(PixelatorPatchError::TextureDeallocated);
		};
		if entry.texture.format() == format {
			return Err(PixelatorPatchError::TextureFormatMissmatch);
		}
		if entry.pixmap_dim.width() < origin.x() + dims.width()
			|| entry.pixmap_dim.height() < origin.y() + dims.height()
		{
			return Err(PixelatorPatchError::PatchOutsideTexture);
		}

		let (bytes, data) = match data {
			PixmapData::RgbA(data) => (4, data),
			PixmapData::Luma(data) => (1, data),
		};
		let size = wgpu::Extent3d {
			width: dims.width(),
			height: dims.height(),
			depth_or_array_layers: 1,
		};
		self.queue().write_texture(
			wgpu::TexelCopyTextureInfo {
				texture: &entry.texture,
				mip_level: 0,
				origin: wgpu::Origin3d {
					x: origin.x(),
					y: origin.y(),
					z: 0,
				},
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
		Ok(())
	}
}

fn upload_texture(
	device: &wgpu::Device,
	queue: &wgpu::Queue,
	dims: &PixmapDimensions,
	data: PixmapData<'_>,
) -> (wgpu::Texture, Flags) {
	let (format, bytes, flags, data) = match data {
		PixmapData::RgbA(data) => (wgpu::TextureFormat::Rgba8Unorm, 4, Flags::empty(), data),
		PixmapData::Luma(data) => (wgpu::TextureFormat::R8Unorm, 1, Flags::GRAYSCALE, data),
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
