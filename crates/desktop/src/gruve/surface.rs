//! One demand-driven JPEG of the host window, shared by all viewers.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const PERIOD: Duration = Duration::from_millis(160);
pub(super) const MAX_EDGE: u32 = 1600;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Mode {
    #[default]
    Off,
    Paused,
    Live,
}

#[derive(Default)]
pub(super) struct Surface {
    mode: Mode,
    epoch: u64,
    demand: Option<Instant>,
    frame: Option<(u64, Arc<[u8]>)>,
    sequence: u64,
}

pub(super) enum Reply {
    Off,
    Paused,
    Waiting,
    Current,
    Frame(u64, Arc<[u8]>),
}

impl Surface {
    fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.epoch += 1;
            self.frame = None;
            self.mode = mode;
        }
    }

    pub(super) fn live_epoch(&self) -> Option<u64> {
        (self.mode == Mode::Live).then_some(self.epoch)
    }

    pub(super) fn poll(&mut self, seen: Option<u64>) -> Reply {
        self.demand = Some(Instant::now());
        match (self.mode, &self.frame) {
            (Mode::Off, _) => Reply::Off,
            (Mode::Paused, _) => Reply::Paused,
            (Mode::Live, None) => Reply::Waiting,
            (Mode::Live, Some((seq, _))) if Some(*seq) == seen => Reply::Current,
            (Mode::Live, Some((seq, data))) => Reply::Frame(*seq, data.clone()),
        }
    }

    fn publish(&mut self, epoch: u64, jpeg: Vec<u8>) {
        if self.live_epoch() == Some(epoch) {
            self.sequence += 1;
            self.frame = Some((self.sequence, jpeg.into()));
        }
    }
}

struct Ticket(u64);
type Readback = (u64, Arc<egui::ColorImage>);

pub(super) struct Capture {
    state: Arc<Mutex<Surface>>,
    encoder: crossbeam_channel::Sender<Readback>,
    last_request: Option<Instant>,
    pending: bool,
}

impl Capture {
    pub(super) fn new(state: Arc<Mutex<Surface>>) -> Self {
        let (encoder, rx) = crossbeam_channel::bounded::<Readback>(1);
        let output = state.clone();
        std::thread::spawn(move || {
            for (epoch, image) in rx {
                if let Some(jpeg) = encode(&image) {
                    output.lock().unwrap().publish(epoch, jpeg);
                }
            }
        });
        Self {
            state,
            encoder,
            last_request: None,
            pending: false,
        }
    }

    pub(super) fn update(&mut self, ctx: &egui::Context, mode: Mode) {
        let mut state = self.state.lock().unwrap();
        state.set_mode(mode);
        let Some(epoch) = state.live_epoch() else {
            return;
        };
        if !state
            .demand
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        drop(state);

        ctx.input(|input| {
            for event in &input.raw.events {
                if let egui::Event::Screenshot {
                    user_data, image, ..
                } = event
                    && let Some(ticket) = user_data
                        .data
                        .as_ref()
                        .and_then(|d| d.downcast_ref::<Ticket>())
                {
                    self.pending = false;
                    if ticket.0 == epoch {
                        let _ = self.encoder.try_send((epoch, image.clone()));
                    }
                }
            }
        });
        // eframe delivers the readback on a later repaint. Recover if the
        // platform discarded the request during a resize or minimization.
        let elapsed = self.last_request.map_or(Duration::MAX, |t| t.elapsed());
        if elapsed >= PERIOD && (!self.pending || elapsed >= Duration::from_secs(1)) {
            self.last_request = Some(Instant::now());
            self.pending = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                Ticket(epoch),
            )));
            ctx.request_repaint();
        }
        ctx.request_repaint_after(PERIOD);
    }
}

fn encode(image: &egui::ColorImage) -> Option<Vec<u8>> {
    let [w, h] = image.size;
    if w == 0 || h == 0 {
        return None;
    }
    // Bound both the CPU copy and the wire image even on a Retina/4K host.
    let divisor = w.max(h).max(MAX_EDGE as usize);
    let (width, height) = (
        (w * MAX_EDGE as usize / divisor).max(1),
        (h * MAX_EDGE as usize / divisor).max(1),
    );
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let row = y * h / height * w;
        for x in 0..width {
            let c = image.pixels[row + x * w / width];
            rgb.extend_from_slice(&[c.r(), c.g(), c.b()]);
        }
    }
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 78)
        .encode(
            &rgb,
            width as u32,
            height as u32,
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_stop_invalidate_inflight_readbacks() {
        let mut state = Surface::default();
        assert!(matches!(state.poll(None), Reply::Off));
        state.set_mode(Mode::Live);
        let epoch = state.live_epoch().unwrap();
        assert!(matches!(state.poll(None), Reply::Waiting));
        state.publish(epoch, vec![1]);
        let Reply::Frame(seq, _) = state.poll(None) else {
            panic!("expected frame")
        };
        assert!(matches!(state.poll(Some(seq)), Reply::Current));
        state.set_mode(Mode::Paused);
        state.publish(epoch, vec![2]);
        assert!(matches!(state.poll(None), Reply::Paused));
        state.set_mode(Mode::Live);
        state.publish(epoch, vec![3]);
        assert!(matches!(state.poll(None), Reply::Waiting));
        state.set_mode(Mode::Off);
        assert!(matches!(state.poll(None), Reply::Off));
    }
}
