//! Uniform scalar quantization helpers shared by the coefficient-stream
//! representations (baseline, block_dct, temporal_dct, motion_aligned) and
//! by `patch_vq`'s codebook.
//!
//! The quantization *knob* `ScalarStep` sweeps the rate axis: each step Δ
//! maps a coefficient `c` to the integer index `round(c/Δ)`. A fixed Δ is
//! applied to every coefficient in a stream, so sweeping Δ from coarse to
//! fine smoothly walks the rate/distortion curve for that representation.
//!
//! This is intentionally naive (no deadzone, no per-band steps); the point of
//! the research platform is to compare *representations* under an identical,
//! un-tuned quantizer, not to build a tuned codec.

use std::marker::PhantomData;

use tpt_media_representation::{Quantizer, RateEstimator};

use crate::entropy::empirical_total_bits;

/// A uniform scalar quantization step `Δ`.
///
/// `index(c) = round(c/Δ)`, `value(i) = i·Δ` — the classic mid-tread
/// uniform scalar quantizer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScalarStep {
    /// Step size (levels are spaced `step` apart). Must be positive.
    pub step: f32,
}

impl ScalarStep {
    /// # Panics
    ///
    /// Panics if `step` is not positive and finite.
    pub fn new(step: f32) -> Self {
        assert!(
            step.is_finite() && step > 0.0,
            "step must be positive and finite"
        );
        ScalarStep { step }
    }

    pub fn index(&self, value: f32) -> i32 {
        (value / self.step).round() as i32
    }

    pub fn value(&self, index: i32) -> f32 {
        index as f32 * self.step
    }
}

/// Quantize a coefficient stream to indices under a fixed step.
pub fn quantize_stream(coeffs: &[f32], step: &ScalarStep) -> Vec<i32> {
    coeffs.iter().map(|c| step.index(*c)).collect()
}

/// Dequantize an index stream back to coefficient values.
pub fn dequantize_stream(indices: &[i32], step: &ScalarStep) -> Vec<f32> {
    indices.iter().map(|&i| step.value(i)).collect()
}

/// Rate estimator for any quantized `Vec<i32>` coefficient stream: the total
/// bits under the stream's own empirical (zeroth-order Shannon) distribution —
/// `n·H`. See [`crate::entropy`] for the measurement definition.
pub struct EntropyRate;

impl RateEstimator<Vec<i32>> for EntropyRate {
    fn estimate_bits(&self, quantized: &Vec<i32>) -> f64 {
        empirical_total_bits(quantized)
    }
}

/// A coefficient stream that a [`LinearQuantizer`] can treat generically:
/// every representation whose `Coeffs` is "a flat stream of `f32`
/// coefficients" (plus metadata) implements this to reuse the same uniform
/// scalar quantizer, instead of one near-identical quantizer per
/// representation. `baseline` and `block_dct` use it directly;
/// `motion_aligned`/`temporal_dct` embed their own streams.
pub trait LinearCoeffs {
    fn stream(&self) -> &[f32];
    fn stream_mut(&mut self) -> &mut [f32];
    fn from_stream(stream: Vec<f32>) -> Self;
}

/// Uniform scalar quantizer for any [`LinearCoeffs`] type `C`.
///
/// `Quantized` is always the index stream `Vec<i32>`, so one
/// [`EntropyRate`] serves every [`LinearQuantizer`] in the rate sweep.
#[derive(Clone, Copy, Debug)]
pub struct LinearQuantizer<C> {
    step: ScalarStep,
    _marker: PhantomData<C>,
}

impl<C: LinearCoeffs> LinearQuantizer<C> {
    /// # Panics
    ///
    /// Panics if `step` is not positive and finite.
    pub fn new(step: f32) -> Self {
        LinearQuantizer {
            step: ScalarStep::new(step),
            _marker: PhantomData,
        }
    }
}

impl<C: LinearCoeffs> Quantizer<C> for LinearQuantizer<C> {
    type Quantized = Vec<i32>;

    fn quantize(&self, coeffs: &C) -> Self::Quantized {
        quantize_stream(coeffs.stream(), &self.step)
    }

    fn dequantize(&self, quantized: &Self::Quantized) -> C {
        C::from_stream(dequantize_stream(quantized, &self.step))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_recovers_value_to_within_half_step() {
        let step = ScalarStep::new(0.25);
        for v in [-1.0f32, -0.37, 0.0, 0.12, 0.49, 2.9] {
            let i = step.index(v);
            let r = step.value(i);
            assert!((v - r).abs() <= 0.125, "v={v} r={r}");
        }
    }

    #[test]
    fn index_is_deterministic() {
        let step = ScalarStep::new(0.1);
        let a = quantize_stream(&[0.33, -0.7, 2.5], &step);
        let b = quantize_stream(&[0.33, -0.7, 2.5], &step);
        assert_eq!(a, b);
    }

    #[test]
    fn finer_step_spans_more_levels() {
        let coarse = ScalarStep::new(1.0);
        let fine = ScalarStep::new(0.01);
        // A finer step maps a spread-out stream to strictly more distinct
        // indices: strictly more levels are reachable.
        let uniq = |s: &ScalarStep| {
            use std::collections::HashSet;
            quantize_stream(&[0.0f32, 0.2, 0.4, 0.6, 0.8, 1.0], s)
                .into_iter()
                .collect::<HashSet<i32>>()
                .len()
        };
        assert!(uniq(&fine) > uniq(&coarse));
    }

    #[test]
    fn entropy_rate_counts_full_stream() {
        // 4 distinct symbols, each repeated -> exactly 2 bits/symbol * 8.
        let q = vec![0i32, 1, 0, 1, 2, 3, 2, 3];
        let bits = EntropyRate.estimate_bits(&q);
        assert!(((bits - 16.0) / 16.0).abs() < 1e-6, "bits={bits}");
    }

    /// §38: empty input. An empty stream transmits nothing, so it costs
    /// nothing — and must not produce the `NaN` that `0 * 0/0` would give.
    #[test]
    fn empty_streams_round_trip_and_cost_nothing() {
        let step = ScalarStep::new(0.25);
        assert!(quantize_stream(&[], &step).is_empty());
        assert!(dequantize_stream(&[], &step).is_empty());
        assert_eq!(EntropyRate.estimate_bits(&Vec::<i32>::new()), 0.0);
    }

    /// §38: overflow. `value / step` is a `f32` division cast to `i32`, which
    /// saturates rather than wrapping — so an absurd ratio pins at
    /// `i32::MAX`/`i32::MIN` instead of aliasing onto a plausible small
    /// index that would silently round-trip to the wrong value.
    #[test]
    fn an_out_of_range_index_saturates_rather_than_wrapping() {
        let tiny = ScalarStep::new(1e-30);
        assert_eq!(tiny.index(1.0), i32::MAX);
        assert_eq!(tiny.index(-1.0), i32::MIN);
        assert_eq!(tiny.index(f32::MAX), i32::MAX);
        // A step far coarser than its input collapses it to the zero level,
        // which is the correct answer for a quantiser that coarse.
        let huge = ScalarStep::new(1e30);
        assert_eq!(huge.index(1.0), 0);
        assert_eq!(huge.index(-1.0), 0);
    }

    /// §38: malformed input. A step that is zero, negative, NaN or infinite
    /// has no meaningful level spacing, and the documented behaviour is a
    /// panic rather than a silently wrong index. `catch_unwind` rather than
    /// `#[should_panic]` because this checks all five bad values, not one.
    #[test]
    fn a_non_positive_or_non_finite_step_is_rejected() {
        for bad in [0.0f32, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let r = std::panic::catch_unwind(|| ScalarStep::new(bad));
            assert!(r.is_err(), "step {bad} should be rejected");
        }
    }

    /// Zero is the coefficient every stream has, and it must map to index 0
    /// and back to exactly 0.0 — including `-0.0`, which compares equal but
    /// would otherwise produce a negative zero on the way back.
    #[test]
    fn zero_round_trips_exactly_in_both_signs() {
        let step = ScalarStep::new(0.25);
        assert_eq!(step.index(0.0), 0);
        assert_eq!(step.index(-0.0), 0);
        assert_eq!(step.value(0), 0.0);
        let back = dequantize_stream(&quantize_stream(&[0.0, -0.0], &step), &step);
        assert_eq!(back, vec![0.0, 0.0]);
    }

    /// The `LinearQuantizer` must be a faithful adapter: whatever
    /// `quantize_stream`/`dequantize_stream` do, it must do identically
    /// through the trait, since that is the path every experiment takes.
    #[test]
    fn the_linear_quantizer_matches_the_free_functions_it_adapts() {
        use tpt_media_representation::Quantizer;
        let stream = vec![0.5f32, -1.25, 3.0, 0.0];
        let q = LinearQuantizer::<crate::representations::BaselineCoeffs>::new(0.25);
        let coeffs = crate::representations::BaselineCoeffs(stream.clone());
        let step = ScalarStep::new(0.25);
        assert_eq!(q.quantize(&coeffs), quantize_stream(&stream, &step));
        let indices = q.quantize(&coeffs);
        let back = q.dequantize(&indices);
        assert_eq!(back.stream(), dequantize_stream(&indices, &step));
    }
}
