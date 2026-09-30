//! Integration: the research platform's public API, driven from *outside*.
//!
//! The inline `#[cfg(test)]` modules can reach private items, so nothing in
//! them proves the crate works for a consumer. That matters here because the
//! crate is a *measurement* platform: the whole claim is that two
//! representations can be swapped and compared honestly through these
//! contracts. A harness that only composes from the inside would not support
//! that claim.
//!
//! What it pins: every representation drives the same rate/distortion
//! machinery through the public API; the matched-rate comparison refuses to
//! extrapolate; and the whole platform is byte-identical across runs, which
//! is the property the published numbers in `docs/representation-research.md`
//! rest on.

use tpt_media_representation::{DistortionMetric, Mse, Quantizer, Representation, SignalBuffer};
use tpt_media_research::diagnostics::{retention_curve, secondary_diagnostics};
use tpt_media_research::harness::{
    default_steps, interpolated_distortion, rd_curve, rd_point, RdPoint,
};
use tpt_media_research::representations::*;
use tpt_media_research::signals::spatial::gradient;
use tpt_media_research::uniform::{EntropyRate, LinearQuantizer};

const W: u32 = 16;
const H: u32 = 16;

/// Every public representation must survive the same analyse → quantise →
/// charge → dequantise → synthesise → measure loop, with every reported
/// number finite. This is the crate's documented validity contract, and it is
/// only meaningful if it holds for *all* the representations being compared.
#[test]
fn every_representation_produces_a_finite_rd_curve() {
    let sig = gradient(W as usize, H as usize);
    let mut out = SignalBuffer::zeroed(sig.dims());
    let steps = default_steps();

    let runs: Vec<(&str, Vec<RdPoint>)> = vec![
        (
            "baseline",
            rd_curve(
                sig.view(),
                &Baseline,
                &BaselineConfig,
                &steps,
                LinearQuantizer::<BaselineCoeffs>::new,
                &EntropyRate,
                &mut out,
            ),
        ),
        (
            "block_dct",
            rd_curve(
                sig.view(),
                &BlockDct,
                &BlockDctConfig { block_size: 8 },
                &steps,
                LinearQuantizer::<BlockDctCoeffs>::new,
                &EntropyRate,
                &mut out,
            ),
        ),
    ];

    for (name, curve) in runs {
        assert_eq!(curve.len(), steps.len(), "{name}: one point per step");
        for p in &curve {
            assert!(p.bits_per_sample.is_finite(), "{name}: rate {p:?}");
            assert!(p.mse.is_finite(), "{name}: mse {p:?}");
            assert!(p.bits_per_sample >= 0.0, "{name}: negative rate {p:?}");
            assert!(p.mse >= 0.0, "{name}: negative mse {p:?}");
        }
        // A representation whose curve is entirely bit-exact measured nothing
        // at all; anything with real error is a real measurement.
        assert!(
            curve.iter().any(|p| p.mse > 0.0),
            "{name}: every frame was bit-exact, which measures nothing"
        );
    }
}

/// The rate axis must be non-decreasing once `rd_curve` has sorted it — the
/// precondition for any matched-rate comparison to mean anything.
#[test]
fn a_linear_representation_spends_more_rate_for_less_error() {
    let sig = gradient(W as usize, H as usize);
    let mut out = SignalBuffer::zeroed(sig.dims());
    let curve = rd_curve(
        sig.view(),
        &Baseline,
        &BaselineConfig,
        &default_steps(),
        LinearQuantizer::<BaselineCoeffs>::new,
        &EntropyRate,
        &mut out,
    );
    assert!(
        curve
            .windows(2)
            .all(|w| w[0].bits_per_sample <= w[1].bits_per_sample),
        "rate must be non-decreasing across the sorted curve"
    );
    // Stated as an existence check, not a direction, so this stays a
    // measurement-validity test rather than a quality claim.
    assert!(
        curve.first().unwrap().mse >= curve.last().unwrap().mse,
        "the finest step should not distort more than the coarsest"
    );
}

/// The matched-rate comparison must interpolate *within* the sampled range and
/// refuse to extrapolate — the property that stops the harness inventing a
/// result for a rate a representation never reached.
#[test]
fn matched_rate_comparison_interpolates_but_never_extrapolates() {
    let points = vec![
        RdPoint {
            bits_per_sample: 0.5,
            mse: 0.40,
        },
        RdPoint {
            bits_per_sample: 1.0,
            mse: 0.20,
        },
        RdPoint {
            bits_per_sample: 2.0,
            mse: 0.05,
        },
    ];
    assert_eq!(interpolated_distortion(&points, 1.0), Some(0.20));
    let mid = interpolated_distortion(&points, 0.75).expect("in range");
    assert!((mid - 0.30).abs() < 1e-12, "mid={mid}");
    // Out of range: refused, in both directions.
    assert_eq!(interpolated_distortion(&points, 0.1), None);
    assert_eq!(interpolated_distortion(&points, 9.0), None);
}

/// A single point (`rd_point`) and the same step inside a curve (`rd_curve`)
/// must agree, or the curve would be measuring something the single-shot
/// measurement does not.
#[test]
fn a_single_point_agrees_with_the_same_step_inside_a_curve() {
    let sig = gradient(W as usize, H as usize);
    let mut out = SignalBuffer::zeroed(sig.dims());
    let step = 0.25f32;
    let single = rd_point(
        sig.view(),
        &Baseline,
        &BaselineConfig,
        &LinearQuantizer::<BaselineCoeffs>::new(step),
        &EntropyRate,
        &mut out,
    );
    let curve = rd_curve(
        sig.view(),
        &Baseline,
        &BaselineConfig,
        &[step],
        LinearQuantizer::<BaselineCoeffs>::new,
        &EntropyRate,
        &mut out,
    );
    assert_eq!(curve.len(), 1);
    assert_eq!(single, curve[0]);
}

/// §39: the crate's numbers are published in `docs/representation-research.md`
/// and must be byte-identical run for run. A harness whose measurements drift
/// cannot be compared against anything, including a previous version of itself.
#[test]
fn measurements_are_byte_identical_across_runs() {
    let sig = gradient(W as usize, H as usize);
    let mut a = SignalBuffer::zeroed(sig.dims());
    let mut b = SignalBuffer::zeroed(sig.dims());
    let steps = default_steps();
    let mk = |out: &mut SignalBuffer| {
        rd_curve(
            sig.view(),
            &BlockDct,
            &BlockDctConfig { block_size: 8 },
            &steps,
            LinearQuantizer::<BlockDctCoeffs>::new,
            &EntropyRate,
            out,
        )
    };
    assert_eq!(mk(&mut a), mk(&mut b));
    // The scratch buffers must have been filled identically too, not just the
    // reported numbers.
    assert_eq!(a.data(), b.data());
}

/// Diagnostics are secondary by policy and must never be mistaken for the
/// primary result. This checks they are computable and self-consistent from
/// outside, without asserting any direction about quality.
#[test]
fn diagnostics_agree_with_the_stream_they_describe() {
    let sig = gradient(W as usize, H as usize);
    let rep = Baseline;
    let coeffs = BaselineCoeffs(sig.data().to_vec());
    let quantized = LinearQuantizer::<BaselineCoeffs>::new(0.25).quantize(&coeffs);

    let d = secondary_diagnostics(&quantized);
    assert_eq!(d.total, quantized.len(), "every index is a coefficient");
    assert!(d.nonzero <= d.total);
    assert!(d.entropy_bits.is_finite() && d.entropy_bits >= 0.0);
    assert!(d.coefficients_for_99pct_energy <= d.total);

    // The retention curve runs from outside too, over the same coefficients.
    let mut out = SignalBuffer::zeroed(sig.dims());
    let curve = retention_curve(
        sig.view(),
        &rep,
        &BaselineConfig,
        coeffs,
        &[0.0, 1000.0],
        &mut out,
    );
    assert_eq!(curve.len(), 2);
    assert!((curve[0].retained_fraction - 1.0).abs() < 1e-9);
    assert_eq!(curve[1].retained_fraction, 0.0, "nothing clears 1000");
    assert!(curve[1].mse.is_finite());
}

/// The representations' own round trips are exercised in their unit tests;
/// what is only visible from outside is that a *lossless* configuration
/// reconstructs the original samples, which is what makes a distortion
/// measurement meaningful in the first place.
#[test]
fn a_lossless_round_trip_reconstructs_the_signal() {
    let sig = gradient(W as usize, H as usize);
    let mut out = SignalBuffer::zeroed(sig.dims());
    let rep = Baseline;
    let cfg = BaselineConfig;
    let coeffs = rep.analyze(sig.view(), &cfg);
    rep.synthesize(&coeffs, &cfg, out.view_mut());
    assert_eq!(out.data(), sig.data());
    assert_eq!(Mse.distortion(sig.view(), out.view()), 0.0);
}
