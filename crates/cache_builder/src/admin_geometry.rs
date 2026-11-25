//! Exact OSM node-ID stitching, shared by focused and whole-planet builds.
use crate::util::GeoPoint;
use std::collections::{HashMap, VecDeque};

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
    let chains = stitch(ways);
    let refs: Vec<_> = chains.iter().flatten().copied().collect();
    // Resolve all rings together: country islands often refer to the same PBF
    // blocks. Per-ring lookups repeatedly decompress those blocks after eviction.
    let resolved = resolve(&refs)?;
    build_region_from_chains(relation_id, name, chains, &resolved)
}

pub(super) fn stitch_chains(ways: &HashMap<i64, Vec<i64>>) -> Vec<Vec<i64>> {
    stitch(ways)
}

pub(super) fn build_region_from_chains(
    relation_id: i64,
    name: Option<String>,
    chains: Vec<Vec<i64>>,
    resolved: &[Option<GeoPoint>],
) -> Result<AdminRegion, String> {
    let total_refs: usize = chains.iter().map(Vec::len).sum();
    if resolved.len() != total_refs {
        return Err("Admin coordinate lookup changed reference count".into());
    }
    let mut region = AdminRegion {
        relation_id,
        name,
        rings: Vec::with_capacity(chains.len()),
        complete: true,
    };
    let mut cursor = 0usize;
    for chain in chains {
        let len = chain.len();
        region.complete &= len >= 4 && chain.first() == chain.last();
        let mut run = Vec::new();
        for &point in &resolved[cursor..cursor + len] {
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
        cursor += len;
    }
    Ok(region)
}

fn stitch(ways: &HashMap<i64, Vec<i64>>) -> Vec<Vec<i64>> {
    let mut sorted: Vec<(i64, &[i64])> = ways
        .iter()
        .filter(|(_, nodes)| nodes.len() >= 2)
        .map(|(&id, nodes)| (id, nodes.as_slice()))
        .collect();
    sorted.sort_unstable_by_key(|&(id, _)| id);

    let n = sorted.len();
    let mut endpoints: HashMap<i64, Vec<usize>> = HashMap::with_capacity(n * 2);
    for (idx, &(_, nodes)) in sorted.iter().enumerate() {
        endpoints.entry(nodes[0]).or_default().push(idx);
        endpoints
            .entry(*nodes.last().unwrap())
            .or_default()
            .push(idx);
    }

    let mut used = vec![false; n];
    let mut result = Vec::new();
    for start_idx in 0..n {
        if used[start_idx] {
            continue;
        }
        used[start_idx] = true;
        let first_nodes = sorted[start_idx].1;
        let mut chain: VecDeque<i64> = VecDeque::with_capacity(first_nodes.len());
        chain.extend(first_nodes.iter().copied());
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
                let Some(&next_idx) = endpoints
                    .get(&end)
                    .and_then(|list| list.iter().find(|&&idx| !used[idx]))
                else {
                    break;
                };
                used[next_idx] = true;
                let nodes = sorted[next_idx].1;
                if nodes[0] == end {
                    if front {
                        for &node in &nodes[1..] {
                            chain.push_front(node);
                        }
                    } else {
                        chain.extend(nodes[1..].iter().copied());
                    }
                } else if front {
                    for &node in nodes[..nodes.len() - 1].iter().rev() {
                        chain.push_front(node);
                    }
                } else {
                    chain.extend(nodes[..nodes.len() - 1].iter().rev().copied());
                }
            }
        }
        result.push(Vec::from(chain));
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
