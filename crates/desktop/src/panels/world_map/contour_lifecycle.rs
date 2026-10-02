//! Drop CPU/GPU globe line layers that were not requested by this frame.
use super::*;
use std::collections::HashSet;

static REQUESTED: OnceLock<Mutex<HashSet<ContourLayer>>> = OnceLock::new();

pub(crate) fn begin_frame() {
    REQUESTED
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .clear();
}

pub(super) fn requested(layer: ContourLayer) {
    REQUESTED
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(layer);
}

pub(crate) fn end_frame() {
    let keep = REQUESTED
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .clone();
    release_unused(&mut instance_cache().lock().unwrap(), &keep);
}

fn release_unused(cache: &mut HashMap<ContourLayer, LayerInstances>, keep: &HashSet<ContourLayer>) {
    for (layer, entry) in cache {
        entry.active = keep.contains(layer);
        if !entry.active {
            entry.current = None;
            entry.source = None;
        }
        // Keep the worker's slot until it finishes. It sees active=false and
        // discards its output; repeated mode changes cannot fan out workers.
    }
}

pub(crate) fn residency_callback(rect: egui::Rect) -> egui::PaintCallback {
    egui_wgpu::Callback::new_paint_callback(rect, Cleanup)
}

struct Cleanup;
impl egui_wgpu::CallbackTrait for Cleanup {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get_mut::<ContourPassResources>() {
            res.used.clear();
        }
        Vec::new()
    }

    fn finish_prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get_mut::<ContourPassResources>() {
            res.layers.retain(|layer, _| res.used.contains(layer));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
    }
}

#[cfg(test)]
#[path = "contour_lifecycle_gpu_tests.rs"]
mod gpu_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_layers_release_instances_and_late_workers_cannot_restore_them() {
        let data = Arc::new(vec![SegmentInstance::line(
            GeoPoint { lat: 0.0, lon: 0.0 },
            GeoPoint { lat: 0.0, lon: 1.0 },
            egui::Color32::WHITE,
        )]);
        let weak = Arc::downgrade(&data);
        let mut cache = HashMap::from([(
            ContourLayer::SrtmGlobe,
            LayerInstances {
                current: Some((1, data)),
                building: Some(2),
                active: true,
                ..Default::default()
            },
        )]);
        release_unused(&mut cache, &HashSet::new());
        assert!(weak.upgrade().is_none());
        let entry = cache.get_mut(&ContourLayer::SrtmGlobe).unwrap();
        assert_eq!(entry.building, Some(2));
        complete_instance_build(entry, 2, Some(Vec::new()));
        assert!(entry.current.is_none());
        assert!(entry.building.is_none());
    }
}
