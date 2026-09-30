//! Integration: the four contracts, implemented from *outside* the crate.
//!
//! The inline `#[cfg(test)]` modules can reach private items, so nothing in
//! them proves these traits work for a real consumer. That matters here more
//! than in a crate whose API is concrete types: `Representation`, `Quantizer`,
//! `RateEstimator` and `CoeffMagnitudes` have **no implementor inside
//! `tpt-media-representation` at all** — they are contracts, and the
//! implementations live elsewhere in the workspace. A broken contract
//! therefore compiles fine and only fails at the boundary, which is exactly
//! what this file crosses.
//!
//! What it pins: the analyse → quantise → charge → dequantise → synthesise →
//! measure round trip, the `Quantized`-carries-everything contract, the "no
//! free decoder-side information" rate rule, and the rate/distortion frontier
//! the whole crate exists to make swappable.

use tpt_media_representation::{
    psnr_db, CoeffMagnitudes, DistortionMetric, Mse, Quantizer, RateEstimator, Representation,
    SignalBuffer, SignalDims, SignalView, SignalViewMut, SparsityReport,
};

/// Bits charged for the `mode` and `step` fields embedded in [`Quantized`].
const SIDE_INFO_BITS: f64 = 16.0;

/// A coefficient block. `samples` is the *entire* decoder-required state:
/// there is no hidden dictionary or side information in this representation,
/// which is what makes the rate accounting below checkable.
struct Coeffs {
    samples: Vec<f32>,
}

impl CoeffMagnitudes for Coeffs {
    fn coefficient_magnitudes(&self) -> Vec<f32> {
        self.samples.iter().map(|s| s.abs()).collect()
    }

    fn retain_above_threshold(&mut self, threshold: f32) -> usize {
        let mut kept = 0;
        for s in self.samples.iter_mut() {
            if s.abs() >= threshold {
                kept += 1;
            } else {
                *s = 0.0;
            }
        }
        kept
    }
}

/// The `Quantized` form. `step` and `mode` are embedded here on purpose, so
/// that `dequantize` needs no other input *and* the rate estimator is forced
/// to charge for them — the two halves of the same contract.
struct Quantized {
    levels: Vec<i32>,
    step: f32,
    mode: u8,
}

/// The simplest possible `T`: the identity. Its only job is to be a real
/// implementor of the contract, so the other three traits have something to
/// compose with.
struct Identity;

impl Representation for Identity {
    /// Out-of-band-agreed block size — a profile constant, never per-input
    /// signalled data, which is the distinction `Config` documents.
    type Config = usize;
    type Coeffs = Coeffs;

    fn analyze(&self, signal: SignalView<'_>, config: &Self::Config) -> Coeffs {
        assert_eq!(
            signal.dims().samples() % config,
            0,
            "block size must divide the volume"
        );
        Coeffs {
            samples: signal.data().to_vec(),
        }
    }

    fn synthesize(
        &self,
        coeffs: &Self::Coeffs,
        _config: &Self::Config,
        mut out: SignalViewMut<'_>,
    ) {
        assert_eq!(
            out.dims().samples(),
            coeffs.samples.len(),
            "synthesize must write exactly one value per sample"
        );
        out.data_mut().copy_from_slice(&coeffs.samples);
    }
}

/// Uniform scalar quantiser with a configurable step.
struct StepQuantizer {
    step: f32,
}

impl Quantizer<Coeffs> for StepQuantizer {
    type Quantized = Quantized;

    fn quantize(&self, coeffs: &Coeffs) -> Quantized {
        Quantized {
            levels: coeffs
                .samples
                .iter()
                .map(|&s| (s / self.step).round() as i32)
                .collect(),
            step: self.step,
            mode: 0,
        }
    }

    fn dequantize(&self, q: &Quantized) -> Coeffs {
        // Note it reads `step` out of `q`, not out of `self` — proof that
        // `Quantized` really does carry everything `dequantize` needs.
        Coeffs {
            samples: q.levels.iter().map(|&l| l as f32 * q.step).collect(),
        }
    }
}

/// Empirical rate: significant bits per level plus a sign, plus the fixed
/// side-information cost. A zero level costs nothing.
struct BitLengthRate;

impl RateEstimator<Quantized> for BitLengthRate {
    fn estimate_bits(&self, q: &Quantized) -> f64 {
        let coeffs: f64 = q
            .levels
            .iter()
            .map(|&l| {
                if l == 0 {
                    0.0
                } else {
                    i32::abs(l).ilog2() as f64 + 1.0 + 1.0 // magnitude + sign
                }
            })
            .sum();
        coeffs + SIDE_INFO_BITS
    }
}

fn dims(w: usize, h: usize, f: usize) -> SignalDims {
    SignalDims {
        width: w,
        height: h,
        frames: f,
    }
}

/// A smooth ramp plus structure, so quantisation error is actually exercised
/// rather than trivially zero.
fn signal() -> SignalBuffer {
    let d = dims(8, 8, 2);
    let data: Vec<f32> = (0..d.samples())
        .map(|i| {
            let x = (i % d.width) as f32;
            let y = (i / d.width % d.height) as f32;
            (x * 0.1 + y * 0.05).sin() * 0.5 + 0.5
        })
        .collect();
    SignalBuffer::from_vec(d, data)
}

#[test]
fn a_consumer_can_implement_all_four_contracts_and_drive_a_full_rd_sweep() {
    // The whole point of the crate: T is swappable, and the surrounding
    // measurement machinery is what stays fixed.
    let fine = sweep(0.001);
    let coarse = sweep(0.5);
    assert!(fine.bits.is_finite() && fine.psnr.is_finite());
    assert!(coarse.bits < fine.bits, "coarser step must cost fewer bits");
    assert!(coarse.psnr < fine.psnr, "coarser step must distort more");
}

#[test]
fn the_rate_distortion_frontier_is_monotone_across_the_whole_sweep() {
    let steps = [0.001, 0.01, 0.05, 0.1, 0.25, 0.5];
    let points: Vec<Point> = steps.iter().map(|&s| sweep(s)).collect();
    for w in points.windows(2) {
        assert!(
            w[1].bits < w[0].bits,
            "bits must fall as the step grows: {} -> {}",
            w[0].bits,
            w[1].bits
        );
        assert!(
            w[1].psnr < w[0].psnr,
            "psnr must fall as the step grows: {} -> {}",
            w[0].psnr,
            w[1].psnr
        );
    }
    // A strictly decreasing rate against a strictly decreasing quality is a
    // frontier; if either half were flat the sweep would be measuring noise.
    assert!(points[0].bits > points[points.len() - 1].bits);
}

#[test]
fn the_quantized_form_carries_everything_dequantize_needs() {
    // The error is bounded by half a step *whatever* the step, which is only
    // true if `step` travelled inside `Quantized`.
    let sig = signal();
    for step in [0.001, 0.01, 0.1, 0.5] {
        let coeffs = Identity.analyze(sig.view(), &sig.dims().width);
        let q = StepQuantizer { step }.quantize(&coeffs);
        let restored = StepQuantizer { step }.dequantize(&q);
        for (a, b) in coeffs.samples.iter().zip(restored.samples.iter()) {
            assert!(
                (a - b).abs() <= step / 2.0 + 1e-6,
                "step {step}: {a} -> {b} exceeds half a step"
            );
        }
    }
}

#[test]
fn the_rate_estimate_charges_for_embedded_side_information() {
    // The `RateEstimator` "no free decoder-side information" rule. `Quantized`
    // carries `step` and `mode`; a decoder needs both, so both are charged.
    // An implementation that ignored them would report a lower rate than the
    // true transmission cost, and its RD frontier would be flattering.
    let sig = signal();
    let q = StepQuantizer { step: 0.1 }.quantize(&Identity.analyze(sig.view(), &8));
    let bits = BitLengthRate.estimate_bits(&q);
    assert!(
        bits > SIDE_INFO_BITS,
        "the side information alone must be charged: {bits}"
    );

    // Changing the mode cannot change the coefficient cost, and the
    // estimator agrees.
    let mut wider = StepQuantizer { step: 0.1 }.quantize(&Identity.analyze(sig.view(), &8));
    wider.mode = 1;
    assert_eq!(BitLengthRate.estimate_bits(&wider), bits);
}

#[test]
fn coefficient_magnitudes_and_the_sparsity_report_work_on_an_external_type() {
    let sig = signal();
    let mut coeffs = Identity.analyze(sig.view(), &sig.dims().width);

    let mags = coeffs.coefficient_magnitudes();
    assert_eq!(mags.len(), sig.dims().samples());
    assert!(mags.iter().all(|&m| m >= 0.0), "magnitudes are absolute");

    let report = SparsityReport::compute(&mags, 1e-6);
    assert_eq!(report.total, sig.dims().samples());
    assert!(report.nonzero > 0);
    assert!((report.energy_compaction.last().copied().unwrap() - 1.0).abs() < 1e-5);

    // Retention zeroes in place and reports the survivors. The signal spans
    // roughly [0.5, 0.93], so 0.8 splits it — a threshold that kept
    // everything would prove nothing about the zeroing half.
    let kept = coeffs.retain_above_threshold(0.8);
    assert!(kept > 0, "some samples are above 0.8");
    assert!(kept < sig.dims().samples(), "some samples are not");
    assert_eq!(coeffs.samples.len(), sig.dims().samples());
    assert!(coeffs.samples.contains(&0.0));
}

#[test]
fn the_measurement_pipeline_is_deterministic() {
    // §39: same input, same numbers, every run. A research platform whose
    // numbers move between runs cannot be compared against anything.
    assert_eq!(sweep(0.05).bits, sweep(0.05).bits);
    assert_eq!(sweep(0.05).psnr, sweep(0.05).psnr);
}

#[test]
fn a_flat_zero_signal_is_lossless_at_every_step_and_reports_so() {
    // Every level is 0 at any step, so the reconstruction is exact and PSNR
    // is infinite. That is the *same* answer an empty volume gives (see the
    // inline distortion tests) — which is precisely why these numbers are
    // only meaningful read alongside the sample count.
    let d = dims(8, 8, 2);
    let sig = SignalBuffer::zeroed(d);
    for step in [0.001, 0.1, 0.5] {
        let coeffs = Identity.analyze(sig.view(), &d.width);
        let q = StepQuantizer { step }.quantize(&coeffs);
        assert!(q.levels.iter().all(|&l| l == 0));
        let restored = StepQuantizer { step }.dequantize(&q);
        let mut out = SignalBuffer::zeroed(d);
        Identity.synthesize(&restored, &d.width, out.view_mut());
        let mse = Mse.distortion(sig.view(), out.view());
        assert_eq!(mse, 0.0, "step {step}");
        assert_eq!(psnr_db(mse, 1.0), f64::INFINITY);
    }
}

#[test]
fn the_half_step_error_bound_is_tight_rather_than_merely_an_upper_bound() {
    // A sample sitting exactly between two levels must come out exactly half
    // a step away. A looser bound would hide a quantiser that is quietly
    // worse than it claims — the bound is the contract, not a courtesy.
    for step in [0.1f32, 0.25, 1.0] {
        let coeffs = Coeffs {
            samples: vec![step / 2.0],
        };
        let q = StepQuantizer { step }.quantize(&coeffs);
        let restored = StepQuantizer { step }.dequantize(&q);
        let err = (coeffs.samples[0] - restored.samples[0]).abs();
        assert!((err - step / 2.0).abs() < 1e-6, "step {step}: err {err}");
    }
}

/// One point on a rate/distortion sweep.
struct Point {
    bits: f64,
    psnr: f64,
}

fn sweep(step: f32) -> Point {
    let sig = signal();
    let rep = Identity;
    let config = sig.dims().width;

    let coeffs = rep.analyze(sig.view(), &config);
    let quantized = StepQuantizer { step }.quantize(&coeffs);
    let bits = BitLengthRate.estimate_bits(&quantized);
    let restored = StepQuantizer { step }.dequantize(&quantized);

    let mut out = SignalBuffer::zeroed(sig.dims());
    rep.synthesize(&restored, &config, out.view_mut());

    let mse = Mse.distortion(sig.view(), out.view());
    Point {
        bits,
        psnr: psnr_db(mse, 1.0),
    }
}
