struct VertexInput {
	@builtin(vertex_index) idx: u32,
	@location(0) pos: vec2<u32>,
	@location(1) dim: vec2<u32>,
	@location(2) tex_pos: vec2<u32>,
	@location(3) tex_dim: vec2<u32>,
};

struct VertexOutput {
	@builtin(position) pos: vec4<f32>,
	@location(0) tex_pos: vec2<f32>,
};

struct Params {
	screen_resolution: vec2<u32>,
}

@group(0) @binding(0)
var<uniform> params: Params;
@group(0) @binding(1)
var atlas_s: sampler;
@group(0) @binding(2)
var atlas_t: texture_2d<f32>;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
	let corner = vec2<u32>(
		(in.idx >> 1u) & 1u,
		in.idx & 1u,
	);
	let corner_offset = in.dim * corner;
	let pos = in.pos + corner_offset;

	let tex_corner_offset = in.tex_dim * corner;
	let tex_pos = in.tex_pos + tex_corner_offset;

	let swap_y = vec2<f32>(1.0, -1.0);
	let screen_res = vec2<f32>(params.screen_resolution);

	var out: VertexOutput;
	out.pos = vec4<f32>(
		swap_y * (2.0 * vec2<f32>(pos) / screen_res - 1.0),
		0.0,
		1.0,
	);
	out.tex_pos = vec2<f32>(tex_pos) / vec2<f32>(textureDimensions(atlas_t));
	return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
	// return vec4<f32>(0.0, 0.0, 0.0, 1.0);
	return textureSample(atlas_t, atlas_s, in.tex_pos);
}
