//! Unified Master UTC Timeline synchronizing sunlight, weather imagery,
//! event replays, seismic station days, and Follow Events regional recaps.

use crate::stellar_time;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClockMode {
    /// Tracks real-time UTC wall clock.
    Live,
    /// Scrubbed or playing inside an explicit `[from_ms, to_ms]` UTC window.
    Playback {
        from_ms: i64,
        to_ms: i64,
        cursor_ms: i64,
        rate: f64,
        playing: bool,
        looping: bool,
    },
    /// Driven passively by an active replay session (e.g. RegionalRecap, ReplayState, StationDay).
    External { cursor_ms: i64 },
}

#[derive(Clone, Debug)]
pub struct MasterClock {
    mode: ClockMode,
    last_live_ms: i64,
    /// When true, scrubbing/playing any timeline automatically synchronizes
    /// Atlas sunlight and Global Weather satellite frames to the active UTC clock.
    pub sync_subsystems: bool,
}

impl Default for MasterClock {
    fn default() -> Self {
        let now_ms = chrono::Utc::now().timestamp_millis();
        Self {
            mode: ClockMode::Live,
            last_live_ms: now_ms,
            sync_subsystems: true,
        }
    }
}

impl MasterClock {
    pub fn mode(&self) -> ClockMode {
        self.mode
    }

    pub fn is_live(&self) -> bool {
        matches!(self.mode, ClockMode::Live)
    }

    pub fn utc_ms(&self) -> i64 {
        match self.mode {
            ClockMode::Live => self.last_live_ms,
            ClockMode::Playback { cursor_ms, .. } | ClockMode::External { cursor_ms } => cursor_ms,
        }
    }

    pub fn utc_secs(&self) -> f64 {
        self.utc_ms() as f64 / 1000.0
    }

    pub fn jd(&self) -> f64 {
        stellar_time::unix_to_jd(self.utc_secs())
    }

    pub fn formatted_utc(&self) -> String {
        chrono::DateTime::from_timestamp_millis(self.utc_ms())
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "— UTC".into())
    }

    pub fn set_live(&mut self) {
        self.mode = ClockMode::Live;
    }

    pub fn set_external(&mut self, cursor_ms: i64) {
        self.mode = ClockMode::External { cursor_ms };
    }

    pub fn open_window(&mut self, from_ms: i64, to_ms: i64, rate: f64, looping: bool) {
        let (a, b) = if from_ms <= to_ms {
            (from_ms, to_ms)
        } else {
            (to_ms, from_ms)
        };
        self.mode = ClockMode::Playback {
            from_ms: a,
            to_ms: b,
            cursor_ms: a,
            rate: rate.max(0.0),
            playing: true,
            looping,
        };
    }

    pub fn seek_fraction(&mut self, fraction: f32) {
        if let ClockMode::Playback {
            from_ms,
            to_ms,
            cursor_ms,
            ..
        } = &mut self.mode
        {
            let f = fraction.clamp(0.0, 1.0) as f64;
            *cursor_ms = *from_ms + ((*to_ms - *from_ms) as f64 * f).round() as i64;
        }
    }

    pub fn toggle_play(&mut self) {
        if let ClockMode::Playback { playing, .. } = &mut self.mode {
            *playing = !*playing;
        }
    }

    /// Advance the clock by `dt_secs` wall seconds. Returns `true` if a playback
    /// cursor moved and the UI should repaint.
    pub fn tick(&mut self, dt_secs: f64, now_utc_ms: i64) -> bool {
        self.last_live_ms = now_utc_ms;
        match &mut self.mode {
            ClockMode::Live | ClockMode::External { .. } => false,
            ClockMode::Playback {
                from_ms,
                to_ms,
                cursor_ms,
                rate,
                playing,
                looping,
            } => {
                if !*playing || *rate <= 0.0 || dt_secs <= 0.0 {
                    return false;
                }
                let advance_ms = (dt_secs * *rate * 1000.0).round() as i64;
                if advance_ms == 0 {
                    return false;
                }
                let next = cursor_ms.saturating_add(advance_ms);
                if next >= *to_ms {
                    if *looping && *to_ms > *from_ms {
                        let span = *to_ms - *from_ms;
                        *cursor_ms = *from_ms + ((next - *from_ms).rem_euclid(span));
                    } else {
                        *cursor_ms = *to_ms;
                        *playing = false;
                    }
                } else {
                    *cursor_ms = next;
                }
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_clock_tracks_live_playback_and_external_modes() {
        let mut clock = MasterClock::default();
        assert!(clock.is_live());
        clock.tick(0.1, 1_700_000_000_000);
        assert_eq!(clock.utc_ms(), 1_700_000_000_000);

        clock.open_window(1_000_000, 1_010_000, 2.0, true);
        assert!(!clock.is_live());
        assert!(clock.tick(3.0, 1_700_000_003_000));
        assert_eq!(clock.utc_ms(), 1_006_000);
        // Wraps cleanly when looping
        assert!(clock.tick(3.0, 1_700_000_006_000));
        assert_eq!(clock.utc_ms(), 1_002_000);

        clock.set_external(1_500_000_000_000);
        assert_eq!(clock.utc_ms(), 1_500_000_000_000);
        assert!((clock.jd() - stellar_time::unix_to_jd(1_500_000_000.0)).abs() < 1e-9);
    }
}
