use super::*;
use crate::model::GeoPoint;

fn source(
    zoom_bucket: i32,
    lat_bucket: i32,
    lon_bucket: i32,
    priority: u64,
    points: &[(f32, f32)],
) -> Source {
    let contours = Arc::new(if points.is_empty() {
        vec![]
    } else {
        vec![ContourPath {
            elevation_m: priority as f32 * 50.0,
            points: points
                .iter()
                .map(|&(lon, lat)| GeoPoint { lon, lat })
                .collect(),
        }]
    });
    Source {
        priority,
        tile: LocalTileGeometry {
            id: LocalTileId {
                zoom_bucket,
                lat_bucket,
                lon_bucket,
            },
            bounds: Bounds::from_contours(&contours),
            contours,
        },
    }
}

fn spans(result: &Result, id: LocalTileId) -> Vec<(f32, f32)> {
    result
        .frame
        .tiles
        .iter()
        .filter(|t| t.id == id)
        .flat_map(|t| t.contours.iter())
        .map(|c| (c.points.first().unwrap().lon, c.points.last().unwrap().lon))
        .collect()
}

#[test]
fn missing_edge_cell_does_not_block_ready_detail_or_leave_double_contours() {
    let old = source(5, 0, 0, 1, &[(-0.06, 0.0), (0.06, 0.0)]);
    let fine = source(6, 0, 0, 2, &[(-0.03, 0.0), (0.03, 0.0)]);
    let result = compose(
        ActiveBody::Earth,
        vec![old.clone(), fine.clone()],
        HashMap::new(),
    );
    assert_eq!(result.frame.tiles.len(), 2);
    assert_eq!(spans(&result, fine.tile.id), vec![(-0.03, 0.03)]);
    let leftovers = spans(&result, old.tile.id);
    assert_eq!(leftovers.len(), 2);
    let edge = core(ActiveBody::Earth, fine.tile.id).max_lon as f32;
    assert_eq!(leftovers, vec![(-0.06, -edge), (edge, 0.06)]);
    // The old line is split at both boundaries, never joined across the hole.
    assert!(leftovers.iter().all(|&(a, b)| b <= -edge || a >= edge));
}

#[test]
fn decoded_empty_replaces_old_lines_but_missing_tiles_leave_them_intact() {
    let old = source(5, 0, 0, 1, &[(-0.06, 0.0), (0.06, 0.0)]);
    let empty = source(6, 0, 0, 2, &[]);
    let missing = compose(ActiveBody::Earth, vec![old.clone()], HashMap::new());
    assert_eq!(spans(&missing, old.tile.id), vec![(-0.06, 0.06)]);
    let known_empty = compose(ActiveBody::Earth, vec![old.clone(), empty], HashMap::new());
    assert_eq!(spans(&known_empty, old.tile.id).len(), 2);
}

#[test]
fn zoom_out_and_back_in_removes_only_the_replaced_pieces() {
    let fine = source(6, 0, 0, 1, &[(-0.03, 0.0), (0.03, 0.0)]);
    let wide = source(5, 0, 0, 2, &[(-0.06, 0.0), (0.06, 0.0)]);
    let out = compose(
        ActiveBody::Earth,
        vec![fine.clone(), wide.clone()],
        HashMap::new(),
    );
    assert!(spans(&out, fine.tile.id).is_empty());
    let mut returning = fine.clone();
    returning.priority = 3;
    let back = compose(
        ActiveBody::Earth,
        vec![returning.clone(), wide.clone()],
        out.pieces,
    );
    assert_eq!(spans(&back, fine.tile.id), vec![(-0.03, 0.03)]);
    assert_eq!(spans(&back, wide.tile.id).len(), 2);
    let same = compose(
        ActiveBody::Earth,
        vec![wide, returning],
        back.pieces.clone(),
    );
    for tile in &back.frame.tiles {
        assert!(Arc::ptr_eq(
            &tile.contours,
            &same.pieces[&tile.id].tile.contours
        ));
    }
}

#[test]
fn legacy_tiles_are_clipped_to_their_core_at_negative_mexico_coordinates() {
    let id = LocalTileId {
        zoom_bucket: 6,
        lat_bucket: 260,
        lon_bucket: -1370,
    };
    let r = core(ActiveBody::Earth, id);
    let latitude = ((r.min_lat + r.max_lat) * 0.5) as f32;
    let legacy = source(
        6,
        id.lat_bucket,
        id.lon_bucket,
        1,
        &[
            (r.min_lon as f32 - 0.1, latitude),
            (r.max_lon as f32 + 0.1, latitude),
        ],
    );
    let result = compose(ActiveBody::Earth, vec![legacy], HashMap::new());
    assert_eq!(
        spans(&result, id),
        vec![(r.min_lon as f32, r.max_lon as f32)]
    );
}

#[test]
fn subtraction_preserves_union_area_without_overlap() {
    let a = Rect {
        min_lon: -1.0,
        max_lon: 1.0,
        min_lat: -1.0,
        max_lat: 1.0,
    };
    let b = Rect {
        min_lon: -0.2,
        max_lon: 0.4,
        min_lat: -0.5,
        max_lat: 0.3,
    };
    let pieces = subtract(a, b);
    let area = |r: &Rect| (r.max_lon - r.min_lon) * (r.max_lat - r.min_lat);
    assert!((pieces.iter().map(area).sum::<f64>() + area(&b) - area(&a)).abs() < 1e-12);
    for (i, x) in pieces.iter().enumerate() {
        for y in &pieces[i + 1..] {
            assert!(
                x.max_lon <= y.min_lon
                    || y.max_lon <= x.min_lon
                    || x.max_lat <= y.min_lat
                    || y.max_lat <= x.min_lat
            );
        }
    }
}
