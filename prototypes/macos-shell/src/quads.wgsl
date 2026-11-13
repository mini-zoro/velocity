struct Screen { size: vec2<f32>, _padding: vec2<f32> }
@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;
@group(0) @binding(2) var<uniform> screen: Screen;
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) clip: vec4<f32>,
}
@vertex fn vs_main(@builtin(vertex_index) vertex: u32,
    @location(0) rect: vec4<f32>, @location(1) uv_rect: vec4<f32>,
    @location(2) color: vec4<f32>, @location(3) clip: vec4<f32>) -> Out {
    let corners = array<vec2<f32>, 6>(vec2(0.,0.), vec2(1.,0.), vec2(0.,1.), vec2(0.,1.), vec2(1.,0.), vec2(1.,1.));
    let corner = corners[vertex];
    let pixel = rect.xy + corner * rect.zw;
    var out: Out;
    out.position = vec4(pixel / screen.size * vec2(2., -2.) + vec2(-1., 1.), 0., 1.);
    out.uv = uv_rect.xy + corner * uv_rect.zw;
    out.color = color;
    out.clip = clip;
    return out;
}
@fragment fn fs_main(input: Out) -> @location(0) vec4<f32> {
    if input.position.x < input.clip.x || input.position.y < input.clip.y || input.position.x >= input.clip.z || input.position.y >= input.clip.w { discard; }
    return textureSampleLevel(atlas, atlas_sampler, input.uv, 0.) * input.color;
}
