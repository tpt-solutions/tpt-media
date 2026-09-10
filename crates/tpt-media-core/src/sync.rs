//! Clock domains and cross-stream synchronization.
//!
//! See `todo.md` §20 ("Time") for the items "Define clock domains" and
//! "Define synchronization model". This module is intentionally minimal
//! and `no_std`-friendly: it defines *types* and *arithmetic*, not
//! scheduling policy. A scheduler/playout queue is a higher-level
//! concern (and lives in `tpt-media-pipeline`, not here).
//!
//! ## Clock domains
//!
//! A [`ClockDomain`] is a named pair of `(name, Timebase)` that
//! identifies a single, internally-consistent time origin. Every track
//! in a stream belongs to exactly one clock domain, identified by the
//! track's `clock_domain` name. Two timestamps that share a clock
//! domain can be compared directly (via [`Timestamp::rescale`]); two
//! timestamps that *don't* share one need an explicit mapping.
//!
//! The names are *opaque strings* — two tracks whose `clock_domain`
//! values compare equal are in the same domain; two that compare
//! unequal are not, even if their `Timebase`s happen to be the same
//! (because the *origins* are different). A `Wallclock` track at
//! `t=0` and a `Mic-Input` track at `t=0` are not the same `t=0`.
//!
//! ## Synchronization model
//!
//! A [`SyncPoint`] pins a single moment in one domain to a single
//! moment in another. A [`SyncMap`] collects one or more such pins
//! and translates timestamps between the mapped domains by linear
//! interpolation between the nearest two pins. Extrapolation outside
//! the spanned range is rejected (see [`SyncMap::translate`] for the
//! rationale).

use alloc::string::String;

use crate::timebase::{Timebase, Timestamp};

/// A named clock domain: an opaque identity (`name`) plus the
/// [`Timebase`] used to count ticks inside that domain.
///
/// `name` is the only thing that determines *which* domain a track
/// belongs to; two `ClockDomain`s with the same name are the same
/// domain regardless of their `timebase` field. (In practice the
/// `timebase` field should match too — but equality is decided
/// strictly by name so that callers can't accidentally equate two
/// accidentally-similar-but-distinct domains.)
#[derive(Clone, Debug)]
pub struct ClockDomain {
    /// Opaque identifier. Compared by `==`.
    pub name: String,
    /// Timebase for ticks inside this domain.
    pub timebase: Timebase,
}

impl PartialEq for ClockDomain {
    fn eq(&self, other: &Self) -> bool {
        // Equality is *name only*. The `timebase` field is stored and
        // used for arithmetic; it does not determine domain identity.
        self.name == other.name
    }
}

impl Eq for ClockDomain {}

impl core::hash::Hash for ClockDomain {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        // Hash only `name`, to stay consistent with `PartialEq` (which
        // also ignores `timebase`). Hashing `timebase` too would violate
        // the `Hash`/`Eq` contract: two domains that compare equal could
        // hash differently.
        self.name.hash(state);
    }
}

impl ClockDomain {
    /// Build a new clock domain. The name is stored as-is; the
    /// `timebase` is stored by value.
    pub fn new(name: impl Into<String>, timebase: Timebase) -> Self {
        ClockDomain {
            name: name.into(),
            timebase,
        }
    }
}

/// A single cross-domain mapping pin: "this moment in `from`
/// coincides with that moment in `to`".
///
/// `from_ticks` and `to_ticks` are raw signed tick counts in the
/// respective domains' timebases; both are converted to seconds for
/// the linear-interpolation math.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SyncPoint {
    /// Source domain (whose `timebase` matches `from_ticks`).
    pub from: ClockDomain,
    /// Tick count in `from`'s timebase.
    pub from_ticks: i64,
    /// Destination domain (whose `timebase` matches `to_ticks`).
    pub to: ClockDomain,
    /// Tick count in `to`'s timebase.
    pub to_ticks: i64,
}

impl SyncPoint {
    /// `from_ticks` as a [`Timestamp`] in `from`'s timebase.
    pub fn from_timestamp(&self) -> Timestamp {
        Timestamp {
            ticks: self.from_ticks,
            timebase: self.from.timebase,
        }
    }

    /// `to_ticks` as a [`Timestamp`] in `to`'s timebase.
    pub fn to_timestamp(&self) -> Timestamp {
        Timestamp {
            ticks: self.to_ticks,
            timebase: self.to.timebase,
        }
    }
}

/// A set of [`SyncPoint`]s defining how to translate timestamps
/// across clock domains.
///
/// Pins are kept in the order they were inserted (no automatic
/// sorting); [`SyncMap::translate`] walks them in stored order.
/// Callers who care about performance on long streams should keep the
/// count small (O(n) per translate) — typical use is one pin at stream
/// start plus a handful of mid-stream corrections.
///
/// Pins are intentionally **homogeneous** in the sense that a
/// single `SyncMap` is meant to translate *between two specific
/// domains* (a `from` and a `to`). The struct does not enforce that
/// — mixing in pins that pair different domain pairs will produce
/// nonsensical interpolation results — but it documents the
/// expectation and tests guard against accidentally using it that
/// way.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncMap {
    pins: alloc::vec::Vec<SyncPoint>,
}

impl SyncMap {
    /// Empty map. Translating through an empty map returns `None`
    /// for every input.
    pub fn new() -> Self {
        SyncMap {
            pins: alloc::vec::Vec::new(),
        }
    }

    /// Add a pin to the map. Pins are appended in insertion order;
    /// callers are responsible for inserting them in
    /// monotonically-increasing `from_ticks` / `to_ticks` order if
    /// they want `translate` to interpolate rather than backtrack.
    pub fn add(&mut self, pin: SyncPoint) {
        self.pins.push(pin);
    }

    /// Number of pins currently in the map.
    pub fn len(&self) -> usize {
        self.pins.len()
    }

    /// True if no pins have been added.
    pub fn is_empty(&self) -> bool {
        self.pins.is_empty()
    }

    /// Translate a [`Timestamp`] from `pin.from`'s domain to
    /// `pin.to`'s domain.
    ///
    /// Returns:
    ///
    /// - `Some(Ok(Timestamp))` for a successful interpolation
    ///   between the two surrounding pins.
    /// - `Some(Err(SyncError::OutOfRange))` if the input timestamp is
    ///   **before the first pin** *or* **after the last pin**.
    ///   Extrapolation is deliberately rejected: sync drift between
    ///   domains is a real phenomenon, and extrapolating beyond the
    ///   last known pin is a guaranteed source of A/V drift bugs.
    ///   Callers who genuinely need to extrapolate should add another
    ///   pin first.
    /// - `Some(Err(SyncError::DomainMismatch))` if the input
    ///   timestamp's timebase doesn't match any pin's `from`
    ///   timebase (or any `from` domain at all).
    /// - `None` if the map is empty.
    pub fn translate(&self, ts: Timestamp) -> Option<core::result::Result<Timestamp, SyncError>> {
        let first = self.pins.first()?;
        if ts.timebase != first.from.timebase {
            return Some(Err(SyncError::DomainMismatch));
        }

        let in_seconds = ts.to_seconds();

        // Linear scan to find the surrounding pair: the largest pin
        // whose `from_ticks` ≤ `ts`, plus the next one. O(n) but `n`
        // is expected to be small (single digits).
        let mut prev: Option<&SyncPoint> = None;
        let mut next: Option<&SyncPoint> = None;
        for pin in &self.pins {
            let pin_secs = pin.from_timestamp().to_seconds();
            if pin_secs <= in_seconds {
                prev = Some(pin);
            } else {
                next = Some(pin);
                break;
            }
        }

        let (prev, next) = match (prev, next) {
            (Some(p), Some(n)) => (p, n),
            // Exact match on the last pin.
            (Some(p), None) if p.from_timestamp().to_seconds() == in_seconds => {
                return Some(Ok(p.to_timestamp()));
            }
            // Out of range: before the first or after the last.
            (None, _) => return Some(Err(SyncError::OutOfRange)),
            (Some(_), None) => return Some(Err(SyncError::OutOfRange)),
        };

        let prev_from_secs = prev.from_timestamp().to_seconds();
        let prev_to_secs = prev.to_timestamp().to_seconds();
        let next_from_secs = next.from_timestamp().to_seconds();
        let next_to_secs = next.to_timestamp().to_seconds();

        // Linear interpolation. Zero-length interval (duplicate pin)
        // → fall back to prev pin's `to` value, no division by zero.
        let span = next_from_secs - prev_from_secs;
        let out_secs = if span.abs() < f64::EPSILON {
            prev_to_secs
        } else {
            let frac = (in_seconds - prev_from_secs) / span;
            prev_to_secs + frac * (next_to_secs - prev_to_secs)
        };

        // Convert seconds back into `next.to.timebase` ticks via
        // `f64` (we are at an API boundary; see
        // `timebase::Timestamp::to_seconds`'s own justification).
        let target_tb = next.to.timebase;
        let ticks_f = out_secs * (target_tb.den as f64) / (target_tb.num as f64);
        let ticks = if !ticks_f.is_finite() {
            return Some(Err(SyncError::OutOfRange));
        } else if ticks_f >= i64::MAX as f64 {
            i64::MAX
        } else if ticks_f <= i64::MIN as f64 {
            i64::MIN
        } else {
            libm::round(ticks_f) as i64
        };

        Some(Ok(Timestamp {
            ticks,
            timebase: target_tb,
        }))
    }
}

/// Errors a [`SyncMap::translate`] call can surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncError {
    /// Input timestamp's timebase doesn't match the map's source
    /// domain.
    DomainMismatch,
    /// Input timestamp falls outside the spanned range (before the
    /// first pin or after the last). No extrapolation is performed;
    /// see [`SyncMap::translate`] for the rationale.
    OutOfRange,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio() -> ClockDomain {
        ClockDomain::new("audio-pulse", Timebase::new(1, 48_000))
    }
    fn video() -> ClockDomain {
        ClockDomain::new("video-90khz", Timebase::NINETY_KHZ)
    }
    fn wallclock() -> ClockDomain {
        ClockDomain::new("wallclock", Timebase::ONE_PER_SECOND)
    }

    #[test]
    fn clock_domain_equality_is_by_name_only() {
        let a = ClockDomain::new("audio-pulse", Timebase::new(1, 48_000));
        let b = ClockDomain::new("audio-pulse", Timebase::MILLISECONDS);
        assert_eq!(a, b);
        let c = ClockDomain::new("video-90khz", Timebase::new(1, 48_000));
        assert_ne!(a, c);
    }

    #[test]
    fn translate_empty_map_returns_none() {
        let m = SyncMap::new();
        let ts = Timestamp {
            ticks: 1000,
            timebase: audio().timebase,
        };
        assert!(m.translate(ts).is_none());
    }

    #[test]
    fn translate_exact_pin_match() {
        // Pin: 48 000 ticks at 48 kHz == 90 000 ticks at 90 kHz
        // (both = 1 second).
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 48_000,
            to: video(),
            to_ticks: 90_000,
        });
        let ts = Timestamp {
            ticks: 48_000,
            timebase: audio().timebase,
        };
        let r = m.translate(ts).unwrap().unwrap();
        assert_eq!(r.ticks, 90_000);
        assert_eq!(r.timebase, video().timebase);
    }

    #[test]
    fn translate_interpolates_between_two_pins() {
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 0,
            to: video(),
            to_ticks: 0,
        });
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 96_000, // 2 s at 48 kHz
            to: video(),
            to_ticks: 180_000, // 2 s at 90 kHz
        });
        let ts = Timestamp {
            ticks: 48_000,
            timebase: audio().timebase,
        };
        let r = m.translate(ts).unwrap().unwrap();
        assert_eq!(r.ticks, 90_000);
        assert_eq!(r.timebase, video().timebase);
    }

    #[test]
    fn translate_rejects_before_first_pin() {
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 48_000,
            to: video(),
            to_ticks: 90_000,
        });
        let ts = Timestamp {
            ticks: 0,
            timebase: audio().timebase,
        };
        let r = m.translate(ts).unwrap();
        assert_eq!(r, Err(SyncError::OutOfRange));
    }

    #[test]
    fn translate_rejects_after_last_pin() {
        // No extrapolation — A/V-drift-prevention guarantee.
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 0,
            to: video(),
            to_ticks: 0,
        });
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 96_000,
            to: video(),
            to_ticks: 180_000,
        });
        let ts = Timestamp {
            ticks: 1_000_000, // past the last pin
            timebase: audio().timebase,
        };
        let r = m.translate(ts).unwrap();
        assert_eq!(r, Err(SyncError::OutOfRange));
    }

    #[test]
    fn translate_rejects_wrong_timebase() {
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 48_000,
            to: video(),
            to_ticks: 90_000,
        });
        let ts = Timestamp {
            ticks: 1000,
            timebase: Timebase::MILLISECONDS,
        };
        let r = m.translate(ts).unwrap();
        assert_eq!(r, Err(SyncError::DomainMismatch));
    }

    #[test]
    fn translate_three_pin_chain_interpolates_locally() {
        // Three pins. Input sits between pins 2 and 3 — interpolation
        // must use *that* pair, not (1,2) or backtrack.
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 0,
            to: video(),
            to_ticks: 0,
        });
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 48_000,
            to: video(),
            to_ticks: 90_000,
        });
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 96_000,
            to: video(),
            to_ticks: 200_000, // non-linear offset
        });
        // 72 000 = 1.5 s at 48 kHz, halfway between pins 2 and 3
        // → halfway between 90 000 and 200 000 = 145 000.
        let ts = Timestamp {
            ticks: 72_000,
            timebase: audio().timebase,
        };
        let r = m.translate(ts).unwrap().unwrap();
        assert_eq!(r.ticks, 145_000);
    }

    #[test]
    fn translate_zero_length_interval_does_not_divide_by_zero() {
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 48_000,
            to: video(),
            to_ticks: 90_000,
        });
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 48_000,
            to: video(),
            to_ticks: 90_000,
        });
        let ts = Timestamp {
            ticks: 48_000,
            timebase: audio().timebase,
        };
        let r = m.translate(ts).unwrap().unwrap();
        assert_eq!(r.ticks, 90_000);
    }

    #[test]
    fn translate_negative_timestamps_in_signed_time() {
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: audio(),
            from_ticks: -48_000,
            to: video(),
            to_ticks: -90_000,
        });
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 0,
            to: video(),
            to_ticks: 0,
        });
        let ts = Timestamp {
            ticks: -24_000, // -0.5 s
            timebase: audio().timebase,
        };
        let r = m.translate(ts).unwrap().unwrap();
        assert_eq!(r.ticks, -45_000);
    }

    #[test]
    fn translate_wallclock_to_video_with_two_pins() {
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: wallclock(),
            from_ticks: 0,
            to: video(),
            to_ticks: 0,
        });
        m.add(SyncPoint {
            from: wallclock(),
            from_ticks: 10, // 10 s
            to: video(),
            to_ticks: 900_000, // 10 s at 90 kHz
        });
        let ts = Timestamp {
            ticks: 5,
            timebase: wallclock().timebase,
        };
        let r = m.translate(ts).unwrap().unwrap();
        assert_eq!(r.ticks, 450_000);
    }

    #[test]
    fn sync_point_accessors_return_timestamps_in_correct_timebase() {
        let p = SyncPoint {
            from: audio(),
            from_ticks: 123,
            to: video(),
            to_ticks: 456,
        };
        let f = p.from_timestamp();
        assert_eq!(f.ticks, 123);
        assert_eq!(f.timebase, audio().timebase);
        let t = p.to_timestamp();
        assert_eq!(t.ticks, 456);
        assert_eq!(t.timebase, video().timebase);
    }

    /// Audio/video sync scenario: two recordings are timestamped in
    /// their own per-domain clocks (audio at 48 kHz, video at 90 kHz)
    /// and joined by two cross-domain pins. The test asserts that
    /// translating a sequence of audio timestamps into video
    /// timestamps yields the expected A/V-aligned video PTS values,
    /// including (a) exact pin matches, (b) interpolated values
    /// between pins, and (c) an out-of-range value past the last pin
    /// being rejected.
    #[test]
    fn audio_video_sync_end_to_end() {
        // Audio is captured at 48 kHz mono, video at 90 kHz (MPEG
        // class). They are joined at capture start (t = 0) and again
        // 60 s in (so we can exercise both the exact-match and
        // interpolation branches).
        let mut m = SyncMap::new();
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 0,
            to: video(),
            to_ticks: 0,
        });
        m.add(SyncPoint {
            from: audio(),
            from_ticks: 60 * 48_000,
            to: video(),
            to_ticks: 60 * 90_000,
        });

        // (a) t = 0 in audio → 0 in video (exact pin match).
        let a0 = Timestamp {
            ticks: 0,
            timebase: audio().timebase,
        };
        let v0 = m.translate(a0).unwrap().unwrap();
        assert_eq!(v0.ticks, 0);

        // (b) t = 30 s audio (= 30 × 48 000 = 1 440 000 ticks) →
        // halfway between pins → 30 × 90 000 = 2 700 000 ticks.
        let a30 = Timestamp {
            ticks: 30 * 48_000,
            timebase: audio().timebase,
        };
        let v30 = m.translate(a30).unwrap().unwrap();
        assert_eq!(v30.ticks, 30 * 90_000);

        // (c) t = 120 s audio (= 5 760 000 ticks) is past the last
        // pin (60 s); must be rejected, not extrapolated.
        let a120 = Timestamp {
            ticks: 120 * 48_000,
            timebase: audio().timebase,
        };
        let r = m.translate(a120).unwrap();
        assert_eq!(r, Err(SyncError::OutOfRange));
    }
}
