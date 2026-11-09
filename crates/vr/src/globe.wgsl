struct Uniforms {
    vp: mat4x4<f32>, inv_vp: mat4x4<f32>, brief_vp: mat4x4<f32>,
    wrist_vp: mat4x4<f32>, table_inverse: mat4x4<f32>, table_eye:vec4<f32>,
    source_projection:vec4<f32>, source_rotation:vec4<f32>, layers:vec4<f32>, wrist_cursor:vec4<f32>,
    eye: vec4<f32>, viewport: vec4<f32>, marker: vec4<f32>, cursor: vec4<f32>,
    ocean_col: vec4<f32>, land_col: vec4<f32>, mount_col: vec4<f32>,
    grid_col: vec4<f32>, hot_col: vec4<f32>,
    radius: f32, show_graticule: u32, active_body: u32, pad: u32,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var earth: texture_2d<f32>;
@group(0) @binding(2) var earth_sampler: sampler;
@group(0) @binding(3) var brief: texture_2d<f32>;
@group(0) @binding(4) var native_map: texture_2d_array<f32>;
@group(0) @binding(5) var wrist_panel: texture_2d_array<f32>;
@group(0) @binding(6) var height_field: texture_2d_array<f32>;
@group(0) @binding(7) var room_panel:texture_2d_array<f32>;
@vertex fn vs_main(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    return vec4(p[id],0.,1.);
}
@fragment fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / u.viewport.xy;
    let ndc = vec4(uv * vec2(2.,-2.) + vec2(-1.,1.), 0.5, 1.);
    if u.layers.z > 1.5 { return local_map(ndc); }
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
    if u.layers.z < 0.5 {
        rgb = mix(rgb,accent,max(dot_alpha,ring));
    } else {
        let r=u.source_rotation;
        let x=geo.x*r.x+geo.z*r.y;
        let z=-geo.x*r.y+geo.z*r.x;
        let y=geo.y*r.z-z*r.w;
        let depth=geo.y*r.w+z*r.z;
        let proj=u.source_projection;
        let map_uv=proj.xy-vec2(x,y)*proj.z/(proj.w-depth);
        if depth*proj.w>1. && all(map_uv>=vec2(0.)) && all(map_uv<=vec2(1.)) {
            rgb=map_color(map_uv);
        }
    }
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
    if u.layers.z>0.5 {return textureSampleLevel(room_panel,earth_sampler,v.uv,i32(u.wrist_cursor.w),0.);}
    return textureSampleLevel(brief,earth_sampler,v.uv,0.);
}

fn map_color(uv: vec2<f32>) -> vec3<f32> {
    let margin = 0.5 / vec2<f32>(textureDimensions(native_map));
    let inside = clamp(uv, margin, vec2(1.) - margin);
    return textureSampleLevel(native_map, earth_sampler, inside, i32(u.layers.x), 0.).rgb;
}

fn terrain_height(uv: vec2<f32>) -> f32 {
    let last = vec2<i32>(textureDimensions(height_field)) - vec2(1);
    let point = clamp(uv, vec2(0.), vec2(1.)) * vec2<f32>(last);
    let low = vec2<i32>(point);
    let high = min(low + vec2(1), last);
    let fraction = fract(point);
    let layer = i32(u.layers.x);
    let nw = textureLoad(height_field, low, layer, 0).r;
    let ne = textureLoad(height_field, vec2(high.x, low.y), layer, 0).r;
    let sw = textureLoad(height_field, vec2(low.x, high.y), layer, 0).r;
    let se = textureLoad(height_field, high, layer, 0).r;
    return mix(mix(nw, ne, fraction.x), mix(sw, se, fraction.x), fraction.y);
}

fn table_uv(point: vec3<f32>) -> vec2<f32> {
    return vec2(0.5 + point.x, 0.5 - point.y);
}

fn local_map(ndc: vec4<f32>) -> vec4<f32> {
    let background = vec4(0.006, 0.009, 0.015, 1.);
    let eye = u.table_eye.xyz;
    let through = u.table_inverse * ndc;
    let ray = normalize(through.xyz / through.w - eye);
    if ray.z >= -0.000001 { return background; }

    var uv = table_uv(eye + ray * (-eye.z / ray.z));
    if u.layers.w > 0.5 {
        // Clip to the table's volume before looking for its first elevation crossing.
        // Controller picking in cartography.rs uses the same height field and search.
        let extent = vec3(0.5, 0.5, 0.25);
        let safe_ray = select(ray, vec3(1e-8), abs(ray) < vec3(1e-8));
        let a = (-extent - eye) / safe_ray;
        let b = ( extent - eye) / safe_ray;
        let near = min(a, b);
        let far = max(a, b);
        let start = max(0., max(near.x, max(near.y, near.z)));
        let end = min(far.x, min(far.y, far.z));
        if start >= end { return background; }

        var low = start;
        var found = false;
        for (var i = 0; i <= 32; i++) {
            var high = mix(start, end, f32(i) / 32.);
            let point = eye + ray * high;
            if point.z <= terrain_height(table_uv(point)) {
                for (var j = 0; j < 5; j++) {
                    let t = (low + high) * 0.5;
                    let mid = eye + ray * t;
                    if mid.z > terrain_height(table_uv(mid)) {
                        low = t;
                    } else {
                        high = t;
                    }
                }
                uv = table_uv(eye + ray * high);
                found = true;
                break;
            }
            low = high;
        }
        if !found { return background; }
    }
    if any(uv < vec2(0.)) || any(uv > vec2(1.)) { return background; }
    let pointer = (1. - smoothstep(0.002, 0.006, distance(uv, u.cursor.xy))) * u.cursor.w;
    return vec4(mix(map_color(uv), vec3(1.), pointer), 1.);
}

@vertex fn vs_wrist(@builtin(vertex_index) id: u32) -> BriefVertex {
    let corners = array<vec2<f32>, 4>(vec2(0., 1.), vec2(1., 1.), vec2(0., 0.), vec2(1., 0.));
    let uv = corners[id];
    return BriefVertex(u.wrist_vp * vec4(uv.x - 0.5, 0.5 - uv.y, 0., 1.), uv);
}

@fragment fn fs_wrist(vertex: BriefVertex) -> @location(0) vec4<f32> {
    let color = textureSampleLevel(wrist_panel, earth_sampler, vertex.uv, i32(u.layers.y), 0.).rgb;
    let distance_to_cursor = distance(vertex.uv, u.wrist_cursor.xy);
    let ring = (1. - smoothstep(0.001, 0.003, abs(distance_to_cursor - 0.008))) * u.wrist_cursor.z;
    return vec4(mix(color, vec3(0.3, 1., 0.8), ring), 1.);
}
