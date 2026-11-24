// GPU instanced 100x100 tile-loading dissolve grid with burning-edge CA fringe.
// Replaces ~44,000 CPU-generated egui vertices per pending tile with 1 quad (4 vertices).

struct Uniforms {
    viewport: vec4<f32>,       // min_x, min_y, size_x, size_y (physical px)
    contour_color: vec4<f32>,  // display RGBA
    hot_color: vec4<f32>,      // display RGBA
    params: vec4<f32>,         // x = breath, y = pixels_per_point, z = 0, w = 0
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var ranks_tex: texture_2d<u32>;

struct VsIn {
    @builtin(vertex_index) vidx: u32,
    @location(0) nw: vec2<f32>,
    @location(1) ne: vec2<f32>,
    @location(2) se: vec2<f32>,
    @location(3) sw: vec2<f32>,
    @location(4) removed: u32,
    @location(5) rank_offset: u32,
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) du_dpx: vec2<f32>,
    @location(2) @interpolate(flat) removed: u32,
    @location(3) @interpolate(flat) rank_offset: u32,
}

const GRID_F: f32 = 100.0;
const TOTAL_CELLS: u32 = 10000u;
const EDGE_BAND: f32 = 0.14;
const CELL_INSET: f32 = 0.10;
const OPACITY: f32 = 0.75;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let e_u = in.ne - in.nw;
    let e_v = in.sw - in.nw;
    let det = e_u.x * e_v.y - e_u.y * e_v.x;
    if abs(det) < 1e-4 {
        out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.uv = vec2<f32>(0.0);
        out.du_dpx = vec2<f32>(0.0);
        out.removed = 0u;
        out.rank_offset = 0u;
        return out;
    }
    // Inverse Jacobian column 0: UV delta per +1 physical pixel in screen X.
    let du_dpx = vec2<f32>(e_v.y, -e_u.y) / det;

    // 4-vertex TriangleStrip: 0=(0,0) NW, 1=(1,0) NE, 2=(0,1) SW, 3=(1,1) SE.
    let cu = f32(in.vidx & 1u);
    let cv = select(0.0, 1.0, in.vidx >= 2u);
    var pos = mix(mix(in.nw, in.ne, cu), mix(in.sw, in.se, cu), cv);

    // Pad horizontally by 2 logical px so the +/-1.8 px CA fringe never clips at tile edges.
    let pad_px = 2.0 * u.params.y;
    let center_x = 0.25 * (in.nw.x + in.ne.x + in.se.x + in.sw.x);
    let x_sign = select(-1.0, 1.0, pos.x >= center_x);
    let shift_x = x_sign * pad_px;
    pos.x += shift_x;
    let uv = vec2<f32>(cu, cv) + du_dpx * shift_x;

    let rel = (pos - u.viewport.xy) / u.viewport.zw;
    let ndc = vec2<f32>(rel.x * 2.0 - 1.0, 1.0 - rel.y * 2.0);
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.du_dpx = du_dpx;
    out.removed = in.removed;
    out.rank_offset = in.rank_offset;
    return out;
}

fn cell_rank(col: i32, row: i32, rank_offset: u32) -> u32 {
    let base = textureLoad(ranks_tex, vec2<i32>(col, row), 0).r;
    return (base + rank_offset) % TOTAL_CELLS;
}

fn eval_ghost_cell(
    uv: vec2<f32>,
    du_dpx: vec2<f32>,
    col: i32,
    row: i32,
    removed: u32,
    rank_offset: u32,
    dir: f32,
) -> f32 {
    if col < 0 || col >= 100 || row < 0 || row >= 100 {
        return 0.0;
    }
    let rank = cell_rank(col, row, rank_offset);
    if rank < removed {
        return 0.0;
    }
    let edge = clamp(f32(rank - removed) / (f32(TOTAL_CELLS) * EDGE_BAND), 0.0, 1.0);
    if edge >= 0.4 {
        return 0.0;
    }
    let t = 1.0 - edge / 0.4;
    let shift_uv = du_dpx * (dir * t * 1.8 * u.params.y);
    let local = (uv - shift_uv) * GRID_F - vec2<f32>(f32(col), f32(row));
    if local.x < CELL_INSET || local.x > (1.0 - CELL_INSET)
        || local.y < CELL_INSET || local.y > (1.0 - CELL_INSET)
    {
        return 0.0;
    }
    return (t * 40.0 * u.params.x * OPACITY) / 255.0;
}

fn sample_ghost(
    uv: vec2<f32>,
    du_dpx: vec2<f32>,
    removed: u32,
    rank_offset: u32,
    dir: f32,
) -> f32 {
    // A ghost shifted by +dir * s (0 <= s <= 1.8*ppp) covers `uv` from a source cell
    // along `uv - dir * s * du_dpx`. Probe 3 points along that short segment, skipping duplicates.
    let step_uv = du_dpx * (dir * 0.9 * u.params.y);
    var alpha = 0.0;
    var prev0 = vec2<i32>(-999, -999);
    var prev1 = vec2<i32>(-999, -999);
    for (var k = 0; k < 3; k += 1) {
        let p = (uv - step_uv * f32(k)) * GRID_F;
        let c = vec2<i32>(floor(p));
        if (c.x == prev0.x && c.y == prev0.y) || (c.x == prev1.x && c.y == prev1.y) {
            continue;
        }
        if k == 0 {
            prev0 = c;
        } else {
            prev1 = c;
        }
        let a = eval_ghost_cell(uv, du_dpx, c.x, c.y, removed, rank_offset, dir);
        if a > 0.0 {
            alpha = a + alpha * (1.0 - a);
        }
    }
    return alpha;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let breath = u.params.x;
    var out_col = vec4<f32>(0.0);

    // 1. Main cell
    let cell_f = in.uv * GRID_F;
    let col = i32(floor(cell_f.x));
    let row = i32(floor(cell_f.y));
    if col >= 0 && col < 100 && row >= 0 && row < 100 {
        let frac = cell_f - vec2<f32>(f32(col), f32(row));
        if frac.x >= CELL_INSET && frac.x <= (1.0 - CELL_INSET)
            && frac.y >= CELL_INSET && frac.y <= (1.0 - CELL_INSET)
        {
            let rank = cell_rank(col, row, in.rank_offset);
            if rank >= in.removed {
                let edge = clamp(f32(rank - in.removed) / (f32(TOTAL_CELLS) * EDGE_BAND), 0.0, 1.0);
                let mix_t = pow(1.0 - edge, 1.8);
                let rgb = mix(u.contour_color.rgb, u.hot_color.rgb, mix_t);
                let alpha = (mix(8.0, 80.0, 1.0 - edge) * breath * OPACITY) / 255.0;
                out_col = vec4<f32>(rgb * alpha, alpha);
            }
        }
    }

    // 2. Hot ghost (shifted +X)
    let hot_a = sample_ghost(in.uv, in.du_dpx, in.removed, in.rank_offset, 1.0);
    if hot_a > 0.0 {
        let src = vec4<f32>(u.hot_color.rgb * hot_a, hot_a);
        out_col = src + out_col * (1.0 - hot_a);
    }

    // 3. Contour ghost (shifted -X)
    let cnt_a = sample_ghost(in.uv, in.du_dpx, in.removed, in.rank_offset, -1.0);
    if cnt_a > 0.0 {
        let src = vec4<f32>(u.contour_color.rgb * cnt_a, cnt_a);
        out_col = src + out_col * (1.0 - cnt_a);
    }

    if out_col.a <= 0.0001 {
        discard;
    }
    return map_output_gamma(out_col);
}
