//! Modified Discrete Cosine Transform with overlapping windows.
//!
//! Sizes supported: 256, 1024, 2048, 4096 (per the spec). Used by
//! `tpt-audio-pulse`, `tpt-audio-aura`, and the small DCT blocks used
//! inside `tpt-video-vault` for residual coding.
//!
//! The forward MDCT takes `2N` time-domain samples and produces `N`
//! frequency-domain coefficients. The inverse takes `N` coefficients and
//! produces `2N` time-domain samples that are accumulated into a
//! 50%-overlap window (perfect-reconstruction with a sine window).

/// Apply a forward MDCT of size `n` (power of two; supported: 256/1024/2048/4096).
///
/// * `input` — `2 * n` time-domain samples.
/// * `output` — `n` frequency-domain coefficients.
pub fn mdct_forward(input: &[f32], output: &mut [f32]) {
    let n = output.len();
    debug_assert_eq!(input.len(), 2 * n);
    let scale = (1.0 / n as f32).sqrt();
    for k in 0..n {
        let mut sum = 0.0f32;
        for i in 0..(2 * n) {
            let angle = core::f32::consts::PI / n as f32
                * (i as f32 + 0.5 + n as f32 / 2.0)
                * (k as f32 + 0.5);
            sum += input[i] * angle.cos();
        }
        output[k] = sum * scale;
    }
}

/// Apply an inverse MDCT of size `n`. `output` must be sized `2 * n`. The
/// caller is responsible for overlapping-and-adding successive `output`
/// buffers with a hop size of `n`.
pub fn mdct_inverse(input: &[f32], output: &mut [f32]) {
    let n = input.len();
    debug_assert_eq!(output.len(), 2 * n);
    let scale = (1.0 / n as f32).sqrt();
    for i in 0..(2 * n) {
        let mut sum = 0.0f32;
        for k in 0..n {
            let angle = core::f32::consts::PI / n as f32
                * (i as f32 + 0.5 + n as f32 / 2.0)
                * (k as f32 + 0.5);
            sum += input[k] * angle.cos();
        }
        output[i] = sum * scale;
    }
}

/// Apply a sine window in place. Used by `tpt-audio-pulse` and `tpt-audio-aura`
/// before/after MDCT to ensure perfect reconstruction with 50% overlap.
pub fn apply_sine_window(buf: &mut [f32]) {
    let n = buf.len();
    for (i, x) in buf.iter_mut().enumerate() {
        let w = (core::f32::consts::PI / n as f32 * (i as f32 + 0.5)).sin();
        *x *= w;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdct_round_trip_256() {
        let n = 256;
        let mut input = vec![0.0f32; 2 * n];
        for (i, x) in input.iter_mut().enumerate() {
            *x = ((i as f32) * 0.13).sin();
        }
        // Apply the analysis window.
        apply_sine_window(&mut input);
        let mut coeffs = vec![0.0f32; n];
        mdct_forward(&input, &mut coeffs);
        let mut recon = vec![0.0f32; 2 * n];
        mdct_inverse(&coeffs, &mut recon);
        apply_sine_window(&mut recon);
        // Perfect-reconstruction tolerance: cosine window is exact here.
        for i in 0..(2 * n) {
            let diff = (recon[i] - input[i]).abs();
            assert!(diff < 1e-2, "i={i} diff={diff}");
        }
    }
}