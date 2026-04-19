use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::mem;
use std::num::NonZeroU64;
use std::ops::Range;
use std::sync::Arc;
use std::sync::Weak;

use wgpu::Buffer;
use wgpu::Device;
use wgpu::Queue;
use wgpu::RenderPass;
use wgpu::Texture;
use wgpu::TextureFormat;
use wgpu::util::DeviceExt;

use crate::Flags;
use crate::PaintPosition;
use crate::PixmapData;
use crate::PixmapDimensions;
use crate::PixmapId;
use crate::PixmapInstance;
use crate::PixmapRef;
use crate::upload_texture;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
	screen_resolution: [u32; 2],
	offset_pos: [f32; 2],
	flags: Flags,
	_unused: u32,
}

#[derive(Debug)]
struct PixmapTexture {
	weak_ref: Weak<PixmapId>,
	pixmap_dim: PixmapDimensions,
	flags: Flags,
	texture: Texture,
}

struct PixmapBatch {
	bind_group: wgpu::BindGroup,
	params_buffer: wgpu::Buffer,
	range: Range<u32>,
}

pub struct Renderer {
	device: Device,
	queue: Queue,

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
	pub fn new(
		device: Device,
		queue: Queue,
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
			device,
			queue,

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

	pub fn resize(&mut self, width: u32, height: u32) {
		self.screen_width = width;
		self.screen_height = height;
	}

	pub fn render(&self, rpass: &mut RenderPass<'_>) {
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

	pub fn prepare(&mut self, mut run_brush: impl FnMut(&mut PixmapBrush<'_>)) {
		// Get new content to work with
		self.spans.clear();
		self.vertices.clear();
		let mut brush = PixmapBrush {
			device: &self.device,
			queue: &self.queue,
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
			self.queue.write_buffer(buffer, 0, contents);
		} else {
			let buffer = self
				.device
				.create_buffer_init(&wgpu::util::BufferInitDescriptor {
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
			let Some(entry) = self.textures.get(&span.pixmap) else {
				log::warn!("Span pointing at non-existing pixmap: {:?}", span.pixmap);
				continue;
			};
			pixmap_id_set.remove(&span.pixmap);

			let params = Params {
				screen_resolution: [self.screen_width, self.screen_height],
				offset_pos: span.pos.inner(),
				flags: entry.flags,
				_unused: 0,
			};
			let params_buffer = if let Some(buffer) = params_buffers.next() {
				self.queue
					.write_buffer(&buffer, 0, bytemuck::cast_slice(&[params]));
				buffer
			} else {
				self.device
					.create_buffer_init(&wgpu::util::BufferInitDescriptor {
						label: Some("pixmap params buffer"),
						contents: bytemuck::cast_slice(&[params]),
						usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
					})
			};
			let texture_view = entry
				.texture
				.create_view(&wgpu::wgt::TextureViewDescriptor::default());
			let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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

		// Drop unused and unreferenced textures
		for pixmap_id in pixmap_id_set {
			if self
				.textures
				.get(&pixmap_id)
				.is_some_and(|t| t.weak_ref.upgrade().is_none())
			{
				log::info!("Drop pixmap {:?}", pixmap_id);
				self.textures.remove(&pixmap_id);
			}
		}
	}
}

struct PixmapSpan {
	pixmap: PixmapId,
	pos: PaintPosition,
	range: Range<u32>,
}

pub struct PixmapBrush<'renderer> {
	device: &'renderer Device,
	queue: &'renderer Queue,

	id_counter: &'renderer mut u64,
	textures: &'renderer mut BTreeMap<PixmapId, PixmapTexture>,
	spans: &'renderer mut Vec<PixmapSpan>,
	instances: &'renderer mut Vec<PixmapInstance>,
}

impl PixmapBrush<'_> {
	#[must_use = "Output needs to be saved or texture is deallocated"]
	pub fn create(&mut self, dims: PixmapDimensions, data: PixmapData) -> PixmapRef {
		let pixmap_id = PixmapId(*self.id_counter);
		*self.id_counter += 1;

		let (texture, flags) = upload_texture(self.device, self.queue, &dims, data);

		let pixmap: PixmapRef = pixmap_id.into();
		let weak_ref = Arc::downgrade(&pixmap.0);

		self.textures.insert(
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
	pub fn update(
		&mut self,
		pixmap: PixmapRef,
		dims: PixmapDimensions,
		data: PixmapData,
	) -> PixmapRef {
		let format = match data {
			PixmapData::RgbA(_) => TextureFormat::Rgba8Unorm,
			PixmapData::Luma(_) => TextureFormat::R8Unorm,
		};
		if let Some(entry) = self.textures.get_mut(pixmap.as_ref())
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
			pixmap
		} else {
			// No match found or different parameters, allocate new texture
			let pixmap_id = PixmapId(*self.id_counter);
			*self.id_counter += 1;

			let pixmap: PixmapRef = pixmap_id.into();
			let weak_ref = Arc::downgrade(&pixmap.0);

			let (texture, flags) = upload_texture(self.device, self.queue, &dims, data);
			self.textures.insert(
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

	pub fn draw(
		&mut self,
		pixmap: &PixmapRef,
		pos: PaintPosition,
		instances: impl IntoIterator<Item = PixmapInstance>,
	) {
		let start = self.instances.len() as u32;
		self.instances.extend(instances);
		let end = self.instances.len() as u32;

		self.spans.push(PixmapSpan {
			pixmap: *pixmap.as_ref(),
			pos,
			range: start..end,
		});
	}
}
