// Pull the retained terrain image through the current camera onto its sphere.
// Uniform layout matches ContourUniforms (80 bytes) and Warp (176 bytes).
struct Camera {
    center:vec2<f32>, viewport_min:vec2<f32>, viewport_size:vec2<f32>,
    yaw_sin:f32, yaw_cos:f32, pitch_sin:f32, pitch_cos:f32,
    radius_focal:f32, camera_distance:f32, radius_offset:f32, alpha:f32,
    stroke_half_px:f32, feather_px:f32, pixels_per_point:f32, horizon_z:f32,
    pad:vec2<f32>,
}
struct Warp { current:Camera, saved:Camera, flags:vec4<f32> }
@group(0) @binding(0) var preview:texture_2d<f32>;
@group(0) @binding(1) var detail:texture_2d<f32>;
@group(0) @binding(2) var linear:sampler;
@group(0) @binding(3) var<uniform> w:Warp;
struct Out { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> }
@vertex fn vs(@builtin(vertex_index) i:u32)->Out {
    let uv=vec2<f32>(f32(i&1u),f32(i>>1u));
    var out:Out;out.position=vec4<f32>(uv.x*2.-1.,1.-uv.y*2.,0.,1.);out.uv=uv;return out;
}
fn saved_uv(uv:vec2<f32>)->vec3<f32> {
    let c=w.current; let s=w.saved;
    let logical=(uv*c.viewport_size+c.viewport_min)/c.pixels_per_point;
    let ray=vec3<f32>((c.center-logical)/c.radius_focal,-1.);
    let a=dot(ray,ray); let d=c.camera_distance; let r=1.+c.radius_offset;
    let discriminant=d*d-a*(d*d-r*r);
    if (discriminant<0.) {return vec3<f32>(0.,0.,-1.);}
    let t=(d-sqrt(discriminant))/a;
    let p=vec3<f32>(ray.xy*t,d-t);
    // Inverse current pitch, then yaw: camera -> geographic sphere.
    let y=p.y*c.pitch_cos+p.z*c.pitch_sin;
    let z=-p.y*c.pitch_sin+p.z*c.pitch_cos;
    let world=vec3<f32>(p.x*c.yaw_cos-z*c.yaw_sin,y,p.x*c.yaw_sin+z*c.yaw_cos);
    let x1=world.x*s.yaw_cos+world.z*s.yaw_sin;
    let z1=-world.x*s.yaw_sin+world.z*s.yaw_cos;
    let y2=world.y*s.pitch_cos-z1*s.pitch_sin;
    let z2=world.y*s.pitch_sin+z1*s.pitch_cos;
    let depth=s.camera_distance-z2;
    if(depth<=0.05 || z2<s.horizon_z) {return vec3<f32>(0.,0.,-1.);}
    let pixel=(s.center-vec2<f32>(x1,y2)*s.radius_focal/depth)*s.pixels_per_point;
    let old=(pixel-s.viewport_min)/s.viewport_size;
    if(any(old<vec2<f32>(0.)) || any(old>vec2<f32>(1.))) {return vec3<f32>(0.,0.,-1.);}
    return vec3<f32>(old,1.);
}
@fragment fn fs(in:Out)->@location(0) vec4<f32> {
    if(w.flags.x>0.) {
        if(w.flags.y>0.) {return textureSampleLevel(detail,linear,in.uv,0.);}
        let old=saved_uv(in.uv);
        // Empty pixels inside retained coverage stay empty: never overlay two
        // contour generations or let preview density erase retained planes.
        if(old.z>0.) {return textureSampleLevel(detail,linear,old.xy,0.);}
    }
    return textureSampleLevel(preview,linear,in.uv,0.);
}
