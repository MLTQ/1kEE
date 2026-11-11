// Geographic point in the local ground chart, in degrees of meridian arc
// for polar views. Mirrors model::polar::offset, including its deep-zoom form.
fn local_ground(point: vec2<f32>, focus: vec2<f32>, pole: f32) -> vec2<f32> {
    let delta = point - focus;
    if pole == 0.0 { return delta; }
    let a = radians(delta.x);
    let r = 90.0 - pole * point.y;
    let half_sin = sin(0.5 * a);
    return vec2<f32>(r * sin(a), delta.y + 2.0 * pole * r * half_sin * half_sin);
}
