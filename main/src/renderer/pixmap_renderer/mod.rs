use std::mem;
use std::num::NonZeroU64;

use wgpu::Device;
use wgpu::Queue;
use wgpu::RenderPass;
use wgpu::TextureFormat;
use wgpu::util::DeviceExt;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RenderError {}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
	screen_resolution: [u32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct PixmapTargetInput {
	pub(crate) pos: [u32; 2],
	pub(crate) dim: [u32; 2],
	pub(crate) tex_pos: [u32; 2],
	pub(crate) tex_dim: [u32; 2],
}

#[derive(Debug)]
pub(crate) struct PixmapInput<'a> {
	pub(crate) pixmap_rgba: &'a [u8],
	pub(crate) pixmap_width: u32,
	pub(crate) pixmap_height: u32,
	pub(crate) targets: Vec<PixmapTargetInput>,
}

#[allow(unused)]
struct PixmapEntry {
	texture: wgpu::Texture,
	bind_group: wgpu::BindGroup,
	instance_buffer: wgpu::Buffer,
	instances: u32,
}

pub(crate) struct Renderer {
	sampler: wgpu::Sampler,
	params_buffer: wgpu::Buffer,
	bind_group_layout: wgpu::BindGroupLayout,
	pipeline: wgpu::RenderPipeline,
	params: Option<Params>,
	entries: Vec<PixmapEntry>,
}

impl Renderer {
	pub(crate) fn new(device: &Device, format: TextureFormat) -> Self {
		let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("pixmap sampler"),
			address_mode_u: wgpu::AddressMode::ClampToEdge,
			address_mode_v: wgpu::AddressMode::ClampToEdge,
			address_mode_w: wgpu::AddressMode::ClampToEdge,
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Nearest,
			mipmap_filter: wgpu::FilterMode::Nearest,
			..Default::default()
		});

		let params = Params {
			screen_resolution: [800, 600],
		};
		let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
			label: Some("pixmap params buffer"),
			contents: bytemuck::cast_slice(&[params]),
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
			bind_group_layouts: &[&bind_group_layout],
			push_constant_ranges: &[],
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
			multisample: wgpu::MultisampleState {
				count: 1,
				mask: !0,
				alpha_to_coverage_enabled: false,
			},
			multiview: None,
			cache: None,
		});

		Self {
			sampler,
			params_buffer,
			bind_group_layout,
			pipeline,
			params: None,
			entries: Vec::new(),
		}
	}

	fn vertex_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
		wgpu::VertexBufferLayout {
			array_stride: mem::size_of::<PixmapTargetInput>() as u64,
			step_mode: wgpu::VertexStepMode::Instance,
			attributes: &[
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Uint32x2,
					offset: 0,
					shader_location: 0,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Uint32x2,
					offset: mem::size_of::<[u32; 2]>() as wgpu::BufferAddress,
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
		self.params = Some(Params {
			screen_resolution: [width, height],
		});
	}

	pub(crate) fn prepare<'a>(
		&mut self,
		device: &Device,
		queue: &Queue,
		inputs: impl Iterator<Item = PixmapInput<'a>>,
	) {
		if let Some(params) = self.params {
			queue.write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&[params]));
		}

		// TODO: Reuse textures
		self.entries.clear();
		for input in inputs {
			let size = wgpu::Extent3d {
				width: input.pixmap_width,
				height: input.pixmap_height,
				depth_or_array_layers: 1,
			};
			let texture = device.create_texture(&wgpu::TextureDescriptor {
				label: Some("pixmap texture"),
				size,
				mip_level_count: 1,
				sample_count: 1,
				dimension: wgpu::TextureDimension::D2,
				format: wgpu::TextureFormat::Rgba8Unorm,
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
				input.pixmap_rgba,
				wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(4 * input.pixmap_width),
					rows_per_image: Some(input.pixmap_height),
				},
				size,
			);
			let texture_view = texture.create_view(&wgpu::wgt::TextureViewDescriptor::default());
			let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
				label: Some("pixmap bind group"),
				layout: &self.bind_group_layout,
				entries: &[
					wgpu::BindGroupEntry {
						binding: 0,
						resource: self.params_buffer.as_entire_binding(),
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

			let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
				label: Some("pixmap instance buffer"),
				contents: bytemuck::cast_slice(input.targets.as_slice()),
				usage: wgpu::BufferUsages::VERTEX,
			});
			let instances = input.targets.len() as u32;

			self.entries.push(PixmapEntry {
				texture,
				bind_group,
				instance_buffer,
				instances,
			});
		}
	}

	pub(crate) fn render(
		&self,
		rpass: &mut RenderPass<'_>,
	) -> std::result::Result<(), RenderError> {
		if !self.entries.is_empty() {
			rpass.set_pipeline(&self.pipeline);
			for entry in &self.entries {
				rpass.set_bind_group(0, &entry.bind_group, &[]);
				rpass.set_vertex_buffer(0, entry.instance_buffer.slice(..));
				rpass.draw(0..4, 0..entry.instances);
			}
		}
		Ok(())
	}
}
