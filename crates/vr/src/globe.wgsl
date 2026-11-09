struct Uniforms {
    vp: mat4x4<f32>, inv_vp: mat4x4<f32>, brief_vp: mat4x4<f32>,
    eye: vec4<f32>, viewport: vec4<f32>, marker: vec4<f32>, cursor: vec4<f32>,
    ocean_col: vec4<f32>, land_col: vec4<f32>, mount_col: vec4<f32>,
    grid_col: vec4<f32>, hot_col: vec4<f32>,
    radius: f32, show_graticule: u32, active_body: u32, pad: u32,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var earth: texture_2d<f32>;
@group(0) @binding(2) var earth_sampler: sampler;
@group(0) @binding(3) var brief: texture_2d<f32>;
@vertex fn vs_main(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    return vec4(p[id],0.,1.);
}
@fragment fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / u.viewport.xy;
    let ndc = vec4(uv * vec2(2.,-2.) + vec2(-1.,1.), 0.5, 1.);
    let point = u.inv_vp * ndc;
    let origin = u.eye.xyz;
    let dir = normalize(point.xyz / point.w - origin);
    let b = dot(origin,dir);
    let disc = b*b - dot(origin,origin) + 1.;
    if disc < 0. { return vec4(0.006,0.009,0.015,1.); }
    let root = sqrt(disc);
    let t = select(-b+root,-b-root,-b-root > 0.);
    if t <= 0. { return vec4(0.006,0.009,0.015,1.); }
    let hit = normalize(origin + dir*t);
    let geo = hit.zyx;
    let facing = max(dot(hit,-dir),0.);
    let lonlat = vec2(atan2(hit.x,hit.z)/(2.*PI)+0.5, 0.5-asin(clamp(hit.y,-1.,1.))/PI);
    let relief = textureSampleLevel(earth,earth_sampler,lonlat,0.).r;
    var rgb = shade_globe(geo,facing) + vec3(0.10,0.25,0.31)*relief*relief*(0.45+0.55*facing);
    let distance = length(hit - u.marker.xyz)*u.viewport.z;
    let ring = 1.-smoothstep(0.002,0.007,abs(distance-0.024));
    let dot_alpha = 1.-smoothstep(0.005,0.009,distance);
    let accent = select(vec3(1.,0.42,0.06),vec3(0.2,1.,0.55),u.marker.w > 0.5);
    rgb = mix(rgb,accent,max(dot_alpha,ring));
    let cursor = (1.-smoothstep(0.006,0.012,length(hit-u.cursor.xyz)*u.viewport.z)) * u.cursor.w;
    return vec4(mix(rgb,vec3(1.),cursor),1.);
}
struct LineVertex { @builtin(position) clip: vec4<f32>, @location(0) point: vec3<f32> }
@vertex fn vs_line(@location(0) point: vec3<f32>) -> LineVertex {
    var out: LineVertex;
    out.clip = u.vp * vec4(point,1.); out.point = point; return out;
}
@fragment fn fs_line(in: LineVertex) -> @location(0) vec4<f32> {
    if dot(in.point,u.eye.xyz-in.point) <= 0. { discard; }
    return vec4(0.35,0.85,0.72,1.);
}

struct BriefVertex { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_brief(@builtin(vertex_index) id: u32) -> BriefVertex {
    let corners = array<vec2<f32>,4>(vec2(0.,1.),vec2(1.,1.),vec2(0.,0.),vec2(1.,0.));
    let uv = corners[id];
    return BriefVertex(u.brief_vp * vec4(uv.x-0.5,0.5-uv.y,0.,1.),uv);
}
@fragment fn fs_brief(v: BriefVertex) -> @location(0) vec4<f32> {
    return textureSample(brief,earth_sampler,v.uv);
}
