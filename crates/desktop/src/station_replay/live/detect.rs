// Compare short- and long-term averages of absolute sample differences. Each
// channel keeps its own baseline, so instrument gain cancels from the ratio.
use miniseed_rs::{MseedRecord, Samples};

#[derive(Default)]
pub(super) struct Detector {
    pub last_ms: i64,
    rate: f64,
    previous: Option<f64>,
    short: f64,
    long: f64,
    warm_s: f64,
    triggered: bool,
}

#[derive(Clone, Copy)]
pub(super) struct Motion {
    pub time_ms: i64,
    pub ratio: f64,
}

impl Detector {
    pub fn record(&mut self, record: &MseedRecord, start_ms: i64) -> Option<Motion> {
        let rate = record.sample_rate;
        let dt = 1.0 / rate;
        let period_ms = dt * 1000.0;
        if self.previous.is_some() {
            // Retransmitted samples cannot create another onset. A gap or a
            // changed sampling rate invalidates the amplitude history.
            if start_ms as f64 <= self.last_ms as f64 + 0.25 * period_ms {
                return None;
            }
            if rate != self.rate || (start_ms - self.last_ms) as f64 > period_ms * 1.6 + 1.0 {
                *self = Self::default();
            }
        }
        self.rate = rate;
        let fast = -(-dt / 1.0).exp_m1();
        let slow = -(-dt / 45.0).exp_m1();
        let mut hit = None;
        let mut sample = |index: usize, value: f64| {
            self.last_ms = start_ms + (index as f64 * period_ms).round() as i64;
            if !value.is_finite() {
                self.previous = None;
                self.warm_s = 0.;
                self.triggered = false;
                return;
            }
            let Some(last) = self.previous.replace(value) else {
                return;
            };
            let amplitude = (value - last).abs();
            if self.warm_s == 0.0 {
                self.short = amplitude;
                self.long = amplitude;
            }
            self.short += fast * (amplitude - self.short);
            self.long += slow * (amplitude - self.long);
            self.warm_s += dt;
            if self.warm_s < 45.0 || self.long <= f64::EPSILON {
                return;
            }
            let ratio = self.short / self.long;
            if !self.triggered && ratio >= 4.5 {
                self.triggered = true;
                hit = Some(Motion {
                    time_ms: self.last_ms,
                    ratio,
                });
            } else if ratio < 1.8 {
                self.triggered = false;
            }
        };
        match &record.samples {
            Samples::Int(v) => v
                .iter()
                .enumerate()
                .for_each(|(i, &x)| sample(i, f64::from(x))),
            Samples::Float(v) => v
                .iter()
                .enumerate()
                .for_each(|(i, &x)| sample(i, f64::from(x))),
            Samples::Double(v) => v.iter().enumerate().for_each(|(i, &x)| sample(i, x)),
        }
        hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(seconds: usize, amplitude: i32) -> MseedRecord {
        MseedRecord::new()
            .with_sample_rate(20.0)
            .with_samples(Samples::Int(
                (0..20 * seconds)
                    .map(|i| if i % 2 == 0 { amplitude } else { -amplitude })
                    .collect(),
            ))
    }

    #[test]
    fn detects_increased_motion_after_warmup_and_resets_after_gaps() {
        let mut detector = Detector::default();
        let quiet = wave(60, 3);
        let loud = wave(2, 300);
        assert!(detector.record(&quiet, 0).is_none());
        let onset = detector.record(&loud, 60_000).unwrap();
        assert!((60_000..61_000).contains(&onset.time_ms));
        assert!(detector.record(&loud, 60_000).is_none());
        assert!(detector.record(&loud, 90_000).is_none());
        assert!(detector.record(&quiet, 92_000).is_none());
    }
}
