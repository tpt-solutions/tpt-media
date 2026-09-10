//! The rate/distortion measurement harness (`todo.md` §56).
//!
//! Primary methodology: for a fixed signal and a fixed `Representation`, sweep
//! the scalar quantization step Δ to produce the representation's **RD
//! frontier** — `bits/sample` (empirical entropy of the transmitted state)
//! vs `MSE`. Representations are compared at *matched rate points* by
//! interpolating each curve **within its sampled range only**; extrapolation
//! is structurally rejected ([`interpolated_distortion`] returns `None`), so
//! the harness can never invent a result for a rate a representation never
//! reached.
//!
//! Metric hierarchy (documented as policy here):
//!
//! * **Primary** — the matched-rate RD comparison (bits/sample vs MSE/PSNR).
//!   This is the only number that may decide "which representation is
//!   better".
//! * **Secondary** (`crate::diagnostics`) — entropy, non-zero coefficient
//!   count, energy compaction. These explain the RD curve; they never decide
//!   anything.
//! * **Diagnostic** (`crate::diagnostics::retention_curve`) — what happens
//!   to a representation when you zero its small coefficients. Diagnostic
//!   only, never used to compare representations.
//!
//! Validity contract (asserted by every experiment's test): every reported
//! value finite; identical inputs reproduce byte-identical output; each
//! representation's own round trip reconstructs within tolerance (exercised
//! per-representation in its own unit tests); matched-rate comparison only
//! ever interpolates.
//!
//! **Measurement honesty**: the harness never asserts a hypothesis. Tests
//! around the oracle/phase experiments check *measurement validity*, never
//! the direction of the result, so "motion alignment didn't help under these
//! conditions" is a legitimate, recordable outcome.

use tpt_media_representation::{
    DistortionMetric, Mse, Quantizer, RateEstimator, Representation, SignalBuffer, SignalView,
};

/// One point on an RD curve: transmission cost per sample and reconstruction
/// error (mean squared error, lower is better).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RdPoint {
    pub bits_per_sample: f64,
    pub mse: f64,
}

impl RdPoint {
    /// PSNR in dB for a signal with peak amplitude `peak` (e.g. `1.0` for a
    /// `[0,1]`-normalized synthetic signal). `f64::INFINITY` when lossless.
    pub fn psnr_db(&self, peak: f64) -> f64 {
        tpt_media_representation::psnr_db(self.mse, peak)
    }
}

/// Measure a single transmission at one quantization step: analyze once,
/// quantize, charge, dequantize, synthesize, measure. `out` is reused (and
/// fully overwritten) scratch.
pub fn rd_point<R, Q, M>(
    signal: SignalView<'_>,
    rep: &R,
    config: &R::Config,
    quantizer: &Q,
    rate: &M,
    out: &mut SignalBuffer,
) -> RdPoint
where
    R: Representation,
    Q: Quantizer<R::Coeffs>,
    M: RateEstimator<Q::Quantized>,
{
    let coeffs = rep.analyze(signal, config);
    let q = quantizer.quantize(&coeffs);
    let total_bits = rate.estimate_bits(&q);
    let recon = quantizer.dequantize(&q);
    rep.synthesize(&recon, config, out.view_mut());
    let mse = Mse.distortion(signal, out.view());
    RdPoint {
        bits_per_sample: total_bits / signal.data().len() as f64,
        mse,
    }
}

/// Sweep a representation's RD curve over `steps` quantization steps.
///
/// `analyze` runs **once** (which is what makes VQ-training and the motion
/// warp honest and deterministic across the sweep); each step then produces
/// one [`RdPoint`]. `make_quantizer(step)` builds the per-step quantizer
/// (stateless quantizers mean this is just a step→quantizer mapping). The
/// returned curve is in increasing-rate order.
pub fn rd_curve<R, Q, M>(
    signal: SignalView<'_>,
    rep: &R,
    config: &R::Config,
    steps: &[f32],
    make_quantizer: impl Fn(f32) -> Q,
    rate: &M,
    out: &mut SignalBuffer,
) -> Vec<RdPoint>
where
    R: Representation,
    Q: Quantizer<R::Coeffs>,
    M: RateEstimator<Q::Quantized>,
{
    let coeffs = rep.analyze(signal, config);
    let n = signal.data().len() as f64;
    let mut points: Vec<RdPoint> = steps
        .iter()
        .map(|&step| {
            let quantizer = make_quantizer(step);
            let q = quantizer.quantize(&coeffs);
            let total_bits = rate.estimate_bits(&q);
            let recon = quantizer.dequantize(&q);
            rep.synthesize(&recon, config, out.view_mut());
            let mse = Mse.distortion(signal, out.view());
            RdPoint {
                bits_per_sample: total_bits / n,
                mse,
            }
        })
        .collect();
    // Sort into increasing-rate order so downstream interpolation and the
    // validity check see a well-formed curve.
    points.sort_by(|a, b| {
        a.bits_per_sample
            .partial_cmp(&b.bits_per_sample)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    points
}

/// The default quantization sweep for the experiments: Δ from `2^-8` (fine)
/// to `2^7` (very coarse on `[0,1]` signals), 16 geometric steps.
///
/// The coarse end exists so *every* condition can reach the low-rate regime:
/// `motion-aligned` on an easy signal operates around `10^-2` bits/sample
/// (64-bit velocity charge amortized over the volume, aligned coefficients
/// near-lossless at every step), so only a sweep that lets the linear
/// representations drop toward 0 bits/sample creates an overlap a matched-rate
/// comparison can live in.
pub fn default_steps() -> Vec<f32> {
    (0..16).map(|i| 2.0f32.powi(-8 + i)).collect()
}

/// Interpolate a curve's distortion at `target_rate` **within** its sampled
/// range — linear in the (`bits_per_sample`, `mse`) plane between adjacent
/// points. Returns `None` for any target outside the curve's sampled range
/// (extrapolation is structurally rejected).
///
/// Exact-rate matches return the exact measured distortion. Nearly-coincident
/// duplicate-rate points collapse to a single value.
pub fn interpolated_distortion(points: &[RdPoint], target_rate: f64) -> Option<f64> {
    let mut pts: Vec<&RdPoint> = points.iter().collect();
    pts.sort_by(|a, b| {
        a.bits_per_sample
            .partial_cmp(&b.bits_per_sample)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Collapse duplicate rates (keep the first, i.e. lowest mse at that rate).
    let mut dedup: Vec<&RdPoint> = Vec::new();
    for p in pts {
        match dedup.last() {
            Some(last) if (last.bits_per_sample - p.bits_per_sample).abs() < 1e-12 => {}
            _ => dedup.push(p),
        }
    }
    if dedup.is_empty() {
        return None;
    }

    let first = dedup[0];
    let last = *dedup.last().unwrap();
    if target_rate < first.bits_per_sample || target_rate > last.bits_per_sample {
        return None;
    }

    if (target_rate - first.bits_per_sample).abs() < 1e-12 {
        return Some(first.mse);
    }

    for w in dedup.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (b.bits_per_sample - a.bits_per_sample).abs() < 1e-12 {
            continue;
        }
        if target_rate >= a.bits_per_sample && target_rate <= b.bits_per_sample {
            let frac = (target_rate - a.bits_per_sample) / (b.bits_per_sample - a.bits_per_sample);
            let mse = a.mse + frac * (b.mse - a.mse);
            return Some(mse);
        }
    }
    None
}

/// A named RD curve, used by the matched-rate comparison table.
pub type LabeledCurve = (&'static str, Vec<RdPoint>);

/// One row of a matched-rate table: a target rate and each curve's
/// distortion interpolated at it (`None` when the curve never reaches the
/// target — i.e. ranking by that row would require extrapolation).
pub type MatchedRateRow = Vec<(String, Option<f64>)>;

/// A matched-rate comparison table: target rate → per-curve distortions.
pub type MatchedRateTable = Vec<(f64, MatchedRateRow)>;

/// For every target rate, report each curve's distortion **interpolated
/// within its own sampled range**; curves that never reach a target (would
/// need extrapolation) are reported as `None`. The harness refuses to rank
/// anything that needed extrapolation.
pub fn matched_rate_table(curves: &[LabeledCurve], targets: &[f64]) -> MatchedRateTable {
    targets
        .iter()
        .map(|&target| {
            let row = curves
                .iter()
                .map(|(name, pts)| (name.to_string(), interpolated_distortion(pts, target)))
                .collect();
            (target, row)
        })
        .collect()
}

/// Validate a curve as a *measurement* (not correctness — see the module
/// docs): all values finite, MSE ≥ 0, rate ≥ 0, rate sorted non-decreasing.
/// A curve that fails this cannot meaningfully be interpolated; callers use
/// it as a guard before trusting `matched_rate_table`.
pub fn validate_curve(points: &[RdPoint]) -> Result<(), String> {
    let mut prev_rate = -1.0f64;
    for (i, p) in points.iter().enumerate() {
        if !p.bits_per_sample.is_finite() || !p.mse.is_finite() {
            return Err(format!("non-finite point {i}: {p:?}"));
        }
        if p.bits_per_sample < 0.0 || p.mse < 0.0 {
            return Err(format!("negative value at point {i}: {p:?}"));
        }
        if p.bits_per_sample + 1e-12 < prev_rate {
            return Err(format!("decreasing rate at point {i}: {p:?}"));
        }
        prev_rate = p.bits_per_sample;
    }
    Ok(())
}

/// Check that every curve in [`matched_rate_table`]'s output actually had a
/// value in range at every target (i.e. nothing was silently extrapolated or
/// dropped). Returns the offending rate cells, or `Err(())` in the impossible
/// success-useful case — callers treat any returned cell as a harness
/// configuration error, since experiments *choose* targets inside the
/// sampled ranges.
pub fn missing_from_table(table: &MatchedRateTable) -> Vec<(f64, String)> {
    let mut missing = Vec::new();
    for (target, row) in table {
        for (name, value) in row {
            if value.is_none() {
                missing.push((*target, name.clone()));
            }
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::representations::{
        Baseline, BaselineCoeffs, BaselineConfig, BlockDct, BlockDctCoeffs, BlockDctConfig,
    };
    use crate::signals::spatial::gradient;
    use crate::uniform::{EntropyRate, LinearQuantizer};
    use tpt_media_representation::{SignalBuffer, SignalDims};

    #[test]
    fn interpolated_distortion_never_extrapolates() {
        let pts = vec![
            RdPoint {
                bits_per_sample: 1.0,
                mse: 0.50,
            },
            RdPoint {
                bits_per_sample: 2.0,
                mse: 0.25,
            },
            RdPoint {
                bits_per_sample: 4.0,
                mse: 0.10,
            },
        ];
        // In-range exact.
        assert!((interpolated_distortion(&pts, 2.0).unwrap() - 0.25).abs() < 1e-12);
        // In-range interpolated.
        let mid = interpolated_distortion(&pts, 1.5).unwrap();
        assert!((mid - 0.375).abs() < 1e-12);
        // Out of range either side -> None (no extrapolation).
        assert!(interpolated_distortion(&pts, 0.5).is_none());
        assert!(interpolated_distortion(&pts, 5.0).is_none());
    }

    #[test]
    fn rd_curve_is_deterministic_and_valid() {
        let dims = SignalDims {
            width: 16,
            height: 16,
            frames: 1,
        };
        let sig = gradient(16, 16);
        let mut out = SignalBuffer::zeroed(dims);
        let rep = Baseline;
        let cfg = BaselineConfig;
        let steps = default_steps();
        let curve_a = rd_curve(
            sig.view(),
            &rep,
            &cfg,
            &steps,
            LinearQuantizer::<BaselineCoeffs>::new,
            &EntropyRate,
            &mut out,
        );
        let curve_b = rd_curve(
            sig.view(),
            &rep,
            &cfg,
            &steps,
            LinearQuantizer::<BaselineCoeffs>::new,
            &EntropyRate,
            &mut out,
        );
        assert_eq!(
            curve_a, curve_b,
            "identical inputs must reproduce identical output"
        );
        validate_curve(&curve_a).expect("curve must be a valid measurement");
    }

    #[test]
    fn finer_quantization_costs_more_bits() {
        let sig = gradient(16, 16);
        let mut out = SignalBuffer::zeroed(sig.dims());
        let rep = BlockDct;
        let cfg = BlockDctConfig { block_size: 4 };
        let coarse = rd_curve(
            sig.view(),
            &rep,
            &cfg,
            &[0.5f32],
            LinearQuantizer::<BlockDctCoeffs>::new,
            &EntropyRate,
            &mut out,
        )[0];
        let fine = rd_curve(
            sig.view(),
            &rep,
            &cfg,
            &[0.0078f32],
            LinearQuantizer::<BlockDctCoeffs>::new,
            &EntropyRate,
            &mut out,
        )[0];
        assert!(fine.bits_per_sample > coarse.bits_per_sample);
        assert!(fine.mse <= coarse.mse);
    }

    #[test]
    fn matched_rate_table_reports_missing_rates() {
        let curves: Vec<LabeledCurve> = vec![(
            "one",
            vec![RdPoint {
                bits_per_sample: 2.0,
                mse: 0.2,
            }],
        )];
        let table = matched_rate_table(&curves, &[1.0, 2.0]);
        let missing = missing_from_table(&table);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, 1.0);
        assert_eq!(table[1].1[0].1.unwrap(), 0.2);
    }
}
