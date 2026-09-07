//! Distortion measurement. Extensible on purpose — PSNR is not claimed to be
//! a sufficient definition of quality; see `docs/representation-research.md`.

use crate::volume::SignalView;

/// A distortion/error measure `D(x, x_hat)`. Lower is better.
pub trait DistortionMetric {
    fn distortion(&self, original: SignalView<'_>, reconstructed: SignalView<'_>) -> f64;
}

/// Mean squared error.
pub struct Mse;

impl DistortionMetric for Mse {
    fn distortion(&self, original: SignalView<'_>, reconstructed: SignalView<'_>) -> f64 {
        assert_eq!(original.dims(), reconstructed.dims(), "distortion requires matching dimensions");
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
        let dims = SignalDims { width: 4, height: 4, frames: 1 };
        let buf = SignalBuffer::filled(dims, 0.5);
        assert_eq!(Mse.distortion(buf.view(), buf.view()), 0.0);
    }

    #[test]
    fn mse_matches_hand_computed_value() {
        let dims = SignalDims { width: 2, height: 2, frames: 1 };
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
}
