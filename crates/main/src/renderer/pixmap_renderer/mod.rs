use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::mem;
use std::num::NonZeroU64;
use std::ops::Range;

use wgpu::Buffer;
use wgpu::Device;
use wgpu::Queue;
use wgpu::RenderPass;
use wgpu::Texture;
use wgpu::TextureFormat;
use wgpu::util::DeviceExt;

use bitflags::bitflags;

bitflags! {
	#[repr(C)]
	#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
	struct Flags: u32 {
		const GRAYSCALE = 0b00000001;
	}
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
	screen_resolution: [u32; 2],
	offset_pos: [f32; 2],
	flags: Flags,
	_unused: u32,
}

#[derive(Debug)]
pub(crate) enum PixmapData<'data> {
	RgbA(&'data [u8]),
	Luma(&'data [u8]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PixmapId(u64);

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PixmapDimensions([u32; 2]);

impl PixmapDimensions {
	pub(crate) fn width(&self) -> u32 {
		self.0[0]
	}

	pub(crate) fn height(&self) -> u32 {
		self.0[1]
	}
}

impl From<[u32; 2]> for PixmapDimensions {
	fn from(value: [u32; 2]) -> Self {
		Self(value)
	}
}

#[derive(Debug, Default)]
pub(crate) struct PaintPosition([f32; 2]);

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

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct PixmapInstance {
	pub(crate) pos: [f32; 2],
	pub(crate) dim: [f32; 2],
	pub(crate) uv_pos: [u32; 2],
	pub(crate) uv_dim: [u32; 2],
}

#[derive(Debug)]
pub(crate) struct PixmapTexture {
	pixmap_dim: PixmapDimensions,
	flags: Flags,
	texture: Texture,
}

#[derive(Debug)]
pub(crate) struct PixmapSpan {
	pixmap_id: PixmapId,
	pos: PaintPosition,
	range: Range<u32>,
}

pub(crate) struct PixmapBrush<'renderer> {
	device: &'renderer Device,
	queue: &'renderer Queue,

	id_counter: &'renderer mut u64,
	textures: &'renderer mut BTreeMap<PixmapId, PixmapTexture>,
	spans: &'renderer mut Vec<PixmapSpan>,
	instances: &'renderer mut Vec<PixmapInstance>,
}

impl PixmapBrush<'_> {
	pub(crate) fn create(&mut self, dims: PixmapDimensions, data: PixmapData) -> PixmapId {
		let pixmap_id = PixmapId(*self.id_counter);
		*self.id_counter += 1;

		let (texture, flags) = upload_texture(self.device, self.queue, &dims, data);

		self.textures.insert(
			pixmap_id,
			PixmapTexture {
				pixmap_dim: dims,
				flags,
				texture,
			},
		);

		pixmap_id
	}

	pub(crate) fn is_pixmap_active(&self, pixmap_id: PixmapId) -> bool {
		self.textures.contains_key(&pixmap_id)
	}

	#[must_use = "Output pixmap_id may be different and needs to be used in subsequent calls to draw!"]
	pub(crate) fn update(
		&mut self,
		pixmap_id: PixmapId,
		dims: PixmapDimensions,
		data: PixmapData,
	) -> PixmapId {
		let format = match data {
			PixmapData::RgbA(_) => TextureFormat::Rgba8Unorm,
			PixmapData::Luma(_) => TextureFormat::R8Unorm,
		};
		if let Some(entry) = self.textures.get_mut(&pixmap_id)
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
			self.queue.write_texture(
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
			pixmap_id
		} else {
			// No match found or different parameters, allocate new texture
			let pixmap_id = PixmapId(*self.id_counter);
			*self.id_counter += 1;

			let (texture, flags) = upload_texture(self.device, self.queue, &dims, data);
			self.textures.insert(
				pixmap_id,
				PixmapTexture {
					pixmap_dim: dims,
					texture,
					flags,
				},
			);
			pixmap_id
		}
	}

	pub(crate) fn draw(
		&mut self,
		pixmap_id: PixmapId,
		pos: PaintPosition,
		instances: impl IntoIterator<Item = PixmapInstance>,
	) {
		let start = self.instances.len() as u32;
		self.instances.extend(instances);
		let end = self.instances.len() as u32;

		self.spans.push(PixmapSpan {
			pixmap_id,
			pos,
			range: start..end,
		});
	}
}

struct PixmapBatch {
	bind_group: wgpu::BindGroup,
	params_buffer: wgpu::Buffer,
	range: Range<u32>,
}

pub(crate) struct Renderer {
	sampler: wgpu::Sampler,
	bind_group_layout: wgpu::BindGroupLayout,
	pipeline: wgpu::RenderPipeline,
	screen_width: u32,
	screen_height: u32,

	id_counter: u64,
	textures: BTreeMap<PixmapId, PixmapTexture>,
	spans: Vec<PixmapSpan>,
	vertices: Vec<PixmapInstance>,

	instance_buffer: Option<Buffer>,
	batches_active: Vec<PixmapBatch>,
	batches_inactive: Vec<PixmapBatch>,
}

impl Renderer {
	pub(crate) fn new(
		device: &Device,
		format: TextureFormat,
		screen_width: u32,
		screen_height: u32,
	) -> Self {
		let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("pixmap sampler"),
			address_mode_u: wgpu::AddressMode::ClampToEdge,
			address_mode_v: wgpu::AddressMode::ClampToEdge,
			address_mode_w: wgpu::AddressMode::ClampToEdge,
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Linear,
			..Default::default()
		});

		let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("pixmap texture bind group layout"),
			entries: &[
				wgpu::BindGroupLayoutEntry {
					binding: 0,
					visibility: wgpu::ShaderStages::VERTEX,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Uniform,
						has_dynamic_offset: false,
						min_binding_size: NonZeroU64::new(
							mem::size_of::<Params>() as wgpu::BufferAddress
						),
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 2,
					visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
					ty: wgpu::BindingType::Texture {
						multisampled: false,
						view_dimension: wgpu::TextureViewDimension::D2,
						sample_type: wgpu::TextureSampleType::Float { filterable: true },
					},
					count: None,
				},
			],
		});

		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("pixmap pipeline layout"),
			bind_group_layouts: &[Some(&bind_group_layout)],
			immediate_size: 0,
		});

		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("pixmap pipeline"),
			layout: Some(&pipeline_layout),
			vertex: wgpu::VertexState {
				module: &shader,
				entry_point: Some("vs_main"),
				buffers: &[Self::vertex_buffer_layout()],
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			},
			fragment: Some(wgpu::FragmentState {
				module: &shader,
				entry_point: Some("fs_main"),
				targets: &[Some(wgpu::ColorTargetState {
					format,
					blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
					write_mask: wgpu::ColorWrites::default(),
				})],
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			}),
			primitive: wgpu::PrimitiveState {
				topology: wgpu::PrimitiveTopology::TriangleStrip,
				strip_index_format: None,
				front_face: wgpu::FrontFace::Ccw,
				cull_mode: Some(wgpu::Face::Back),
				polygon_mode: wgpu::PolygonMode::Fill,
				unclipped_depth: false,
				conservative: false,
			},
			depth_stencil: None,
			multisample: wgpu::MultisampleState::default(),
			multiview_mask: None,
			cache: None,
		});

		Self {
			sampler,
			bind_group_layout,
			pipeline,
			screen_width,
			screen_height,

			id_counter: 0,
			textures: BTreeMap::new(),
			spans: Vec::new(),
			vertices: Vec::new(),

			instance_buffer: None,
			batches_active: Vec::new(),
			batches_inactive: Vec::new(),
		}
	}

	fn vertex_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
		wgpu::VertexBufferLayout {
			array_stride: mem::size_of::<PixmapInstance>() as u64,
			step_mode: wgpu::VertexStepMode::Instance,
			attributes: &[
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x2,
					offset: 0,
					shader_location: 0,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x2,
					offset: mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
					shader_location: 1,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Uint32x2,
					offset: mem::size_of::<[u32; 4]>() as wgpu::BufferAddress,
					shader_location: 2,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Uint32x2,
					offset: mem::size_of::<[u32; 6]>() as wgpu::BufferAddress,
					shader_location: 3,
				},
			],
		}
	}

	pub(crate) fn resize(&mut self, width: u32, height: u32) {
		self.screen_width = width;
		self.screen_height = height;
	}

	pub(crate) fn prepare(
		&mut self,
		device: &Device,
		queue: &Queue,
		mut run_brush: impl FnMut(&mut PixmapBrush<'_>),
	) {
		// Get new content to work with
		self.spans.clear();
		self.vertices.clear();
		let mut brush = PixmapBrush {
			device,
			queue,
			id_counter: &mut self.id_counter,
			textures: &mut self.textures,
			spans: &mut self.spans,
			instances: &mut self.vertices,
		};
		run_brush(&mut brush);

		// Upload vertices
		let contents = bytemuck::cast_slice(self.vertices.as_slice());
		if let Some(buffer) = &self.instance_buffer
			&& buffer.size() >= contents.len() as u64
		{
			queue.write_buffer(buffer, 0, contents);
		} else {
			let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
				label: Some("pixmap instance buffer"),
				contents,
				usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
			});
			self.instance_buffer = Some(buffer);
		}

		// Create batches of textures and instance ranges
		mem::swap(&mut self.batches_active, &mut self.batches_inactive);
		let mut pixmap_id_set = self.textures.keys().cloned().collect::<BTreeSet<_>>();
		let mut params_buffers = self.batches_inactive.drain(..).map(|e| e.params_buffer);
		for span in &self.spans {
			let Some(entry) = self.textures.get(&span.pixmap_id) else {
				log::warn!("Span pointing at non-existing pixmap: {:?}", span.pixmap_id);
				continue;
			};
			pixmap_id_set.remove(&span.pixmap_id);

			let params = Params {
				screen_resolution: [self.screen_width, self.screen_height],
				offset_pos: span.pos.inner(),
				flags: entry.flags,
				_unused: 0,
			};
			let params_buffer = if let Some(buffer) = params_buffers.next() {
				queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&[params]));
				buffer
			} else {
				device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
					label: Some("pixmap params buffer"),
					contents: bytemuck::cast_slice(&[params]),
					usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
				})
			};
			let texture_view = entry
				.texture
				.create_view(&wgpu::wgt::TextureViewDescriptor::default());
			let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
				label: Some("pixmap bind group"),
				layout: &self.bind_group_layout,
				entries: &[
					wgpu::BindGroupEntry {
						binding: 0,
						resource: params_buffer.as_entire_binding(),
					},
					wgpu::BindGroupEntry {
						binding: 1,
						resource: wgpu::BindingResource::Sampler(&self.sampler),
					},
					wgpu::BindGroupEntry {
						binding: 2,
						resource: wgpu::BindingResource::TextureView(&texture_view),
					},
				],
			});
			self.batches_active.push(PixmapBatch {
				bind_group,
				params_buffer,
				range: span.range.clone(),
			});
		}
		drop(params_buffers);

		// Drop unused textures
		for pixmap_id in pixmap_id_set {
			self.textures.remove(&pixmap_id);
		}
	}

	pub(crate) fn render(&self, rpass: &mut RenderPass<'_>) {
		if !self.batches_active.is_empty()
			&& let Some(instance_buffer) = &self.instance_buffer
		{
			rpass.set_pipeline(&self.pipeline);
			rpass.set_vertex_buffer(0, instance_buffer.slice(..));

			for batch in &self.batches_active {
				rpass.set_bind_group(0, &batch.bind_group, &[]);
				rpass.draw(0..4, batch.range.clone());
			}
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
