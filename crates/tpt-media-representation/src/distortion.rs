//! Distortion measurement. Extensible on purpose — PSNR is not claimed to be
//! a sufficient definition of quality; see `docs/representation-research.md`.

use crate::volume::SignalView;

/// A distortion/error measure `D(x, x_hat)`. Lower is better.
pub trait DistortionMetric {
    /// The distortion between `original` and `reconstructed`; finite
    /// and non-negative, lower is better.
    fn distortion(&self, original: SignalView<'_>, reconstructed: SignalView<'_>) -> f64;
}

/// Mean squared error.
pub struct Mse;

impl DistortionMetric for Mse {
    fn distortion(&self, original: SignalView<'_>, reconstructed: SignalView<'_>) -> f64 {
        assert_eq!(
            original.dims(),
            reconstructed.dims(),
            "distortion requires matching dimensions"
        );
        let n = original.data().len() as f64;
        if n == 0.0 {
            return 0.0;
        }
        let sum_sq: f64 = original
            .data()
            .iter()
            .zip(reconstructed.data().iter())
            .map(|(&a, &b)| {
                let d = (a - b) as f64;
                d * d
            })
            .sum();
        sum_sq / n
    }
}

/// Root mean squared error.
pub struct Rmse;

impl DistortionMetric for Rmse {
    fn distortion(&self, original: SignalView<'_>, reconstructed: SignalView<'_>) -> f64 {
        Mse.distortion(original, reconstructed).sqrt()
    }
}

/// Peak Signal-to-Noise Ratio in dB, given an already-computed MSE and the
/// signal's peak value (e.g. `1.0` for a `[0,1]`-normalized synthetic
/// signal). Not a [`DistortionMetric`] itself (PSNR is a quality figure,
/// higher-is-better, unlike the lower-is-better convention of this trait) —
/// call this separately when reporting results.
///
/// Returns `f64::INFINITY` for a lossless (zero-MSE) reconstruction.
pub fn psnr_db(mse: f64, peak: f64) -> f64 {
    if mse <= 0.0 {
        return f64::INFINITY;
    }
    10.0 * (peak * peak / mse).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::{SignalBuffer, SignalDims};

    #[test]
    fn mse_of_identical_signals_is_zero() {
        let dims = SignalDims {
            width: 4,
            height: 4,
            frames: 1,
        };
        let buf = SignalBuffer::filled(dims, 0.5);
        assert_eq!(Mse.distortion(buf.view(), buf.view()), 0.0);
    }

    #[test]
    fn mse_matches_hand_computed_value() {
        let dims = SignalDims {
            width: 2,
            height: 2,
            frames: 1,
        };
        let a = SignalBuffer::from_vec(dims, vec![0.0, 0.0, 0.0, 0.0]);
        let b = SignalBuffer::from_vec(dims, vec![1.0, 1.0, 1.0, 1.0]);
        assert_eq!(Mse.distortion(a.view(), b.view()), 1.0);
        assert_eq!(Rmse.distortion(a.view(), b.view()), 1.0);
    }

    #[test]
    fn psnr_is_infinite_for_zero_mse_and_finite_otherwise() {
        assert_eq!(psnr_db(0.0, 1.0), f64::INFINITY);
        let db = psnr_db(0.01, 1.0);
        assert!(db.is_finite());
        assert!((db - 20.0).abs() < 1e-9);
    }

    /// §38: empty input. A volume with no samples has nothing to be wrong
    /// about, so `Mse` reports 0 — which `psnr_db` then reports as a lossless
    /// reconstruction. Both facts are pinned here, because the pair is the
    /// trap: a sweep that measured *nothing* is indistinguishable from one
    /// that measured *perfectly* unless the caller checks the sample count.
    #[test]
    fn an_empty_volume_measures_zero_error_and_so_reports_infinite_psnr() {
        let dims = SignalDims {
            width: 0,
            height: 0,
            frames: 0,
        };
        let buf = SignalBuffer::zeroed(dims);
        assert_eq!(Mse.distortion(buf.view(), buf.view()), 0.0);
        assert_eq!(Rmse.distortion(buf.view(), buf.view()), 0.0);
        // Not a bug: `mse = 0` is the documented lossless answer. The point is
        // that "no samples" also produces it, so the caller must check.
        assert_eq!(
            psnr_db(Mse.distortion(buf.view(), buf.view()), 1.0),
            f64::INFINITY
        );
    }

    /// §38: malformed input. A metric over mismatched volumes has no
    /// meaning — the sample counts differ, so `zip` would silently compare a
    /// prefix and ignore the rest. Rejected rather than answered.
    #[test]
    #[should_panic(expected = "distortion requires matching dimensions")]
    fn distortion_rejects_volumes_of_differing_dimensions() {
        let a = SignalBuffer::zeroed(SignalDims {
            width: 2,
            height: 2,
            frames: 1,
        });
        let b = SignalBuffer::zeroed(SignalDims {
            width: 3,
            height: 3,
            frames: 1,
        });
        Mse.distortion(a.view(), b.view());
    }

    #[test]
    #[should_panic(expected = "distortion requires matching dimensions")]
    fn rmse_inherits_the_same_dimension_check() {
        // `Rmse` is `Mse.sqrt()`, so the check is reached through it too;
        // pinning it stops a future refactor from dropping the guard.
        let a = SignalBuffer::zeroed(SignalDims {
            width: 2,
            height: 2,
            frames: 1,
        });
        let b = SignalBuffer::zeroed(SignalDims {
            width: 2,
            height: 2,
            frames: 2,
        });
        Rmse.distortion(a.view(), b.view());
    }

    /// §38: maximum values. `f32::MAX` against `f32::MIN` squares to infinity
    /// in `f32` before the `f64` accumulator ever sees it, so the answer is
    /// infinite rather than a huge finite number. Finite-but-wrong would be
    /// the worse outcome for a quality sweep, so the overflow is recorded.
    #[test]
    fn opposite_extreme_magnitudes_saturate_rather_than_report_a_finite_score() {
        let dims = SignalDims {
            width: 2,
            height: 1,
            frames: 1,
        };
        let hi = SignalBuffer::filled(dims, f32::MAX);
        let lo = SignalBuffer::filled(dims, f32::MIN);
        let mse = Mse.distortion(hi.view(), lo.view());
        assert!(
            !mse.is_finite(),
            "squaring a full-range difference overflows f32: mse={mse}"
        );
        assert_eq!(Rmse.distortion(hi.view(), lo.view()), f64::INFINITY);
    }

    /// `psnr_db` is documented as returning infinity for a non-positive MSE.
    /// A negative MSE is not reachable from `Mse` (a sum of squares), but the
    /// function is public and takes an `f64`, so the branch is pinned.
    #[test]
    fn psnr_treats_a_non_positive_mse_as_lossless() {
        assert_eq!(psnr_db(0.0, 1.0), f64::INFINITY);
        assert_eq!(psnr_db(-1.0, 1.0), f64::INFINITY);
    }

    #[test]
    fn psnr_of_a_zero_peak_signal_is_minus_infinity_unless_it_is_also_lossless() {
        // A zero-peak signal has no dynamic range, so any error is infinitely
        // bad relative to it; 0 dB of peak over 1.0 of error is -inf.
        assert_eq!(psnr_db(1.0, 0.0), f64::NEG_INFINITY);
        // ...and the degenerate 0/0 case is caught by the same guard.
        assert_eq!(psnr_db(0.0, 0.0), f64::INFINITY);
    }
}
