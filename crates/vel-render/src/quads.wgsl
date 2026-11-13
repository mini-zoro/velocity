struct Screen { size: vec2<f32>, padding: vec2<f32> }
@group(0) @binding(0) var atlas: texture_2d_array<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;
@group(0) @binding(2) var<uniform> screen: Screen;
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) clip: vec4<f32>,
    @location(3) local: vec2<f32>,
    @location(4) @interpolate(flat) size: vec2<f32>,
    @location(5) @interpolate(flat) params: vec4<f32>,
    @location(6) @interpolate(flat) primitive: vec4<f32>,
}
@vertex fn vs_main(@builtin(vertex_index) vertex: u32,
    @location(0) rect: vec4<f32>, @location(1) uv_rect: vec4<f32>,
    @location(2) color: vec4<f32>, @location(3) clip: vec4<f32>,
    @location(4) params: vec4<f32>, @location(5) primitive: vec4<f32>) -> Out {
    let corners = array<vec2<f32>,6>(vec2(0.,0.),vec2(1.,0.),vec2(0.,1.),vec2(0.,1.),vec2(1.,0.),vec2(1.,1.));
    let corner = corners[vertex];
    let pixel = rect.xy + corner * rect.zw;
    var out: Out;
    out.position = vec4(pixel / screen.size * vec2(2.,-2.) + vec2(-1.,1.),0.,1.);
    out.uv = uv_rect.xy + corner * uv_rect.zw;
    out.color = color; out.clip = clip; out.local = corner * rect.zw;
    out.size = rect.zw; out.params = params; out.primitive = primitive;
    return out;
}
fn rounded_distance(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2(radius);
    return length(max(q,vec2(0.))) + min(max(q.x,q.y),0.) - radius;
}
@fragment fn fs_main(input: Out) -> @location(0) vec4<f32> {
    if input.position.x < input.clip.x || input.position.y < input.clip.y || input.position.x >= input.clip.z || input.position.y >= input.clip.w { discard; }
    let kind = u32(input.primitive.x);
    if kind == 2u { return textureSampleLevel(atlas,atlas_sampler,input.uv,i32(input.primitive.y),0.) * input.color; }
    var coverage = 1.;
    if kind == 0u {
        let blur = max(input.params.y,0.5);
        let d = rounded_distance(input.local - input.size * 0.5,max(input.size * 0.5 - vec2(blur),vec2(0.)),input.params.x);
        coverage = 1. - smoothstep(-blur,blur,d);
    } else if kind == 1u && input.params.x > 0. {
        let radius = min(input.params.x,min(input.size.x,input.size.y) * 0.5);
        let d = rounded_distance(input.local - input.size * 0.5,input.size * 0.5,radius);
        coverage = 1. - smoothstep(-0.5,0.5,d);
    } else if kind == 3u {
        let center = input.size.y * 0.5;
        let wave = select(0.,sin(input.local.x * 1.5707963) * max(0.,center-input.params.z),input.params.w > 0.);
        coverage = 1. - smoothstep(max(0.,input.params.z*0.5-0.5),input.params.z*0.5+0.5,abs(input.local.y-center-wave));
    }
    return vec4(input.color.rgb,input.color.a * coverage);
}
