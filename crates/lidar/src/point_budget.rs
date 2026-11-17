//! One resource envelope shared by every survey/pass in a Target load.
use std::{
    cell::Cell,
    time::{Duration, Instant},
};
#[derive(Debug)]
pub(crate) struct StreamBudget {
    pub deadline: Instant,
    pub decoded: Cell<u64>,
    pub nodes: Cell<usize>,
    pub transferred: Cell<u64>,
    pub max_decoded: u64,
    pub max_nodes: usize,
    pub max_transfer: u64,
}
impl Default for StreamBudget {
    fn default() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(180),
            decoded: Cell::new(0),
            nodes: Cell::new(0),
            transferred: Cell::new(0),
            max_decoded: 96_000_000,
            max_nodes: 4096,
            max_transfer: 256 * 1024 * 1024,
        }
    }
}
impl StreamBudget {
    pub fn limit(&self) -> Option<&'static str> {
        if Instant::now() >= self.deadline {
            Some("Combined loading time limit reached")
        } else if self.decoded.get() >= self.max_decoded {
            Some("Combined decoded-point limit reached")
        } else if self.nodes.get() >= self.max_nodes {
            Some("Combined node limit reached")
        } else if self.transferred.get() >= self.max_transfer {
            Some("Combined transfer limit reached")
        } else {
            None
        }
    }
    pub fn remaining_decoded(&self) -> usize {
        self.max_decoded
            .saturating_sub(self.decoded.get())
            .min(32768) as usize
    }
    pub fn remaining_transfer(&self) -> u64 {
        self.max_transfer.saturating_sub(self.transferred.get())
    }
    pub fn remaining_time(&self) -> Duration {
        self.deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
    }
}
