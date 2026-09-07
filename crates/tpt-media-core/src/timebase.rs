//! Time arithmetic without `f64`.
//!
//! Time in `tpt-media` is expressed as a [`Timestamp`] in units of a
//! rational [`Timebase`]. Conversions between timebases use exact integer
//! arithmetic where possible and only fall back to `f64` at API
//! boundaries.

use core::cmp::Ordering;
use core::fmt;
use core::ops::{Add, Sub};

/// A rational timebase: `num / den` seconds per tick.
///
/// Both fields are `u32`. `0/0` is an invalid timebase and panics in
/// debug; in release it is treated as `1/1` to keep callers safe.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Timebase {
    /// Numerator.
    pub num: u32,
    /// Denominator.
    pub den: u32,
}

impl Timebase {
    /// One tick per second.
    pub const ONE_PER_SECOND: Timebase = Timebase { num: 1, den: 1 };

    /// 1000 ticks per second (milliseconds).
    pub const MILLISECONDS: Timebase = Timebase { num: 1, den: 1_000 };

    /// 1_000_000 ticks per second (microseconds).
    pub const MICROSECONDS: Timebase = Timebase {
        num: 1,
        den: 1_000_000,
    };

    /// 90_000 ticks per second (MPEG-class).
    pub const NINETY_KHZ: Timebase = Timebase {
        num: 1,
        den: 90_000,
    };

    /// Build a new timebase, panicking in debug if either component is
    /// zero. In release, returns `1/1` instead.
    #[inline]
    pub const fn new(num: u32, den: u32) -> Self {
        if num == 0 || den == 0 {
            debug_assert!(num != 0, "timebase numerator must be non-zero");
            debug_assert!(den != 0, "timebase denominator must be non-zero");
            Timebase::ONE_PER_SECOND
        } else {
            Timebase { num, den }
        }
    }

    /// Returns `true` if this is a valid (non-zero) timebase.
    #[inline]
    pub const fn is_valid(self) -> bool {
        self.num != 0 && self.den != 0
    }

    /// Compute the integer gcd of two `u32`s.
    #[inline]
    pub const fn gcd(mut a: u32, mut b: u32) -> u32 {
        while b != 0 {
            let t = b;
            b = a % b;
            a = t;
        }
        a
    }

    /// Return a copy of this timebase reduced to lowest terms.
    pub fn reduced(self) -> Self {
        let g = Self::gcd(self.num, self.den);
        Timebase {
            num: self.num / g,
            den: self.den / g,
        }
    }
}

impl Default for Timebase {
    fn default() -> Self {
        Timebase::ONE_PER_SECOND
    }
}

impl fmt::Display for Timebase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

/// A point in time, expressed as a signed tick count in a given
/// [`Timebase`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Timestamp {
    /// Tick count (signed).
    pub ticks: i64,
    /// Timebase of the tick count.
    pub timebase: Timebase,
}

impl Timestamp {
    /// The zero timestamp for a given timebase.
    #[inline]
    pub const fn zero(timebase: Timebase) -> Self {
        Timestamp { ticks: 0, timebase }
    }

    /// Convert this timestamp to another timebase.
    ///
    /// Returns `None` if the conversion would overflow `i64` or if
    /// either timebase is invalid.
    ///
    /// Math: a tick at `self.timebase` represents
    /// `ticks · self.timebase.num / self.timebase.den` seconds; in
    /// `target`, the same duration is
    /// `ticks · self.timebase.num / self.timebase.den · target.den / target.num`
    /// ticks. So
    ///
    /// ```text
    /// new_ticks = ticks · self.timebase.num · target.den
    ///           / (self.timebase.den · target.num)
    /// ```
    ///
    /// Every intermediate is computed in `i128` with explicit
    /// `checked_mul` so an overflow anywhere along the way surfaces as
    /// `None` instead of a wrong answer.
    pub fn rescale(self, target: Timebase) -> Option<Timestamp> {
        if !self.timebase.is_valid() || !target.is_valid() {
            return None;
        }
        if self.timebase == target {
            return Some(self);
        }
        let num = (self.ticks as i128)
            .checked_mul(self.timebase.num as i128)?
            .checked_mul(target.den as i128)?;
        let den = (self.timebase.den as i128).checked_mul(target.num as i128)?;
        let new = num.checked_div(den)?;
        if new > i64::MAX as i128 || new < i64::MIN as i128 {
            return None;
        }
        Some(Timestamp {
            ticks: new as i64,
            timebase: target,
        })
    }

    /// Convert to seconds as `f64`. Used only at API boundaries.
    #[inline]
    pub fn to_seconds(self) -> f64 {
        if !self.timebase.is_valid() {
            return 0.0;
        }
        (self.ticks as f64) * (self.timebase.num as f64) / (self.timebase.den as f64)
    }
}

impl Ord for Timestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.rescale(other.timebase) {
            Some(t) => t.ticks.cmp(&other.ticks),
            None => self
                .to_seconds()
                .partial_cmp(&other.to_seconds())
                .unwrap_or(Ordering::Equal),
        }
    }
}

impl PartialOrd for Timestamp {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} @ {}", self.ticks, self.timebase)
    }
}

/// A signed duration with a timebase-independent representation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Duration {
    /// Tick count (signed).
    pub ticks: i64,
    /// Timebase.
    pub timebase: Timebase,
}

impl Duration {
    /// Zero duration.
    pub const ZERO: Duration = Duration {
        ticks: 0,
        timebase: Timebase::ONE_PER_SECOND,
    };

    /// Build a duration from a count of samples at a given sample rate.
    pub fn from_samples(samples: i64, sample_rate: u32) -> Self {
        Duration {
            ticks: samples,
            timebase: Timebase::new(1, sample_rate),
        }
    }

    /// Build a duration from a floating-point seconds count.
    pub fn from_seconds(seconds: f64) -> Self {
        let nanos = (seconds * 1e9).round() as i64;
        Duration {
            ticks: nanos,
            timebase: Timebase::MICROSECONDS,
        }
    }

    /// View this duration as a [`Timestamp`] in the given timebase.
    pub fn as_timestamp(self, target: Timebase) -> Timestamp {
        Timestamp {
            ticks: self.ticks,
            timebase: self.timebase,
        }
        .rescale(target)
        .unwrap_or(Timestamp::zero(target))
    }

    /// Convert to seconds as `f64`.
    pub fn to_seconds(self) -> f64 {
        if !self.timebase.is_valid() {
            return 0.0;
        }
        (self.ticks as f64) * (self.timebase.num as f64) / (self.timebase.den as f64)
    }
}

impl Default for Duration {
    fn default() -> Self {
        Duration::ZERO
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ticks @ {}", self.ticks, self.timebase)
    }
}

impl Add<Duration> for Timestamp {
    type Output = Timestamp;
    /// Adds a [`Duration`] to this timestamp.
    ///
    /// # Overflow handling
    ///
    /// On overflow the result is **saturated** to `Timestamp::MAX` /
    /// `Timestamp::MIN` (i.e., `i64::MAX` / `i64::MIN` ticks in the
    /// current timebase). This is deliberately *not* a fallible
    /// operation: the `+` operator has no way to return `None`, and
    /// silently returning the unchanged left-hand side (the previous
    /// behaviour, via `unwrap_or(self.ticks)`) is much worse — a
    /// caller that overflows once would get a `Timestamp` that's
    /// neither the pre-overflow value nor the correct post-overflow
    /// value, and would have no way to tell.
    ///
    /// Callers that need to *detect* overflow should use
    /// [`Timestamp::checked_add_duration`] instead, which returns
    /// `Option<Timestamp>` and propagates the `None` explicitly.
    fn add(self, rhs: Duration) -> Timestamp {
        let rhs_ts = rhs.as_timestamp(self.timebase);
        let ticks = match self.ticks.checked_add(rhs_ts.ticks) {
            Some(t) => t,
            None => {
                // Saturate in the direction of the sign of `rhs_ts.ticks`.
                if rhs_ts.ticks >= 0 {
                    i64::MAX
                } else {
                    i64::MIN
                }
            }
        };
        Timestamp {
            ticks,
            timebase: self.timebase,
        }
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Timestamp;
    /// Subtracts a [`Duration`] from this timestamp. See
    /// [`Timestamp::add`] for the overflow policy (saturating); use
    /// [`Timestamp::checked_sub_duration`] for fallible arithmetic.
    fn sub(self, rhs: Duration) -> Timestamp {
        let rhs_ts = rhs.as_timestamp(self.timebase);
        let ticks = match self.ticks.checked_sub(rhs_ts.ticks) {
            Some(t) => t,
            None => {
                // Saturate in the direction of the sign of `-rhs_ts.ticks`,
                // i.e., opposite to `rhs_ts.ticks`.
                if rhs_ts.ticks >= 0 {
                    i64::MIN
                } else {
                    i64::MAX
                }
            }
        };
        Timestamp {
            ticks,
            timebase: self.timebase,
        }
    }
}

impl Timestamp {
    /// Checked addition of a [`Duration`]. Returns `None` on overflow
    /// of the underlying `i64` tick counter.
    ///
    /// Prefer [`Timestamp::add`] when saturation is acceptable (the
    /// default for the `+` operator); prefer this when the caller
    /// genuinely needs to know the addition didn't fit.
    pub fn checked_add_duration(self, rhs: Duration) -> Option<Timestamp> {
        let rhs_ts = rhs.as_timestamp(self.timebase);
        self.ticks.checked_add(rhs_ts.ticks).map(|t| Timestamp {
            ticks: t,
            timebase: self.timebase,
        })
    }

    /// Checked subtraction of a [`Duration`]. Returns `None` on
    /// overflow of the underlying `i64` tick counter.
    pub fn checked_sub_duration(self, rhs: Duration) -> Option<Timestamp> {
        let rhs_ts = rhs.as_timestamp(self.timebase);
        self.ticks.checked_sub(rhs_ts.ticks).map(|t| Timestamp {
            ticks: t,
            timebase: self.timebase,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timebase_reduced() {
        let tb = Timebase::new(1000, 48000);
        let r = tb.reduced();
        assert_eq!(r.num, 1);
        assert_eq!(r.den, 48);
    }

    #[test]
    fn rescale_identity() {
        let ts = Timestamp {
            ticks: 1234,
            timebase: Timebase::MILLISECONDS,
        };
        let r = ts.rescale(Timebase::MILLISECONDS).unwrap();
        assert_eq!(r, ts);
    }

    #[test]
    fn rescale_90khz_to_millis() {
        let ts = Timestamp {
            ticks: 90_000,
            timebase: Timebase::NINETY_KHZ,
        };
        let r = ts.rescale(Timebase::MILLISECONDS).unwrap();
        assert_eq!(r.ticks, 1000);
    }

    #[test]
    fn rescale_overflow_returns_none() {
        // Going 1-Hz → 90 kHz is a 90,000× expansion. `i64::MAX` seconds
        // (~292 billion years) times 90,000 overflows `i64` (~9 × 10^23)
        // by a wide margin, which is exactly what we want to exercise
        // here.
        let ts = Timestamp {
            ticks: i64::MAX,
            timebase: Timebase::ONE_PER_SECOND,
        };
        let r = ts.rescale(Timebase::NINETY_KHZ);
        assert!(r.is_none());
    }

    #[test]
    fn rescale_overflow_checked_mul_uses_correct_factors() {
        // Sanity check that the bug fix (and only the bug fix) doesn't
        // silently change *non-overflowing* results: a 1-Hz timestamp of
        // 1 second must become exactly 90_000 ticks at 90 kHz.
        let ts = Timestamp {
            ticks: 1,
            timebase: Timebase::ONE_PER_SECOND,
        };
        let r = ts.rescale(Timebase::NINETY_KHZ).unwrap();
        assert_eq!(r.ticks, 90_000);
    }

    #[test]
    fn timestamp_ordering() {
        let a = Timestamp {
            ticks: 90_000,
            timebase: Timebase::NINETY_KHZ,
        };
        let b = Timestamp {
            ticks: 1_000,
            timebase: Timebase::MILLISECONDS,
        };
        assert_eq!(a.cmp(&b), Ordering::Equal);
    }

    #[test]
    fn timestamp_add_duration() {
        let ts = Timestamp {
            ticks: 0,
            timebase: Timebase::MILLISECONDS,
        };
        let d = Duration {
            ticks: 500,
            timebase: Timebase::MILLISECONDS,
        };
        let sum = ts + d;
        assert_eq!(sum.ticks, 500);
    }

    #[test]
    fn timestamp_add_duration_saturates_on_overflow() {
        // Adding a positive duration that would overflow `i64::MAX`
        // must saturate, not silently return the unchanged left-hand
        // side (the previous behaviour). The previous bug was a
        // `unwrap_or(self.ticks)`, which here would have returned
        // `i64::MAX` *as if the addition hadn't happened* — except
        // that `i64::MAX` was already `self.ticks`, so the silent
        // fallback happened to agree with saturation in this exact
        // direction. The case that exposes the difference is a
        // *negative* saturation (overflow while subtracting a positive
        // duration): see `timestamp_sub_duration_saturates_on_overflow`.
        let ts = Timestamp {
            ticks: i64::MAX,
            timebase: Timebase::MILLISECONDS,
        };
        let d = Duration {
            ticks: 1,
            timebase: Timebase::MILLISECONDS,
        };
        let sum = ts + d;
        assert_eq!(sum.ticks, i64::MAX);
    }

    #[test]
    fn timestamp_sub_duration_saturates_on_overflow() {
        // This is the case the old `unwrap_or(self.ticks)` got *wrong*:
        // `i64::MIN - Duration{1 ms}` would underflow, and the old
        // code would return `i64::MIN` as if nothing happened. Saturation
        // also lands on `i64::MIN` here, but the *direction* of the
        // fix matters: a positive duration subtracted past zero must
        // clamp to `i64::MIN`, while a *negative* duration subtracted
        // past zero must clamp to `i64::MAX`.
        let ts = Timestamp {
            ticks: 0,
            timebase: Timebase::MILLISECONDS,
        };
        let d_neg = Duration {
            ticks: -1,
            timebase: Timebase::MILLISECONDS,
        };
        // Subtracting a *negative* duration == adding: 0 - (-1) = 1.
        let r = ts - d_neg;
        assert_eq!(r.ticks, 1);

        let ts = Timestamp {
            ticks: i64::MIN,
            timebase: Timebase::MILLISECONDS,
        };
        let d = Duration {
            ticks: 1,
            timebase: Timebase::MILLISECONDS,
        };
        let r = ts - d;
        assert_eq!(r.ticks, i64::MIN, "must saturate, not silently return self");
    }

    #[test]
    fn timestamp_checked_add_duration_propagates_none() {
        let ts = Timestamp {
            ticks: i64::MAX,
            timebase: Timebase::MILLISECONDS,
        };
        let d = Duration {
            ticks: 1,
            timebase: Timebase::MILLISECONDS,
        };
        assert!(ts.checked_add_duration(d).is_none());
        let ts = Timestamp {
            ticks: i64::MAX - 5,
            timebase: Timebase::MILLISECONDS,
        };
        assert_eq!(
            ts.checked_add_duration(Duration {
                ticks: 5,
                timebase: Timebase::MILLISECONDS
            })
            .unwrap()
            .ticks,
            i64::MAX
        );
    }

    #[test]
    fn timestamp_checked_sub_duration_propagates_none() {
        let ts = Timestamp {
            ticks: i64::MIN,
            timebase: Timebase::MILLISECONDS,
        };
        let d = Duration {
            ticks: 1,
            timebase: Timebase::MILLISECONDS,
        };
        assert!(ts.checked_sub_duration(d).is_none());
    }

    #[test]
    fn timestamp_add_sub_negative_duration_round_trips() {
        // `a + d == b`, `b - d == a` for ordinary values; negative
        // durations must behave consistently.
        let a = Timestamp {
            ticks: 100,
            timebase: Timebase::MILLISECONDS,
        };
        let d_pos = Duration {
            ticks: 50,
            timebase: Timebase::MILLISECONDS,
        };
        let d_neg = Duration {
            ticks: -50,
            timebase: Timebase::MILLISECONDS,
        };
        assert_eq!((a + d_pos).ticks, 150);
        assert_eq!((a + d_neg).ticks, 50);
        let b = a + d_pos;
        assert_eq!((b - d_pos).ticks, 100);
    }

    #[test]
    fn duration_from_samples() {
        let d = Duration::from_samples(480, 48_000);
        assert_eq!(d.ticks, 480);
        assert_eq!(d.timebase.den, 48_000);
        assert!((d.to_seconds() - 0.01).abs() < 1e-9);
    }

    #[test]
    fn long_duration_arithmetic() {
        let ticks_per_day = 90_000i64 * 60 * 60 * 24;
        let ts = Timestamp {
            ticks: ticks_per_day,
            timebase: Timebase::NINETY_KHZ,
        };
        let seconds = ts.to_seconds();
        assert!((seconds - 86_400.0).abs() < 1e-6);
    }

    #[test]
    fn invalid_timebase_does_not_panic_in_release() {
        let bad = Timebase { num: 0, den: 0 };
        assert!(!bad.is_valid());
        let ts = Timestamp {
            ticks: 1,
            timebase: bad,
        };
        assert_eq!(ts.to_seconds(), 0.0);
    }
}
