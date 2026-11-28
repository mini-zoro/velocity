struct Screen { size: vec2<f32>, padding: vec2<f32> }
// Glyph coverage (R8) and color glyph/image (RGBA) pages; primitive.w selects one.
@group(0) @binding(0) var atlas: texture_2d_array<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;
@group(0) @binding(2) var color_atlas: texture_2d_array<f32>;
@group(1) @binding(0) var<uniform> screen: Screen;
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) clip: vec4<f32>,
    @location(3) local: vec2<f32>,
    @location(4) @interpolate(flat) size: vec2<f32>,
    @location(5) @interpolate(flat) params: vec4<f32>,
    @location(6) @interpolate(flat) primitive: vec4<f32>,
    @location(7) @interpolate(flat) edges: vec4<f32>,
}
@vertex fn vs_main(@builtin(vertex_index) vertex: u32,
    @location(0) rect: vec4<f32>, @location(1) uv_rect: vec4<f32>,
    @location(2) color: vec4<f32>, @location(3) clip: vec4<f32>,
    @location(4) params: vec4<f32>, @location(5) primitive: vec4<f32>) -> Out {
    let corners = array<vec2<f32>,6>(vec2(0.,0.),vec2(1.,0.),vec2(0.,1.),vec2(0.,1.),vec2(1.,0.),vec2(1.,1.));
    let corner = corners[vertex];
    var pixel = rect.xy + corner * rect.zw;
    if u32(primitive.x) == 8u { // one filled triangle, then a degenerate triangle
        let vertices = array<vec2<f32>,6>(rect.xy,params.xy,params.zw,params.zw,params.xy,params.zw);
        pixel = vertices[vertex];
    }
    var out: Out;
    out.position = vec4(pixel / screen.size * vec2(2.,-2.) + vec2(-1.,1.),0.,1.);
    out.uv = uv_rect.xy + corner * uv_rect.zw;
    out.edges = uv_rect;
    out.color = color; out.clip = clip; out.local = corner * rect.zw;
    out.size = rect.zw; out.params = params; out.primitive = primitive;
    return out;
}
fn rounded_distance(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2(radius);
    return length(max(q,vec2(0.))) + min(max(q.x,q.y),0.) - radius;
}
fn corner_distance(p: vec2<f32>, size: vec2<f32>, radii: vec4<f32>) -> f32 {
    let top = p.y < size.y * 0.5;
    let left = p.x < size.x * 0.5;
    let radius = select(select(radii.z,radii.w,left),select(radii.y,radii.x,left),top);
    return rounded_distance(p - size * 0.5,size * 0.5,radius);
}
// A mask page stores coverage in .r; RGBA pages read as before. A mask texel equals
// the old white RGBA texel (1,1,1,coverage); its transparent border reads (1,1,1,0)
// instead of (0,0,0,0), which only matters between texel centers, where the old
// value darkened the glyph's colour.
fn atlas_sample(uv: vec2<f32>, page: i32, color: bool) -> vec4<f32> {
    if color { return textureSampleLevel(color_atlas,atlas_sampler,uv,page,0.); }
    return vec4(1.,1.,1.,textureSampleLevel(atlas,atlas_sampler,uv,page,0.).r);
}
fn glow_sample(uv: vec2<f32>, page: i32, bounds: vec4<f32>, color: bool) -> f32 {
    if any(uv < bounds.xy) || any(uv > bounds.zw) { return 0.; }
    return atlas_sample(uv,page,color).a;
}
@fragment fn fs_main(input: Out) -> @location(0) vec4<f32> {
    if input.position.x < input.clip.x || input.position.y < input.clip.y || input.position.x >= input.clip.z || input.position.y >= input.clip.w { discard; }
    let kind = u32(input.primitive.x);
    if kind == 8u { return input.color; }
    if kind == 7u {
        let a=input.params.xy; let b=input.params.zw;
        let delta=b-a;
        let t=clamp(dot(input.local-a,delta)/max(dot(delta,delta),0.0001),0.,1.);
        let distance=length(input.local-a-t*delta);
        let coverage=1.-smoothstep(max(0.,input.primitive.z-0.5),input.primitive.z+0.5,distance);
        return vec4(input.color.rgb,input.color.a*coverage);
    }
    if kind == 9u {
        var alpha = 0.;
        for (var y = -2; y <= 2; y++) { for (var x = -2; x <= 2; x++) {
            let weight = exp(-f32(x*x+y*y)*0.35);
            alpha += glow_sample(input.uv + vec2(f32(x),f32(y))*input.primitive.z,i32(input.primitive.y),input.params,input.primitive.w > 0.5)*weight/8.7;
        } }
        return vec4(input.color.rgb,input.color.a*alpha);
    }
    if kind == 6u {
        let edge = min(min(input.local.x, input.size.x-input.local.x), min(input.local.y, input.size.y-input.local.y));
        if edge >= input.params.x { discard; }
        return input.color;
    }
    if kind == 5u {
        // One stippled guide per quad. Period/on-length are in physical pixels.
        if (input.local.y % max(input.params.x, 1.)) >= input.params.y { discard; }
        return input.color;
    }
    if kind == 2u { return atlas_sample(input.uv,i32(input.primitive.y),input.primitive.w > 0.5) * input.color; }
    if kind == 4u {
        var coverage = 1. - smoothstep(-0.5,0.5,corner_distance(input.local,input.size,input.params));
        let side = u32(input.primitive.y);
        if side > 0u {
            let edges = input.edges; // top/right/bottom/left, in physical pixels
            let inner_size = max(input.size - vec2(edges.w+edges.y,edges.x+edges.z),vec2(0.));
            let inner_radii = max(input.params - vec4(max(edges.x,edges.w),max(edges.x,edges.y),max(edges.z,edges.y),max(edges.z,edges.w)),vec4(0.));
            let inside = select(0., 1. - smoothstep(-0.5,0.5,corner_distance(input.local-vec2(edges.w,edges.x),inner_size,inner_radii)),all(inner_size > vec2(0.)));
            coverage *= 1. - inside;
            let distances = vec4(input.local.y,input.size.x-input.local.x,input.size.y-input.local.y,input.local.x) / max(edges,vec4(0.0001));
            var selected = 0u;
            var minimum = distances.x;
            if distances.y < minimum { selected=1u; minimum=distances.y; }
            if distances.z < minimum { selected=2u; minimum=distances.z; }
            if distances.w < minimum { selected=3u; }
            if selected + 1u != side { discard; }
        }
        return vec4(input.color.rgb,input.color.a*coverage);
    }
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
        let wave = select(0.,sin(input.local.x * 1.5707963) * max(0.,center-input.params.z),input.params.w > 0. && input.params.w < 1.5);
        coverage = 1. - smoothstep(max(0.,input.params.z*0.5-0.5),input.params.z*0.5+0.5,abs(input.local.y-center-wave));
        if input.params.w > 1.5 { coverage *= 1. - smoothstep(0.7,1.2,abs((input.local.x % 4.) - 2.)); }
    }
    return vec4(input.color.rgb,input.color.a * coverage);
}
