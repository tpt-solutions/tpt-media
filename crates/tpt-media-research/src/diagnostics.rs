//! Secondary and diagnostic measurements (`todo.md` §56 metric hierarchy).
//!
//! **None of these numbers may decide anything.** The only primary result is
//! the matched-rate RD comparison in [`crate::harness`]. These explain *why*
//! an RD curve looks the way it does (secondary), or probe a specific
//! mechanism without the ambition of comparing representations (diagnostic).
//!
//! * Secondary: empirical entropy of the *quantized* coefficient stream
//!   (`entropy_bits`), non-zero coefficient count, and energy compaction
//!   (top-`k` coefficients capturing 99 % of energy). These are reported
//!   per curve, per operating point.
//! * Diagnostic: the coefficient-**retention** curve — zero every coefficient
//!   below a magnitude threshold (via
//!   `tpt-media-representation::CoeffMagnitudes`) and watch distortion. This
//!   is only meaningful for linear-coefficient representations (`baseline`,
//!   `block_dct`, `temporal_dct`, `motion_aligned`); `patch_vq`'s decoder
//!   state is a dictionary + indices with no linear coefficient stream to
//!   prune, so it has no retention curve.

use tpt_media_representation::{
    CoeffMagnitudes, DistortionMetric, Mse, Representation, SignalBuffer, SignalView,
};

use crate::entropy::empirical_entropy_bits;

/// Secondary diagnostics over one quantized coefficient stream (the stream
/// is populated per representation; multi-stream representations concatenate
/// their transmitted streams).
#[derive(Clone, Copy, Debug)]
pub struct SecondaryDiagnostics {
    /// Empirical zeroth-order entropy of the quantized stream, in bits/sample.
    pub entropy_bits: f64,
    /// Coefficients with magnitude above the nominal zero threshold.
    pub nonzero: usize,
    /// Total coefficients in the stream.
    pub total: usize,
    /// Fewest top-magnitude coefficients needed to capture 99 % of the
    /// stream's energy.
    pub coefficients_for_99pct_energy: usize,
}

pub fn secondary_diagnostics(quantized: &[i32]) -> SecondaryDiagnostics {
    let mags: Vec<f32> = quantized.iter().map(|&i| i as f32).collect();
    let report = tpt_media_representation::SparsityReport::compute(&mags, 1e-6);
    SecondaryDiagnostics {
        entropy_bits: empirical_entropy_bits(quantized),
        nonzero: report.nonzero,
        total: report.total,
        coefficients_for_99pct_energy: report.coefficients_for_energy_fraction(0.99),
    }
}

/// One point on a diagnostic (never-primary) retention curve.
#[derive(Clone, Copy, Debug)]
pub struct RetentionPoint {
    /// Magnitude threshold (coefficients below it were zeroed).
    pub threshold: f32,
    /// Fraction of coefficients retained after zeroing.
    pub retained_fraction: f64,
    /// MSE after re-synthesis with zeroed coefficients.
    pub mse: f64,
}

/// Sweep the coefficient-retention curve of a linear-coefficient
/// representation: clone the (dequantized) `Coeffs`, zero every coefficient
/// with magnitude `< threshold`, re-synthesize, measure. `Coeffs` must be
/// `Clone` (all of this crate's are).
///
/// **Diagnostic only** — zeroing coefficients is *not* an encoding strategy
/// and never counts as a rate; the curve is for understanding, not ranking.
pub fn retention_curve<R>(
    signal: SignalView<'_>,
    rep: &R,
    config: &R::Config,
    coeffs: R::Coeffs,
    thresholds: &[f32],
    out: &mut SignalBuffer,
) -> Vec<RetentionPoint>
where
    R: Representation,
    R::Coeffs: CoeffMagnitudes + Clone,
{
    let total = coeffs.coefficient_magnitudes().len();
    thresholds
        .iter()
        .map(|&th| {
            let mut c = coeffs.clone();
            let retained = c.retain_above_threshold(th);
            rep.synthesize(&c, config, out.view_mut());
            let mse = Mse.distortion(signal, out.view());
            RetentionPoint {
                threshold: th,
                retained_fraction: if total == 0 {
                    1.0
                } else {
                    retained as f64 / total as f64
                },
                mse,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::representations::{Baseline, BaselineCoeffs, BaselineConfig};
    use crate::signals::spatial::gradient;

    #[test]
    fn sparsity_report_helpers_match_percentile_intent() {
        // One dominant coefficient => entropy low, needs just 1 for 99% energy.
        let q = vec![100i32, 1, -1, 1, -1, 1];
        let d = secondary_diagnostics(&q);
        assert!(d.nonzero == 6 && d.total == 6);
        assert!(d.coefficients_for_99pct_energy <= 3);
        assert!(d.entropy_bits < 2.0);
    }

    #[test]
    fn retention_curve_is_diagnostic_not_primary() {
        let sig = gradient(8, 8);
        let rep = Baseline;
        let cfg = BaselineConfig;
        let coeffs = BaselineCoeffs(sig.data().to_vec());
        let mut out = SignalBuffer::zeroed(sig.dims());
        let curve = retention_curve(
            sig.view(),
            &rep,
            &cfg,
            coeffs.clone(),
            &[0.0, 0.5, 2.0],
            &mut out,
        );
        assert_eq!(curve.len(), 3);
        // Zeroing nothing keeps everything; zeroing everything keeps nothing.
        assert!((curve[0].retained_fraction - 1.0).abs() < 1e-9);
        assert_eq!(curve[2].retained_fraction, 0.0);
        // All outputs finite (validity, not a claim about quality).
        assert!(curve.iter().all(|p| p.mse.is_finite()));
    }
}
