//! Linear Predictive Coding primitives: autocorrelation and the
//! Levinson-Durbin recursion for computing LPC coefficients from a signal.
//!
//! Used by `tpt-audio-aura` (lossless mode) and `tpt-audio-vox` (CELP).
//!
//! Both functions are deterministic and allocation-free for reasonable
//! prediction orders (≤ 64).

#[cfg(not(feature = "std"))]
use alloc::vec;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Compute the autocorrelation function `r[0..=order]` of the signal.
///
/// * `signal` — input samples.
/// * `order` — desired LPC order. `r.len() == order + 1` on return.
pub fn autocorrelation(signal: &[f32], order: usize) -> Vec<f32> {
    let mut r = vec![0.0f32; order + 1];
    for lag in 0..=order {
        let mut sum = 0.0f32;
        let n = signal.len();
        for i in 0..n.saturating_sub(lag) {
            sum += signal[i] * signal[i + lag];
        }
        r[lag] = sum;
    }
    r
}

/// Solve for the LPC coefficients of `order` from autocorrelation `r[0..=order]`.
///
/// On success `out` contains the prediction coefficients (length `order + 1`,
/// with `out[0] == 1.0`).
///
/// Returns `Err` if the autocorrelation is not positive-definite (`r[0] <= 0`).
pub fn levinson_durbin(r: &[f32], out: &mut [f32]) -> crate::Result<()> {
    let order = out.len() - 1;
    debug_assert_eq!(r.len(), order + 1);
    if r[0] <= 0.0 {
        return Err(crate::Error::RangeInvariant("autocorrelation not positive-definite"));
    }
    let mut a = vec![0.0f32; order + 1];
    let mut tmp = vec![0.0f32; order + 1];
    a[0] = 1.0;
    let mut e = r[0];
    for i in 1..=order {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let k = -acc / e;
        for j in 1..i {
            tmp[j] = a[j] + k * a[i - j];
        }
        for j in 1..i {
            a[j] = tmp[j];
        }
        a[i] = k;
        e *= 1.0 - k * k;
        if e <= 0.0 {
            return Err(crate::Error::RangeInvariant("zero residual energy"));
        }
    }
    out.copy_from_slice(&a);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autocorrelation_of_sine() {
        let sig: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.05).sin()).collect();
        let r = autocorrelation(&sig, 8);
        // r[0] is the energy; r[1] should be positive and bounded by r[0].
        assert!(r[0] > 0.0);
        assert!(r[1].abs() <= r[0]);
    }

    #[test]
    fn levinson_durbin_constant_signal() {
        // Constant signal: a constant can be predicted exactly with a[1] = 1.
        let sig = vec![1.0f32; 256];
        let r = autocorrelation(&sig, 2);
        let mut a = [0.0f32; 3];
        levinson_durbin(&r, &mut a).unwrap();
        assert!((a[0] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn levinson_durbin_rejects_bad_input() {
        let r = [0.0f32, 1.0];
        let mut a = [0.0f32; 2];
        assert!(levinson_durbin(&r, &mut a).is_err());
    }
}