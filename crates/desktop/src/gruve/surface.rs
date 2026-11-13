//! One demand-driven JPEG of the host window, shared by all viewers.
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

const DEMAND_TIMEOUT: Duration = Duration::from_secs(2);
const PERIOD: Duration = Duration::from_micros(1_000_000 / 24);
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
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
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
    fn set_mode(&self, mode: Mode) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        if state.mode != mode {
            state.epoch += 1;
            state.frame = None;
            state.mode = mode;
            self.changed.notify_all();
        }
        (mode == Mode::Live && state.demand.is_some_and(|t| t.elapsed() < DEMAND_TIMEOUT))
            .then_some(state.epoch)
    }

    pub(super) fn live_epoch(&self) -> Option<u64> {
        let state = self.state.lock().unwrap();
        (state.mode == Mode::Live).then_some(state.epoch)
    }

    pub(super) fn poll(&self, seen: Option<u64>, wait: bool) -> Reply {
        let mut state = self.state.lock().unwrap();
        let previous = state.demand.replace(Instant::now());
        if state.mode == Mode::Live && previous.is_none_or(|t| t.elapsed() >= DEMAND_TIMEOUT) {
            crate::app::request_repaint();
        }
        if wait {
            let epoch = state.epoch;
            (state, _) = self
                .changed
                .wait_timeout_while(state, Duration::from_millis(600), |s| {
                    s.mode == Mode::Live
                        && s.epoch == epoch
                        && s.frame.as_ref().is_none_or(|(seq, _)| Some(*seq) == seen)
                })
                .unwrap();
        }
        state.reply(seen)
    }

    fn publish(&self, epoch: u64, jpeg: Vec<u8>) {
        let mut state = self.state.lock().unwrap();
        if state.mode != Mode::Live || state.epoch != epoch {
            return;
        }
        // Keep waiting guests on their current image while the window is still.
        if state
            .frame
            .as_ref()
            .is_some_and(|(_, last)| last.as_ref() == jpeg)
        {
            return;
        }
        state.sequence += 1;
        state.frame = Some((state.sequence, jpeg.into()));
        self.changed.notify_all();
    }
}

impl State {
    fn reply(&self, seen: Option<u64>) -> Reply {
        match (self.mode, &self.frame) {
            (Mode::Off, _) => Reply::Off,
            (Mode::Paused, _) => Reply::Paused,
            (Mode::Live, None) => Reply::Waiting,
            (Mode::Live, Some((seq, _))) if Some(*seq) == seen => Reply::Current,
            (Mode::Live, Some((seq, data))) => Reply::Frame(*seq, data.clone()),
        }
    }
}

struct Ticket(u64);
type Readback = (u64, Arc<egui::ColorImage>);

pub(super) struct Capture {
    state: Arc<Surface>,
    encoder: crossbeam_channel::Sender<Readback>,
    last_request: Option<Instant>,
    pending: bool,
}

impl Capture {
    pub(super) fn new(state: Arc<Surface>) -> Self {
        let (encoder, rx) = crossbeam_channel::bounded::<Readback>(1);
        let output = state.clone();
        std::thread::spawn(move || {
            for (epoch, image) in rx {
                if let Some(jpeg) = encode(&image) {
                    output.publish(epoch, jpeg);
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
        let Some(epoch) = self.state.set_mode(mode) else {
            self.pending = false;
            self.last_request = None;
            return;
        };
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
        let next = if self.pending {
            Duration::from_secs(1)
        } else {
            PERIOD
        };
        ctx.request_repaint_after(next.saturating_sub(self.last_request.unwrap().elapsed()));
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
    if (width, height) == (w, h) {
        for c in &image.pixels {
            rgb.extend_from_slice(&[c.r(), c.g(), c.b()]);
        }
    } else {
        for y in 0..height {
            let row = y * h / height * w;
            for x in 0..width {
                let c = image.pixels[row + x * w / width];
                rgb.extend_from_slice(&[c.r(), c.g(), c.b()]);
            }
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
    fn slow_viewer_gets_the_latest_frame_and_waiters_observe_pause() {
        let surface = Surface::default();
        surface.set_mode(Mode::Live);
        let epoch = surface.live_epoch().unwrap();
        for n in 1..=3 {
            surface.publish(epoch, vec![n]);
        }
        surface.publish(epoch, vec![3]);
        assert!(matches!(surface.poll(Some(3), false), Reply::Current));
        assert!(matches!(surface.poll(Some(1), true), Reply::Frame(3, data) if *data == [3]));

        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| surface.poll(Some(3), true));
            surface.set_mode(Mode::Paused);
            assert!(matches!(waiter.join().unwrap(), Reply::Paused));
        });
        surface.set_mode(Mode::Live);
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| surface.poll(None, true));
            surface.publish(surface.live_epoch().unwrap(), vec![4]);
            assert!(matches!(waiter.join().unwrap(), Reply::Frame(4, _)));
        });
    }

    #[test]
    fn settings_and_stop_invalidate_inflight_readbacks() {
        let state = Surface::default();
        assert!(matches!(state.poll(None, false), Reply::Off));
        state.set_mode(Mode::Live);
        let epoch = state.live_epoch().unwrap();
        assert!(matches!(state.poll(None, false), Reply::Waiting));
        state.publish(epoch, vec![1]);
        let Reply::Frame(seq, _) = state.poll(None, false) else {
            panic!("expected frame")
        };
        assert!(matches!(state.poll(Some(seq), false), Reply::Current));
        state.set_mode(Mode::Paused);
        state.publish(epoch, vec![2]);
        assert!(matches!(state.poll(None, false), Reply::Paused));
        state.set_mode(Mode::Live);
        state.publish(epoch, vec![3]);
        assert!(matches!(state.poll(None, false), Reply::Waiting));
        state.set_mode(Mode::Off);
        assert!(matches!(state.poll(None, false), Reply::Off));
    }
}
