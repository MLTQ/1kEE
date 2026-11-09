// All distances are measured from the source grid, never screen-space noise.
fn atlas_light(geo:vec2<f32>)->vec3<f32> {
    if u.light.w<0.5 {return u.light.xyz;}
    let lat=radians(geo.y);let lon=radians(geo.x);
    let east=vec3<f32>(-sin(lon),0.0,cos(lon));
    let north=vec3<f32>(-sin(lat)*cos(lon),cos(lat),-sin(lat)*sin(lon));
    let up=vec3<f32>(cos(lat)*cos(lon),sin(lat),cos(lat)*sin(lon));
    return vec3<f32>(dot(u.light.xyz,east),dot(u.light.xyz,north),dot(u.light.xyz,up));
}
fn atlas_inside(uv:vec2<f32>)->bool {
    return uv.y>=0.0 && uv.y<=1.0 && (u.region.z-u.region.x>359.9 || (uv.x>=0.0 && uv.x<=1.0));
}
fn atlas_slope(uv:vec2<f32>,h:f32,spacing:vec2<f32>,radius:vec2<f32>)->vec2<f32> {
    let size=vec2<f32>(textureDimensions(heights));
    let delta=radius/size;
    let east=height_at_or(uv+vec2<f32>(delta.x,0.0),h);
    let west=height_at_or(uv-vec2<f32>(delta.x,0.0),h);
    let north=height_at_or(uv-vec2<f32>(0.0,delta.y),h);
    let south=height_at_or(uv+vec2<f32>(0.0,delta.y),h);
    return vec2<f32>(east-west,north-south)/(2.0*radius*spacing);
}
fn atlas_cast(uv:vec2<f32>,h:f32,spacing:vec2<f32>,light:vec3<f32>,footprint:f32,exaggeration:f32)->f32 {
    if light.z<=0.01 {return 0.0;}
    let horizontal=length(light.xy);
    if horizontal<0.01 {return 1.0;}
    let direction=light.xy/horizontal;
    let size=vec2<f32>(textureDimensions(heights));
    let ray=vec2<f32>(direction.x,-direction.y)/(spacing*size);
    let rise=light.z/horizontal;
    let start=max(max(spacing.x,spacing.y)*1.5,footprint);
    var visibility=1.0;
    for (var i=0u;i<16u;i+=1u) {
        let distance=start*exp2(f32(i)*0.36);
        if distance>40000.0 {break;}
        let sample_uv=uv+ray*distance;
        if !atlas_inside(sample_uv) {break;}
        let obstruction=(height_at_or(sample_uv,h)-h)*exaggeration/distance;
        // A finite angular penumbra avoids binary/noisy shadow edges.
        visibility=min(visibility,1.0-smoothstep(rise-0.035,rise+0.07,obstruction));
    }
    return visibility;
}
fn atlas_shade(uv:vec2<f32>,geo:vec2<f32>,base:vec3<f32>,pixel:vec2<f32>)->vec3<f32> {
    let size=vec2<f32>(textureDimensions(heights));
    let spacing=max(vec2<f32>(0.05),vec2<f32>(
        (u.region.z-u.region.x)*111195.0*max(abs(cos(radians(geo.y))),0.05),
        (u.region.w-u.region.y)*111195.0)/size);
    let h=height_at(uv);
    let fine_radius=max(vec2<f32>(1.0),pixel*0.7);
    let broad_radius=max(fine_radius*4.0,vec2<f32>(600.0)/spacing);
    let fine=atlas_slope(uv,h,spacing,fine_radius);
    let broad=atlas_slope(uv,h,spacing,broad_radius);
    let strength=u.appearance.y;
    let exaggeration=1.0+1.5*strength;
    let normal=normalize(vec3<f32>(-fine*exaggeration,1.0));
    let broad_normal=normalize(vec3<f32>(-broad*exaggeration,1.0));
    let light=atlas_light(geo);
    let diffuse=max(dot(normal,light),0.0)*0.68+max(dot(broad_normal,light),0.0)*0.32;
    let footprint=max(pixel.x*spacing.x,pixel.y*spacing.y);
    let visibility=atlas_cast(uv,h,spacing,light,footprint,exaggeration);
    // Valley enclosure operates at broad physical scales, keeping mountain
    // masses legible alongside native ridge detail.
    var enclosure=0.0;
    for (var i=0u;i<2u;i+=1u) {
        let distance=max(900.0*exp2(f32(i)*2.0),footprint*6.0);
        let delta=vec2<f32>(distance)/(spacing*size);
        let mean=(height_at_or(uv+vec2<f32>(delta.x,0.0),h)+height_at_or(uv-vec2<f32>(delta.x,0.0),h)
            +height_at_or(uv+vec2<f32>(0.0,delta.y),h)+height_at_or(uv-vec2<f32>(0.0,delta.y),h))*0.25;
        enclosure+=max((mean-h)/distance,0.0);
    }
    let ambient=1.0-clamp(enclosure*strength*0.7,0.0,0.40);
    let key=clamp((diffuse*visibility-0.04)/max(light.z-0.04,0.25),0.0,1.3);
    let cool_shadow=base*vec3<f32>(0.25,0.29,0.44)+vec3<f32>(0.015,0.018,0.035);
    let warm_light=base*vec3<f32>(1.025,1.005,0.955);
    var shaded=mix(cool_shadow,warm_light,min(key,1.0))*ambient;
    shaded+=vec3<f32>(1.0,0.91,0.73)*max(key-1.0,0.0)*0.18;
    shaded=mix(base,shaded,min(strength,1.0));
    if u.light.w>0.5 {shaded*=mix(0.25,1.0,smoothstep(-0.10,0.12,light.z));}
    let luminance=dot(shaded,vec3<f32>(0.2126,0.7152,0.0722));
    return clamp(mix(vec3<f32>(luminance),shaded,u.appearance.x),vec3<f32>(0.0),vec3<f32>(1.0));
}
