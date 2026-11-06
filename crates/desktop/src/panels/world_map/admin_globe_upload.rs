//! One GPU publication point for a globe boundary's lines and optional fill.
use super::super::line_upload::{Budget, Upload};
use super::{Appearance, Batch, InstanceChunk, SegmentInstance, Triangle};
use eframe::wgpu;

#[derive(Default)]
pub(super) struct Geometry {
    appearance: Option<Appearance>,
    pub displayed: Option<Resident>,
    pending: Option<Pending>,
}
pub(super) struct Resident {
    version: u64,
    pub lines: Vec<InstanceChunk>,
    pub fill: Option<Vec<InstanceChunk>>,
}
struct Pending {
    version: u64,
    shade: bool,
    // A shade toggle can reuse the resident lines of the same batch.
    lines: Option<Upload<SegmentInstance>>,
    fill: Option<Upload<Triangle>>,
}
impl Geometry {
    /// Finish a compatible in-flight pair before starting the next camera view.
    /// Returning false requests another frame under the shared upload budget.
    pub fn stage(
        &mut self,
        device: &wgpu::Device,
        budget: &mut Budget,
        batch: &Batch,
        shade: bool,
    ) -> bool {
        if self.appearance.as_ref() != Some(&batch.appearance) {
            *self = Self {
                appearance: Some(batch.appearance.clone()),
                ..Self::default()
            };
        }
        if self.pending.as_ref().is_some_and(|p| p.shade != shade) {
            self.pending = None;
        }
        if !shade && let Some(displayed) = &mut self.displayed {
            displayed.fill = None;
        }
        let needs_fill = shade && !batch.triangles.is_empty();
        let same_lines = self
            .displayed
            .as_ref()
            .is_some_and(|d| d.version == batch.version);
        if same_lines
            && self
                .displayed
                .as_ref()
                .is_some_and(|d| d.fill.is_some() == needs_fill)
        {
            self.pending = None; // Camera returned to the already displayed batch.
            return true;
        }
        let candidate = self.pending.get_or_insert_with(|| Pending {
            version: batch.version,
            shade,
            lines: (!same_lines).then(|| Upload::new(batch.lines.clone())),
            fill: needs_fill.then(|| Upload::new(batch.triangles.clone())),
        });
        if candidate
            .lines
            .as_mut()
            .is_some_and(|u| !u.advance(device, budget))
            || candidate
                .fill
                .as_mut()
                .is_some_and(|u| !u.advance(device, budget))
        {
            return false;
        }
        let ready = self.pending.take().unwrap();
        let lines = match ready.lines {
            Some(upload) => upload.chunks,
            None => {
                self.displayed
                    .take()
                    .expect("same-version lines are resident")
                    .lines
            }
        };
        let current = ready.version == batch.version && ready.fill.is_some() == needs_fill;
        self.displayed = Some(Resident {
            version: ready.version,
            lines,
            fill: ready.fill.map(|u| u.chunks),
        });
        current
    }
}
