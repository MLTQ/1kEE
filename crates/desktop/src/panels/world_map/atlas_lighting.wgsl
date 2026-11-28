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
    let start=max(max(spacing.x,spacing.y)*1.25,footprint*0.9);
    // Scale shadow reach with viewport footprint so continental mountain chains
    // cast long, tangible shadows across adjacent plains as well as local valleys.
    let max_dist=clamp(footprint*72.0,28000.0,240000.0);
    var visibility=1.0;
    var distance=start;
    for (var i=0u;i<24u;i+=1u) {
        if distance>max_dist {break;}
        let sample_uv=uv+ray*distance;
        if !atlas_inside(sample_uv) {break;}
        let obstruction=(height_at_or(sample_uv,h)-h)*exaggeration/distance;
        // Soft finite angular penumbra yields physical studio-lit shadow edges.
        visibility=min(visibility,1.0-smoothstep(rise-0.028,rise+0.062,obstruction));
        if visibility<=0.001 {return 0.0;}
        distance*=1.245;
    }
    return visibility;
}
fn atlas_shade(uv:vec2<f32>,geo:vec2<f32>,base:vec3<f32>,pixel:vec2<f32>)->vec3<f32> {
    let strength=u.appearance.y;
    let light=atlas_light(geo);
    var shaded=base;
    if strength>0.0001 {
        let size=vec2<f32>(textureDimensions(heights));
        let spacing=max(vec2<f32>(0.05),vec2<f32>(
            (u.region.z-u.region.x)*111195.0*max(abs(cos(radians(geo.y))),0.05),
            (u.region.w-u.region.y)*111195.0)/size);
        let h=height_at(uv);
        let is_land=h>=0.0;
        let footprint=max(pixel.x*spacing.x,pixel.y*spacing.y);
        let fine_radius=max(vec2<f32>(1.0),pixel*0.65);
        let mid_radius=max(fine_radius*2.5,vec2<f32>(250.0)/spacing);
        let broad_radius=max(fine_radius*5.5,vec2<f32>(900.0)/spacing);
        let fine=atlas_slope(uv,h,spacing,fine_radius);
        let mid=atlas_slope(uv,h,spacing,mid_radius);
        let broad=atlas_slope(uv,h,spacing,broad_radius);
        // Scale-adaptive sculptural exaggeration: natural crisp relief up close,
        // dramatic physical plaster-model relief on continental/regional views.
        let scale_boost=1.0+4.2*smoothstep(60.0,3200.0,footprint);
        let ocean_damp=select(0.68,1.0,is_land);
        let exaggeration=(1.2+2.4*strength)*scale_boost*ocean_damp;
        let normal=normalize(vec3<f32>(-fine*exaggeration,1.0));
        let mid_normal=normalize(vec3<f32>(-mid*exaggeration*0.85,1.0));
        let broad_normal=normalize(vec3<f32>(-broad*exaggeration*0.68,1.0));
        let diffuse=max(dot(normal,light),0.0)*0.54
            +max(dot(mid_normal,light),0.0)*0.26
            +max(dot(broad_normal,light),0.0)*0.20;
        let cast_exag=exaggeration*select(0.85,1.25,is_land);
        let visibility=atlas_cast(uv,h,spacing,light,footprint,cast_exag);
        // Multi-scale crevice ambient occlusion + sunlit ridge convexity.
        // 8-direction sampling on the tightest octave removes cardinal-axis bias.
        var enclosure=0.0;
        var convexity=0.0;
        let inv_span=1.0/(spacing*size);
        let d0=max(220.0,footprint*2.1);
        let delta0=vec2<f32>(d0)*inv_span;
        let diag0=delta0*0.70710678;
        let mean0=(
            height_at_or(uv+vec2<f32>(delta0.x,0.0),h)+height_at_or(uv-vec2<f32>(delta0.x,0.0),h)+
            height_at_or(uv+vec2<f32>(0.0,delta0.y),h)+height_at_or(uv-vec2<f32>(0.0,delta0.y),h)+
            height_at_or(uv+diag0,h)+height_at_or(uv-diag0,h)+
            height_at_or(uv+vec2<f32>(diag0.x,-diag0.y),h)+height_at_or(uv+vec2<f32>(-diag0.x,diag0.y),h)
        )*0.125;
        enclosure+=max((mean0-h)/d0,0.0)*1.35;
        convexity+=max((h-mean0)/d0,0.0)*1.10;
        let d1=max(1100.0,footprint*6.2);
        let delta1=vec2<f32>(d1)*inv_span;
        let mean1=(
            height_at_or(uv+vec2<f32>(delta1.x,0.0),h)+height_at_or(uv-vec2<f32>(delta1.x,0.0),h)+
            height_at_or(uv+vec2<f32>(0.0,delta1.y),h)+height_at_or(uv-vec2<f32>(0.0,delta1.y),h)
        )*0.25;
        enclosure+=max((mean1-h)/d1,0.0)*1.0;
        convexity+=max((h-mean1)/d1,0.0)*0.75;
        let d2=max(4200.0,footprint*16.0);
        let delta2=vec2<f32>(d2)*inv_span;
        let mean2=(
            height_at_or(uv+vec2<f32>(delta2.x,0.0),h)+height_at_or(uv-vec2<f32>(delta2.x,0.0),h)+
            height_at_or(uv+vec2<f32>(0.0,delta2.y),h)+height_at_or(uv-vec2<f32>(0.0,delta2.y),h)
        )*0.25;
        enclosure+=max((mean2-h)/d2,0.0)*0.85;
        convexity+=max((h-mean2)/d2,0.0)*0.55;
        let ao_scale=scale_boost*ocean_damp;
        let ambient=1.0-clamp(enclosure*ao_scale*strength*1.15,0.0,0.52);
        // Steep-slope lithographic ink pooling: mountain flanks deepen into rich
        // dark umber/charcoal ink while flat plains stay bright chalky parchment.
        let grad_mag=length(fine*exaggeration)*0.6+length(mid*exaggeration)*0.4;
        let slope_ink=smoothstep(0.22,1.55,grad_mag)*select(0.16,0.33,is_land)*min(strength,1.4);
        let ink_tint=select(vec3<f32>(0.11,0.23,0.31),vec3<f32>(0.22,0.13,0.12),is_land);
        let inked_base=mix(base,ink_tint,slope_ink);
        // Sculptural studio key light + cast shadow + cool slate shadow fill.
        let key=clamp((diffuse*visibility-0.03)/max(light.z-0.03,0.24),0.0,1.40);
        let shadow_tint=select(vec3<f32>(0.26,0.36,0.48),vec3<f32>(0.20,0.22,0.31),is_land);
        let shadow_lift=select(vec3<f32>(0.012,0.022,0.035),vec3<f32>(0.018,0.018,0.028),is_land);
        let cool_shadow=inked_base*shadow_tint+shadow_lift;
        let warm_light=inked_base*select(vec3<f32>(1.03,1.03,1.01),vec3<f32>(1.055,1.028,0.975),is_land);
        // Extra cast-shadow bite on flat lowlands so mountain shadows read clearly on rag paper.
        let cast_shade=mix(0.50,1.0,visibility);
        shaded=mix(cool_shadow,warm_light,min(key,1.0))*ambient*mix(1.0,cast_shade,0.40);
        // Crisp warm-parchment sunlit crest highlight on exposed ridges.
        let crest=clamp(convexity*ao_scale*1.05,0.0,1.0)*visibility*smoothstep(0.12,0.82,diffuse);
        shaded+=select(vec3<f32>(0.85,0.96,0.96),vec3<f32>(1.0,0.95,0.84),is_land)*(max(key-0.94,0.0)*0.24+crest*0.21);
        shaded=mix(base,shaded,min(strength,1.0));
    }
    if u.light.w>0.5 {shaded*=mix(0.25,1.0,smoothstep(-0.10,0.12,light.z));}
    let luminance=dot(shaded,vec3<f32>(0.2126,0.7152,0.0722));
    return clamp(mix(vec3<f32>(luminance),shaded,u.appearance.x),vec3<f32>(0.0),vec3<f32>(1.0));
}
