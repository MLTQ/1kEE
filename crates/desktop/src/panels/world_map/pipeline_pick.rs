//! Camera-independent BVH of the exact segments sent to the globe GPU renderer.
use super::{contour_pass::SegmentInstance, globe_scene::GlobeLayout, infrastructure_hover};
use crate::model::GlobeViewState;
use egui::{Pos2, Rect};

struct Node {
    min: [f32; 3],
    max: [f32; 3],
    start: usize,
    len: usize,
    children: Option<[usize; 2]>,
}
pub(super) struct Index {
    nodes: Vec<Node>,
    order: Vec<usize>,
    owners: Vec<usize>,
}
impl Index {
    pub(super) fn new(segments: &[SegmentInstance], owners: Vec<usize>) -> Self {
        assert_eq!(segments.len(), owners.len());
        let mut index = Self {
            nodes: Vec::new(),
            order: (0..segments.len()).collect(),
            owners,
        };
        if !segments.is_empty() {
            build(&mut index.nodes, &mut index.order, 0, segments);
        }
        index
    }
    pub(super) fn query(
        &self,
        segments: &[SegmentInstance],
        layout: &GlobeLayout,
        view: &GlobeViewState,
        pointer: Pos2,
        mut hit: impl FnMut(usize, f32),
    ) -> usize {
        if self.nodes.is_empty() {
            return 0;
        }
        let project = Projection::new(layout, view);
        let mut stack = vec![0];
        let mut tested = 0;
        while let Some(id) = stack.pop() {
            let node = &self.nodes[id];
            if !project.contains(node, pointer) {
                continue;
            }
            if let Some(children) = node.children {
                stack.extend(children);
                continue;
            }
            for &i in &self.order[node.start..node.start + node.len] {
                tested += 1;
                let [a, b] = segments[i].endpoints();
                if let (Some(a), Some(b)) = (project.point(a), project.point(b)) {
                    let distance = infrastructure_hover::line_distance(pointer, a, b);
                    if distance <= infrastructure_hover::RADIUS {
                        hit(self.owners[i], distance);
                    }
                }
            }
        }
        tested
    }
}
fn build(
    nodes: &mut Vec<Node>,
    order: &mut [usize],
    offset: usize,
    segments: &[SegmentInstance],
) -> usize {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for &i in order.iter() {
        for p in segments[i].endpoints() {
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
    }
    let id = nodes.len();
    nodes.push(Node {
        min,
        max,
        start: offset,
        len: order.len(),
        children: None,
    });
    if order.len() > 16 {
        let axis = (0..3)
            .max_by(|&a, &b| (max[a] - min[a]).total_cmp(&(max[b] - min[b])))
            .unwrap();
        let mid = order.len() / 2;
        let center = |i: usize| {
            let [a, b] = segments[i].endpoints();
            a[axis] + b[axis]
        };
        order.select_nth_unstable_by(mid, |&a, &b| center(a).total_cmp(&center(b)));
        let (left, right) = order.split_at_mut(mid);
        let children = [
            build(nodes, left, offset, segments),
            build(nodes, right, offset + mid, segments),
        ];
        nodes[id].children = Some(children);
    }
    id
}
struct Projection {
    layout: GlobeLayout,
    sy: f32,
    cy: f32,
    sp: f32,
    cp: f32,
}
impl Projection {
    fn new(layout: &GlobeLayout, view: &GlobeViewState) -> Self {
        Self {
            layout: *layout,
            sy: view.yaw.sin(),
            cy: view.yaw.cos(),
            sp: view.pitch.sin(),
            cp: view.pitch.cos(),
        }
    }
    fn rotate(&self, p: [f32; 3]) -> [f32; 3] {
        let x = p[0] * self.cy + p[2] * self.sy;
        let z = -p[0] * self.sy + p[2] * self.cy;
        [
            x,
            p[1] * self.cp - z * self.sp,
            p[1] * self.sp + z * self.cp,
        ]
    }
    fn screen(&self, p: [f32; 3]) -> Pos2 {
        let scale =
            self.layout.radius * self.layout.focal_length / (self.layout.camera_distance - p[2]);
        egui::pos2(
            self.layout.center.x - p[0] * scale,
            self.layout.center.y - p[1] * scale,
        )
    }
    fn point(&self, p: [f32; 3]) -> Option<Pos2> {
        let p = self.rotate(p);
        // Same whole-segment horizon/near-plane rejection as contour_lines.wgsl.
        (self.layout.camera_distance - p[2] > 0.05 && p[2] >= 1.0 / self.layout.camera_distance)
            .then(|| self.screen(p))
    }
    fn contains(&self, node: &Node, pointer: Pos2) -> bool {
        let mut bounds = Rect::NOTHING;
        let mut max_z = f32::NEG_INFINITY;
        let mut near = false;
        for corner in 0..8 {
            let p = self.rotate(std::array::from_fn(|axis| {
                if corner & (1 << axis) == 0 {
                    node.min[axis]
                } else {
                    node.max[axis]
                }
            }));
            max_z = max_z.max(p[2]);
            if self.layout.camera_distance - p[2] <= 0.05 {
                near = true;
            } else {
                bounds.extend_with(self.screen(p));
            }
        }
        // Linear perspective extrema on a box occur at corners when w>0.
        // Near-plane intersections remain candidates; leaf tests are exact.
        max_z >= 1.0 / self.layout.camera_distance
            && (near
                || bounds
                    .expand(infrastructure_hover::RADIUS + 0.01)
                    .contains(pointer))
    }
}

#[cfg(test)]
#[path = "pipeline_pick_tests.rs"]
mod tests;
