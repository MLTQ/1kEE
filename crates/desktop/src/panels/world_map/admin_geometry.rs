//! Prepare concave polygons with holes once, off the UI thread.
use crate::model::GeoPoint;
use lyon_tessellation::path::{Path, math::point};
use lyon_tessellation::{BuffersBuilder, FillOptions, FillTessellator, FillVertex, VertexBuffers};

#[derive(Default)]
pub(super) struct Fill {
    pub vertices: Vec<GeoPoint>,
    pub indices: Vec<u32>,
}

pub(super) fn closed(ring: &[GeoPoint]) -> bool {
    ring.len() >= 4
        && ring
            .first()
            .zip(ring.last())
            .is_some_and(|(a, b)| a.lat == b.lat && a.lon == b.lon)
}

/// The archive unwraps consecutive source vertices before simplification. Shift
/// whole paths only: re-unwrapping a simplified edge can reverse a long arc.
pub(super) fn align_rings(rings: &mut [Vec<GeoPoint>]) {
    let anchor = rings.first().and_then(|r| r.first()).map_or(0., |p| p.lon);
    for ring in rings {
        if let Some(first) = ring.first() {
            let shift = ((anchor - first.lon) / 360.).round() * 360.;
            if shift != 0. {
                for p in ring {
                    p.lon += shift;
                }
            }
        }
    }
}

pub(super) fn tessellate(rings: &[Vec<GeoPoint>]) -> Result<Fill, String> {
    if rings.is_empty() || rings.iter().any(|r| !closed(r)) {
        return Err("Open boundary geometry cannot be shaded".into());
    }
    let mut builder = Path::builder();
    for ring in rings {
        builder.begin(point(ring[0].lon, ring[0].lat));
        for p in &ring[1..ring.len() - 1] {
            builder.line_to(point(p.lon, p.lat));
        }
        builder.close();
    }
    let mut buffers: VertexBuffers<GeoPoint, u32> = VertexBuffers::new();
    FillTessellator::new()
        .tessellate_path(
            &builder.build(),
            &FillOptions::even_odd(),
            &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex<'_>| {
                let p = vertex.position();
                GeoPoint { lon: p.x, lat: p.y }
            }),
        )
        .map_err(|e| format!("Cannot shade boundary: {e:?}"))?;
    Ok(Fill {
        vertices: buffers.vertices,
        indices: buffers.indices,
    })
}

#[cfg(test)]
#[path = "admin_geometry_tests.rs"]
mod tests;
