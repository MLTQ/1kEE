struct Uniforms {
    viewport:vec4<f32>, region:vec4<f32>, a:vec4<f32>, b:vec4<f32>, c:vec4<f32>, d:vec4<f32>,
    globe:vec4<f32>, sphere:vec4<f32>, style:vec4<f32>, grid:vec4<f32>, extra:vec4<f32>,
    major:vec4<f32>, minor:vec4<f32>, exclude:vec4<f32>,
    light:vec4<f32>, appearance:vec4<f32>,
}
@group(0) @binding(0) var<uniform> u:Uniforms;
@group(1) @binding(0) var heights:texture_2d<f32>;
@group(1) @binding(1) var pigments:texture_2d<f32>;
@group(1) @binding(2) var linear_sampler:sampler;

struct Vertex {
    @builtin(position) clip:vec4<f32>,
    @location(0) uv:vec2<f32>,
    @location(1) elevation:f32,
    @location(2) geo:vec2<f32>,
}
fn safe_height(h:f32,fallback:f32)->f32 {
    if h>=-11000.0 && h<=9000.0 {return h;}
    return fallback;
}
fn height_at_or(coordinate:vec2<f32>,fallback:f32)->f32 {
    var uv=coordinate;
    if u.region.z-u.region.x>359.9 {uv.x=fract(uv.x);}
    let size=vec2<i32>(textureDimensions(heights));
    let p=clamp(uv*vec2<f32>(size)-0.5,vec2<f32>(0.0),vec2<f32>(size-1));
    let cell=vec2<i32>(floor(p));let next=min(cell+1,size-1);let t=fract(p);
    let a=safe_height(textureLoad(heights,cell,0).r,fallback);
    let b=safe_height(textureLoad(heights,vec2<i32>(next.x,cell.y),0).r,fallback);
    let c=safe_height(textureLoad(heights,vec2<i32>(cell.x,next.y),0).r,fallback);
    let d=safe_height(textureLoad(heights,next,0).r,fallback);
    return mix(mix(a,b,t.x),mix(c,d,t.x),t.y);
}
fn height_at(uv:vec2<f32>)->f32 {return height_at_or(uv,0.0);}
@vertex
fn vs_surface(@builtin(vertex_index) vertex:u32,@builtin(instance_index) row:u32)->Vertex {
    let cells=vec2<f32>(u.grid.xy);
    let coordinate=vec2<f32>(f32(vertex/2u),f32(row+vertex%2u))/cells;
    let size=vec2<f32>(textureDimensions(heights));
    let uv=(coordinate*(size-1.0)+0.5)/size;
    let geo=vec2<f32>(mix(u.region.x,u.region.z,uv.x),mix(u.region.w,u.region.y,uv.y));
    let h=height_at(uv);
    var logical:vec2<f32>;var depth:f32;var w=1.0;
    if u.extra.x>0.5 {
        let x=(geo.x+u.extra.y-u.a.x)*u.a.z;
        let y=(geo.y-u.a.y)*u.a.w;
        let z=h*u.b.x;
        let xr=x*u.b.y-y*u.b.z;let yr=x*u.b.z+y*u.b.y;
        logical=vec2<f32>(u.c.y+xr*u.c.w,
            u.c.z-yr*u.b.w*u.d.x+yr*u.c.x*u.d.y-z*u.c.x*u.d.z-z*u.b.w*u.d.w);
        // Ground distance is monotonic along viewing rays in the shared affine
        // Local projector, including large layer spread and near-horizontal views.
        depth=clamp(0.5+(yr-u.grid.z)*u.grid.w,0.001,0.999);
    } else {
        let lat=radians(geo.y);let lon=radians(geo.x);let radius=1.0+h*u.sphere.z;
        let p=radius*vec3<f32>(cos(lat)*cos(lon),sin(lat),cos(lat)*sin(lon));
        let cy=cos(u.globe.x);let sy=sin(u.globe.x);let cp=cos(u.globe.y);let sp=sin(u.globe.y);
        let x=p.x*cy+p.z*sy;let z=-p.x*sy+p.z*cy;
        let y=p.y*cp-z*sp;let vz=p.y*sp+z*cp;
        w=max(u.globe.w-vz,0.001);
        logical=u.sphere.xy-vec2<f32>(x,y)*u.globe.z/w;
        let near=max(u.globe.w-1.05,0.05);let far=u.globe.w+1.05;
        depth=clamp(far/(far-near)*(1.0-near/w),0.001,0.999);
    }
    let xy=(logical-u.viewport.xy)/u.viewport.zw;
    var out:Vertex;
    out.clip=vec4<f32>((xy.x*2.0-1.0)*w,(1.0-xy.y*2.0)*w,depth*w,w);
    out.uv=uv;out.elevation=h;out.geo=geo;
    return out;
}
fn isoline(h:f32,interval:f32)->f32 {
    let level=h/interval;let width=max(fwidth(level),0.00001);
    let distance=abs(fract(level+0.5)-0.5);
    return (1.0-smoothstep(width*u.style.y*0.2,width*(u.style.y*0.5+0.6),distance))*min(1.0,0.6/width);
}
@fragment
fn fs_surface(v:Vertex)->@location(0) vec4<f32> {
    let pixel=max(vec2<f32>(1.0),fwidth(v.uv)*vec2<f32>(textureDimensions(heights)));
    let interval=select(max(200.0,u.style.x),u.style.x,v.elevation>=0.0);
    let minor=isoline(v.elevation,interval);
    let major=isoline(v.elevation,max(50.0,interval*10.0));
    let dh=fwidth(v.elevation);
    let coast=(1.0-smoothstep(max(dh*0.2,0.001),max(dh,0.002),abs(v.elevation)))*step(0.001,dh)*u.sphere.w;
    if u.extra.z>0.5 {
        let center=(u.exclude.x+u.exclude.z)*0.5;
        let lon=center+((v.geo.x-center+540.0)%360.0)-180.0;
        if lon>=u.exclude.x && lon<=u.exclude.z && v.geo.y>=u.exclude.y && v.geo.y<=u.exclude.w {discard;}
    }
    var color=textureSampleLevel(pigments,linear_sampler,v.uv,0.0);
    if color.a<0.5 {discard;}
    color=vec4<f32>(atlas_shade(v.uv,v.geo,color.rgb,pixel),1.0);
    let enabled=select(u.style.w,u.style.z,v.elevation>=0.0);
    color=vec4<f32>(mix(color.rgb,u.minor.rgb,minor*select(0.18,0.32,v.elevation>=0.0)*enabled),1.0);
    color=vec4<f32>(mix(color.rgb,u.major.rgb,major*select(0.35,0.60,v.elevation>=0.0)*enabled),1.0);
    color=vec4<f32>(mix(color.rgb,vec3<f32>(0.24,0.37,0.40),coast*0.45),1.0);
    return color;
}
