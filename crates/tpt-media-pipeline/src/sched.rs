//! Realtime scheduling profile and latency observation.
//!
//! A realtime media pipeline commits to a worst-case latency per output
//! item. [`Pipeline::drive_scheduled`] measures each item's inter-arrival
//! time against a caller-provided [`Clock`], reports it through a
//! [`LatencyObserver`], and counts deadline misses and stalls in a
//! [`ScheduledRun`] report. Everything is clock-injected and deterministic
//! in tests via a manual clock.

use alloc::vec::Vec;

/// Wall-clock source for scheduling measurement.
pub trait Clock {
    /// Current time in nanoseconds.
    fn now_ns(&self) -> u64;
}

/// Receives per-item latency measurements.
pub trait LatencyObserver {
    /// Record the measured latency (nanoseconds) of one output item.
    fn observe(&mut self, latency_ns: u64);

    /// How many recorded latencies exceeded `max_latency_ns`.
    fn deadline_misses(&self, _max_latency_ns: u64) -> u64 {
        0
    }
}

/// Realtime scheduling commitment for a pipeline run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeProfile {
    /// Worst-case acceptable latency per output item, in nanoseconds.
    pub max_latency_ns: u64,
    /// Consecutive non-output steps tolerated before the run is declared a
    /// stall (see [`crate::pipeline::STALL_BOUND`]).
    pub stall_bound: usize,
}

impl RealtimeProfile {
    /// A conservative default: 16 ms budget (one 60 Hz frame) and the
    /// default stall bound.
    pub fn soft_realtime() -> Self {
        RealtimeProfile {
            max_latency_ns: 16_000_000,
            stall_bound: crate::pipeline::STALL_BOUND,
        }
    }
}

/// Outcome of a scheduled run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScheduledRun {
    /// Output items produced.
    pub items: u64,
    /// Items that exceeded the profile latency budget.
    pub deadline_misses: u64,
    /// Times the run stalled (exceeded the stall bound).
    pub stalls: u64,
}

/// A latency observer that records every sample (for analysis/steering).
#[derive(Debug, Clone, Default)]
pub struct LatencyLog {
    samples: Vec<u64>,
}

impl LatencyLog {
    /// All recorded latency samples, in emit order.
    pub fn samples(&self) -> &[u64] {
        &self.samples
    }
}

impl LatencyObserver for LatencyLog {
    fn observe(&mut self, latency_ns: u64) {
        self.samples.push(latency_ns);
    }

    fn deadline_misses(&self, max_latency_ns: u64) -> u64 {
        self.samples.iter().filter(|&&l| l > max_latency_ns).count() as u64
    }
}
