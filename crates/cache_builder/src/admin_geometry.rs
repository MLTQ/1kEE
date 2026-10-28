//! Exact OSM node-ID stitching, shared by focused and whole-planet builds.
use crate::util::GeoPoint;
use std::collections::{HashMap, HashSet, VecDeque};

pub type CoordinateResolver<'a> = dyn FnMut(&[i64]) -> Result<Vec<Option<GeoPoint>>, String> + 'a;

pub struct AdminRegion {
    pub relation_id: i64,
    pub name: Option<String>,
    pub rings: Vec<Vec<GeoPoint>>,
    /// All member ways/nodes present and every chain closed, including inner rings.
    pub complete: bool,
}

pub fn assemble(
    relation_id: i64,
    name: Option<String>,
    ways: &HashMap<i64, Vec<i64>>,
    resolve: &mut CoordinateResolver<'_>,
) -> Result<AdminRegion, String> {
    let mut region = AdminRegion {
        relation_id,
        name,
        rings: Vec::new(),
        complete: true,
    };
    for chain in stitch(ways) {
        region.complete &= chain.len() >= 4 && chain.first() == chain.last();
        let points = resolve(&chain)?;
        if points.len() != chain.len() {
            return Err("Admin coordinate lookup changed reference count".into());
        }
        let mut run = Vec::new();
        for point in points {
            if let Some(point) = point {
                run.push(point);
            } else {
                region.complete = false;
                if run.len() >= 2 {
                    region.rings.push(std::mem::take(&mut run));
                }
                run.clear();
            }
        }
        if run.len() >= 2 {
            region.rings.push(run);
        }
    }
    Ok(region)
}

fn stitch(ways: &HashMap<i64, Vec<i64>>) -> Vec<Vec<i64>> {
    let mut ids: Vec<_> = ways.keys().copied().collect();
    ids.sort_unstable();
    let mut endpoints: HashMap<i64, Vec<i64>> = HashMap::new();
    for &id in &ids {
        let nodes = &ways[&id];
        if nodes.len() < 2 {
            continue;
        }
        endpoints.entry(nodes[0]).or_default().push(id);
        endpoints
            .entry(*nodes.last().unwrap())
            .or_default()
            .push(id);
    }
    let mut used = HashSet::new();
    let mut result = Vec::new();
    for id in ids {
        if !used.insert(id) || ways[&id].len() < 2 {
            continue;
        }
        let mut chain: VecDeque<i64> = ways[&id].iter().copied().collect();
        // Extend both ends: the first way may be in the middle of an open border.
        for front in [false, true] {
            loop {
                if chain.front() == chain.back() {
                    break;
                }
                let end = if front {
                    *chain.front().unwrap()
                } else {
                    *chain.back().unwrap()
                };
                let next = endpoints
                    .get(&end)
                    .and_then(|list| list.iter().find(|id| !used.contains(id)));
                let Some(&next) = next else { break };
                used.insert(next);
                let nodes = &ways[&next];
                let extension: Vec<_> = if nodes[0] == end {
                    nodes.iter().skip(1).copied().collect()
                } else {
                    nodes.iter().rev().skip(1).copied().collect()
                };
                for node in extension {
                    if front {
                        chain.push_front(node);
                    } else {
                        chain.push_back(node);
                    }
                }
            }
        }
        result.push(chain.into_iter().collect());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stitches_reversed_members_and_both_ends_without_coordinate_rounding() {
        let ways = HashMap::from([(1, vec![2, 3]), (2, vec![1, 2]), (3, vec![4, 3])]);
        assert_eq!(stitch(&ways), vec![vec![1, 2, 3, 4]]);
        let ways = HashMap::from([(1, vec![2, 3]), (2, vec![1, 2]), (3, vec![1, 4, 3])]);
        assert_eq!(stitch(&ways), vec![vec![2, 3, 4, 1, 2]]);
    }
    #[test]
    fn missing_node_splits_lines_and_disables_fill_for_entire_relation() {
        let ways = HashMap::from([(1, vec![1, 2, 3, 4, 5, 1])]);
        let region = assemble(10, None, &ways, &mut |ids| {
            Ok(ids
                .iter()
                .map(|id| {
                    (*id != 3).then_some(GeoPoint {
                        lat: *id as f32,
                        lon: 0.0,
                    })
                })
                .collect())
        })
        .unwrap();
        assert!(!region.complete);
        assert_eq!(
            region.rings.iter().map(Vec::len).collect::<Vec<_>>(),
            [2, 3]
        );
    }
}
