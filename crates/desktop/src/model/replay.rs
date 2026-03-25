use super::events::{EventRecord, EventSeverity};
use std::time::Instant;

/// A single event flare active during replay — carries its own fade timeline.
#[derive(Clone, Debug)]
pub struct ActiveFlare {
    pub event: EventRecord,
    /// Wall-clock seconds (total play time elapsed) at which this flare spawned.
    pub spawn_wall: f64,
    /// Wall-clock seconds this flare lives for.
    pub fade_duration: f32,
}

impl ActiveFlare {
    pub fn for_event(event: EventRecord, spawn_wall: f64) -> Self {
        let fade_duration = match event.severity {
            EventSeverity::Critical => 8.0,
            EventSeverity::Elevated => 4.5,
            EventSeverity::Advisory => 2.0,
        };
        Self {
            event,
            spawn_wall,
            fade_duration,
        }
    }

    /// Beam/marker alpha in `[0, 1]` given current total wall-clock play time.
    pub fn alpha(&self, wall_elapsed: f64) -> f32 {
        let age = (wall_elapsed - self.spawn_wall) as f32;
        if age < 0.0 {
            return 0.0;
        }
        let f = self.fade_duration;
        let ramp_in = 0.25f32;
        let ramp_out = (f * 0.22).max(0.5);
        let a = if age < ramp_in {
            age / ramp_in
        } else if age > f - ramp_out {
            ((f - age) / ramp_out).max(0.0)
        } else {
            1.0
        };
        a.clamp(0.0, 1.0)
    }

    /// Alpha for the one-shot expanding spawn ring.
    pub fn ring_alpha(&self, wall_elapsed: f64) -> f32 {
        let age = (wall_elapsed - self.spawn_wall) as f32;
        if age < 0.0 || age > 1.4 {
            return 0.0;
        }
        (1.0 - age / 1.4).clamp(0.0, 1.0)
    }

    /// Screen-space radius for the expanding spawn ring (pixels).
    pub fn ring_radius(&self, wall_elapsed: f64) -> f32 {
        let age = (wall_elapsed - self.spawn_wall) as f32;
        (age.max(0.0) * 55.0).min(80.0)
    }

    pub fn is_expired(&self, wall_elapsed: f64) -> bool {
        (wall_elapsed - self.spawn_wall) as f32 >= self.fade_duration
    }
}

/// Active replay session.
pub struct ReplayState {
    /// All events to replay, sorted oldest-first.
    pub events: Vec<(i64, EventRecord)>,
    /// Simulated time window [from, to] in unix seconds.
    pub sim_from: i64,
    pub sim_to: i64,
    /// Total wall-clock seconds for the full replay.
    pub wall_duration: f64,
    /// Wall-clock `Instant` when play was last (re)started; `None` if paused.
    play_start: Option<Instant>,
    /// Seconds of play time accumulated before the most recent pause.
    elapsed_before_pause: f64,
    /// Currently visible flares.
    pub active_flares: Vec<ActiveFlare>,
    /// Index into `events` of the next event to spawn.
    next_idx: usize,
}

impl ReplayState {
    pub fn new(
        events: Vec<(i64, EventRecord)>,
        sim_from: i64,
        sim_to: i64,
        wall_duration: f64,
    ) -> Self {
        Self {
            events,
            sim_from,
            sim_to,
            wall_duration,
            play_start: Some(Instant::now()),
            elapsed_before_pause: 0.0,
            active_flares: Vec::new(),
            next_idx: 0,
        }
    }

    /// Total wall-clock seconds of play time elapsed so far.
    pub fn wall_elapsed(&self) -> f64 {
        let live = self
            .play_start
            .map(|s| s.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        self.elapsed_before_pause + live
    }

    /// Simulated seconds elapsed within [sim_from, sim_to].
    fn sim_elapsed_secs(&self) -> f64 {
        let frac = (self.wall_elapsed() / self.wall_duration).clamp(0.0, 1.0);
        frac * (self.sim_to - self.sim_from) as f64
    }

    /// Current simulated unix timestamp.
    fn sim_now(&self) -> f64 {
        self.sim_from as f64 + self.sim_elapsed_secs()
    }

    /// Fraction through the replay `[0, 1]`.
    pub fn progress(&self) -> f32 {
        (self.wall_elapsed() / self.wall_duration).clamp(0.0, 1.0) as f32
    }

    pub fn is_paused(&self) -> bool {
        self.play_start.is_none()
    }

    pub fn is_finished(&self) -> bool {
        self.wall_elapsed() >= self.wall_duration
    }

    pub fn pause(&mut self) {
        if self.play_start.is_some() {
            self.elapsed_before_pause = self.wall_elapsed();
            self.play_start = None;
        }
    }

    pub fn resume(&mut self) {
        if self.play_start.is_none() {
            self.play_start = Some(Instant::now());
        }
    }

    pub fn restart(&mut self) {
        self.elapsed_before_pause = 0.0;
        self.play_start = Some(Instant::now());
        self.active_flares.clear();
        self.next_idx = 0;
    }

    /// Advance the playhead: spawn newly-reached events as flares, expire old ones.
    /// Returns `true` if the UI needs a repaint (flares are changing).
    pub fn tick(&mut self) -> bool {
        if self.is_paused() {
            return false;
        }
        let wall_now = self.wall_elapsed();
        let sim_now = self.sim_now();

        // Spawn events whose simulated time has been reached.
        while self.next_idx < self.events.len() {
            let (evt_unix, _) = &self.events[self.next_idx];
            if (*evt_unix as f64) <= sim_now {
                let (_, event) = self.events[self.next_idx].clone();
                self.active_flares
                    .push(ActiveFlare::for_event(event, wall_now));
                self.next_idx += 1;
            } else {
                break;
            }
        }

        // Expire flares past their fade duration.
        self.active_flares.retain(|f| !f.is_expired(wall_now));

        !self.is_finished()
    }
}
