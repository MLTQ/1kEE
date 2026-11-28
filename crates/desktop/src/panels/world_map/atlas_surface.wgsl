struct Uniforms {
    viewport:vec4<f32>, region:vec4<f32>, a:vec4<f32>, b:vec4<f32>, c:vec4<f32>, d:vec4<f32>,
    globe:vec4<f32>, sphere:vec4<f32>, style:vec4<f32>, grid:vec4<f32>, extra:vec4<f32>,
    major:vec4<f32>, minor:vec4<f32>, exclude:vec4<f32>,
    light:vec4<f32>, appearance:vec4<f32>,
}
@group(0) @binding(0) var<uniform> u:Uniforms;
@group(0) @binding(1) var tile_coverage:texture_2d<u32>;
@group(1) @binding(0) var heights:texture_2d<f32>;
@group(1) @binding(1) var pigments:texture_2d<f32>;
@group(1) @binding(2) var linear_sampler:sampler;
@group(1) @binding(3) var forest_coverage:texture_2d<f32>;

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
    var uv=(coordinate*(size-1.0)+0.5)/size;
    var geo=vec2<f32>(mix(u.region.x,u.region.z,uv.x),mix(u.region.w,u.region.y,uv.y));
    // The polar cap must reach both the geographic pole and the longitude
    // seam. Sample clamped edge texels at the actual raster bounds.
    if u.extra.x>0.5 && u.globe.x!=0.0 {
        uv=coordinate;
        geo=vec2<f32>(mix(u.region.x,u.region.z,uv.x),mix(u.region.w,u.region.y,uv.y));
    }
    if u.extra.w>0.5 {
        geo=vec2<f32>(mix(u.exclude.x,u.exclude.z,coordinate.x),mix(u.exclude.w,u.exclude.y,coordinate.y));
        uv=vec2<f32>((geo.x-u.region.x)/(u.region.z-u.region.x),(u.region.w-geo.y)/(u.region.w-u.region.y));
    }
    let h=height_at(uv);
    var logical:vec2<f32>;var depth:f32;var w=1.0;
    if u.extra.x>0.5 {
        let offset=local_ground(vec2<f32>(geo.x+u.extra.y,geo.y),u.a.xy,u.globe.x);
        let x=offset.x*u.a.z;let y=offset.y*u.a.w;
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
    let dh=fwidth(h);
    let level=h/interval;
    let width=max(fwidth(level),0.00001);
    let distance=abs(fract(level+0.5)-0.5);
    // Suppress contour ink on completely flat plateaus/0m coastal bands (fwidth == 0)
    // and leave the 0m sea-level isoline exclusively to the dedicated Coastline toggle.
    let slope_mask=smoothstep(0.0002,0.002,dh);
    let non_zero_level=step(0.5,abs(round(level)));
    return (1.0-smoothstep(width*u.style.y*0.2,width*(u.style.y*0.5+0.6),distance))*min(1.0,0.6/width)*slope_mask*non_zero_level;
}
@fragment
fn fs_surface(v:Vertex)->@location(0) vec4<f32> {
    let pixel=max(vec2<f32>(1.0),fwidth(v.uv)*vec2<f32>(textureDimensions(heights)));
    let interval=select(max(200.0,u.style.x),u.style.x,v.elevation>=0.0);
    let minor=isoline(v.elevation,interval);
    let major=isoline(v.elevation,max(50.0,interval*10.0));
    let dh=fwidth(v.elevation);
    let coast=(1.0-smoothstep(max(dh*0.2,0.001),max(dh,0.002),abs(v.elevation)))*step(0.001,dh)*u.sphere.w;
    if u.extra.z>1.5 {
        let lon=((v.geo.x+540.0)%360.0);
        let cell=clamp(vec2<i32>(floor(vec2<f32>(lon,v.geo.y+90.0)/5.0)),vec2<i32>(0),vec2<i32>(71,35));
        if textureLoad(tile_coverage,cell,0).r>0u {discard;}
    } else if u.extra.z>0.5 {
        let center=(u.exclude.x+u.exclude.z)*0.5;
        let lon=center+((v.geo.x-center+540.0)%360.0)-180.0;
        if lon>=u.exclude.x && lon<=u.exclude.z && v.geo.y>=u.exclude.y && v.geo.y<=u.exclude.w {discard;}
    }
    var color=textureSampleLevel(pigments,linear_sampler,v.uv,0.0);
    if color.a<0.5 {discard;}
    if u.appearance.z>0.5 {
        let coverage=textureSampleLevel(forest_coverage,linear_sampler,v.uv,0.0).r;
        color=vec4<f32>(mix(color.rgb,vec3<f32>(0.22,0.36,0.20),coverage*0.74),1.0);
    }
    let flags=u32(round(max(u.appearance.w,0.0)));
    let tanaka=(flags & 1u) != 0u;
    let analysis=(flags >> 1u) & 3u;
    var slope_vec=vec2<f32>(0.0);
    if tanaka || analysis != 0u {
        let size=vec2<f32>(textureDimensions(heights));
        let spacing=max(vec2<f32>(0.05),vec2<f32>(
            (u.region.z-u.region.x)*111195.0*max(abs(cos(radians(v.geo.y))),0.05),
            (u.region.w-u.region.y)*111195.0)/size);
        let h_c=height_at(v.uv);
        let fine_radius=max(vec2<f32>(1.0),pixel*0.7);
        slope_vec=atlas_slope(v.uv,h_c,spacing,fine_radius);
        if analysis==1u {
            // Slope angle (degrees) mapped to high-legibility cartographic magma ramp.
            let deg=degrees(atan(length(slope_vec)));
            let t=clamp(deg/50.0,0.0,1.0);
            let low=vec3<f32>(0.12,0.18,0.28);
            let mid=vec3<f32>(0.86,0.42,0.16);
            let high=vec3<f32>(0.99,0.92,0.48);
            let ramp=select(mix(low,mid,t*2.0),mix(mid,high,(t-0.5)*2.0),t>=0.5);
            color=vec4<f32>(mix(color.rgb,ramp,0.78),1.0);
        } else if analysis==2u {
            // Aspect compass wheel (N=cyan, E=emerald, S=amber, W=violet), faded on flat ground.
            let mag=length(slope_vec);
            let ang=atan2(-slope_vec.x,-slope_vec.y);
            let wheel=0.5+0.5*cos(ang+vec3<f32>(0.0,2.094,4.188));
            let flat_fade=smoothstep(0.02,0.18,mag);
            color=vec4<f32>(mix(color.rgb,wheel,0.72*flat_fade),1.0);
        } else if analysis==3u {
            // Topographic Position Index (TPI) ridgelines (warm gold) & valleys (deep indigo),
            // scaled adaptively to both local hills and continental mountain ranges.
            let view_span_m=min(spacing.x*size.x,spacing.y*size.y);
            let radius_m=clamp(view_span_m*0.06,60.0,900.0);
            let broad_r=max(fine_radius*4.0,vec2<f32>(radius_m)/spacing);
            let inv_s=1.0/(spacing*size);
            let d_uv=broad_r*spacing*inv_s;
            let mean=(height_at_or(v.uv+vec2<f32>(d_uv.x,0.0),h_c)+height_at_or(v.uv-vec2<f32>(d_uv.x,0.0),h_c)
                +height_at_or(v.uv+vec2<f32>(0.0,d_uv.y),h_c)+height_at_or(v.uv-vec2<f32>(0.0,d_uv.y),h_c))*0.25;
            let norm_m=clamp(radius_m*0.12,12.0,120.0);
            let tpi=clamp((h_c-mean)/norm_m,-1.0,1.0);
            let ridge_col=vec3<f32>(0.99,0.82,0.34);
            let valley_col=vec3<f32>(0.12,0.22,0.52);
            let tpi_col=select(mix(color.rgb,valley_col,abs(tpi)*0.78),mix(color.rgb,ridge_col,tpi*0.85),tpi>=0.0);
            color=vec4<f32>(tpi_col,1.0);
        }
    }
    color=vec4<f32>(atlas_shade(v.uv,v.geo,color.rgb,pixel),1.0);
    let enabled=select(u.style.w,u.style.z,v.elevation>=0.0);
    if tanaka {
        let light=atlas_light(v.geo);
        let grad_len=length(slope_vec);
        let light_len=length(light.xy);
        let lit=select(0.0,clamp(dot(-slope_vec/grad_len,light.xy/light_len),-1.0,1.0),grad_len>0.002 && light_len>0.002);
        let hi_col=vec3<f32>(0.98,0.96,0.90);
        let sh_col=vec3<f32>(0.06,0.08,0.11);
        let tanaka_minor=mix(sh_col,hi_col,lit*0.5+0.5);
        let tanaka_major=mix(sh_col*0.8,vec3<f32>(1.0,0.98,0.93),lit*0.5+0.5);
        let contrast=0.45+0.55*abs(lit);
        color=vec4<f32>(mix(color.rgb,tanaka_minor,minor*select(0.24,0.48,v.elevation>=0.0)*contrast*enabled),1.0);
        color=vec4<f32>(mix(color.rgb,tanaka_major,major*select(0.45,0.78,v.elevation>=0.0)*contrast*enabled),1.0);
    } else {
        color=vec4<f32>(mix(color.rgb,u.minor.rgb,minor*select(0.18,0.32,v.elevation>=0.0)*enabled),1.0);
        color=vec4<f32>(mix(color.rgb,u.major.rgb,major*select(0.35,0.60,v.elevation>=0.0)*enabled),1.0);
    }
    color=vec4<f32>(mix(color.rgb,vec3<f32>(0.24,0.37,0.40),coast*0.45),1.0);
    return color;
}
