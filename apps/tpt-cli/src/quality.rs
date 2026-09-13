//! Objective audio quality metrics (todo.md §37, audio subset).
//!
//! These are the measurement primitives behind `tpt benchmark`: signal-to-
//! noise ratio against a reference, and a log-spectral distance computed
//! over short-time magnitude spectra. Everything here is a pure function of
//! its inputs — no state, no randomness, no I/O — so two runs on the same
//! inputs produce identical numbers (todo.md §39).
//!
//! Conventions:
//!
//! * references and candidates are mono `f32` samples in the codec's own
//!   output scale ( Pulse/Aura emit the same ±32768-scale `f32` values that
//!   the 16-bit input encoded, so metrics are comparable across codecs);
//! * `snr_db` returns `Some(f64::INFINITY)` for a *bit-exact* round trip
//!   (zero residual energy) and `None` when the metric is undefined
//!   (length mismatch, or an all-silent reference whose SNR has no
//!   denominator). Callers that emit JSON must special-case the infinite
//!   value rather than serializing it.

use tpt_dsp_core::{fft, twiddles, C32};

/// Signal-to-noise ratio in dB of `test` against `reference`
/// (`10·log10(Σref² / Σ(ref−test)²)`).
///
/// Returns `None` when the slices differ in length or when `reference` is
/// (numerically) silent — a silent reference makes SNR undefined, not
/// infinite, so the caller decides what to report. Returns
/// `Some(f64::INFINITY)` when the residual is exactly zero (bit-exact).
pub fn snr_db(reference: &[f32], test: &[f32]) -> Option<f64> {
    if reference.len() != test.len() {
        return None;
    }
    let mut signal = 0f64;
    let mut error = 0f64;
    for (r, t) in reference.iter().zip(test.iter()) {
        let r = *r as f64;
        let d = r - *t as f64;
        signal += r * r;
        error += d * d;
    }
    if signal <= 0.0 {
        return None;
    }
    if error == 0.0 {
        return Some(f64::INFINITY);
    }
    Some(10.0 * (signal / error).log10())
}

/// RMS level of `samples` in dBFS (full-scale sine ≈ −3 dBFS), using the
/// conventional `20·log10(rms)` against a full-scale *square* wave, i.e.
/// dB relative to a ±1.0 peak square. Returns `None` for empty or
/// numerically silent input.
pub fn rms_db(samples: &[f32]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    if sum <= 0.0 {
        return None;
    }
    let rms = (sum / samples.len() as f64).sqrt();
    Some(20.0 * rms.log10())
}

/// Floor applied to magnitude spectra before the dB conversion in
/// [`log_spectral_distance_db`], so silent frames produce a large-but-
/// finite error instead of `-inf`.
const SPECTRAL_FLOOR: f32 = 1e-6;

/// Log-spectral distance in dB between `test` and `reference`: the frames
/// of both signals are Hann-windowed, transformed with
/// `tpt-dsp-core`'s FFT, and compared as `20·log10` magnitude spectra; the
/// reported value is the RMS of the per-bin dB error, averaged over all
/// frames (only the non-redundant lower half of each spectrum is used).
///
/// This is deliberately the simplest *symmetric* spectral metric: it says
/// nothing about masking or audibility (no perceptual model — see §37's
/// "perceptual metrics where practical" caveat), but it catches spectral
/// damage (band quantization holes, missing highs) that a time-domain SNR
/// can hide.
///
/// Returns `None` when the slices differ in length, or `fft_size` is not a
/// power of two / too small to be meaningful (< 64).
pub fn log_spectral_distance_db(reference: &[f32], test: &[f32], fft_size: usize) -> Option<f64> {
    if reference.len() != test.len() || !tpt_dsp_core::is_power_of_two(fft_size) || fft_size < 64 {
        return None;
    }
    let hop = fft_size / 2;
    if reference.len() < fft_size {
        return None;
    }
    let window: Vec<f32> = (0..fft_size)
        .map(|n| {
            // Periodic Hann: w[n] = 0.5·(1 − cos(2πn/N)); the periodic
            // variant keeps overlap-add of consecutive frames at 50% hop
            // constant (COLA), which is what the frame averaging below
            // assumes.
            0.5 * (1.0 - (core::f64::consts::TAU * n as f64 / fft_size as f64).cos()) as f32
        })
        .collect();

    // Scratch buffers, allocated once.
    let mut buf = vec![C32::new(0.0, 0.0); fft_size];
    let mut spec = vec![C32::new(0.0, 0.0); fft_size];
    let mut scratch = vec![C32::new(0.0, 0.0); fft_size];
    twiddles(fft_size, &mut scratch);

    let mut frames = 0u64;
    let mut squared_db_error = 0f64;
    let mut start = 0usize;
    while start + fft_size <= reference.len() {
        for (i, w) in window.iter().enumerate() {
            buf[i] = C32::new(reference[start + i] * w, 0.0);
        }
        fft(&buf, &mut spec, &mut scratch);
        // Lower half only: the upper half is the conjugate mirror for real
        // input and would just count every error twice.
        let half = fft_size / 2;
        let mut ref_mag = vec![0f64; half];
        for (k, slot) in ref_mag.iter_mut().enumerate() {
            let m = spec[k].norm();
            *slot = (m.max(SPECTRAL_FLOOR) as f64).log10() * 20.0;
        }
        for (i, w) in window.iter().enumerate() {
            buf[i] = C32::new(test[start + i] * w, 0.0);
        }
        fft(&buf, &mut spec, &mut scratch);
        let mut frame_sq = 0f64;
        for (k, r_db) in ref_mag.iter().enumerate() {
            let m = spec[k].norm();
            let t_db = (m.max(SPECTRAL_FLOOR) as f64).log10() * 20.0;
            frame_sq += (r_db - t_db) * (r_db - t_db);
        }
        squared_db_error += frame_sq / half as f64;
        frames += 1;
        start += hop;
    }
    if frames == 0 {
        return None;
    }
    Some((squared_db_error / frames as f64).sqrt())
}

/// Average encoded bitrate in bits/second: `bytes·8 · sample_rate /
/// sample_count`. Returns `None` when `sample_count` is zero (no audio to
/// attribute the bytes to).
pub fn bitrate_bits_per_second(
    encoded_bytes: u64,
    sample_count: u64,
    sample_rate: u32,
) -> Option<u64> {
    if sample_count == 0 {
        return None;
    }
    Some((encoded_bytes * 8).checked_mul(u64::from(sample_rate))? / sample_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (core::f32::consts::TAU * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn identical_signals_have_infinite_snr() {
        let x = sine(997.0, 48_000, 4096, 0.5);
        assert_eq!(snr_db(&x, &x), Some(f64::INFINITY));
    }

    #[test]
    fn more_noise_gives_lower_snr() {
        let x = sine(997.0, 48_000, 8192, 0.5);
        let quiet: Vec<f32> = x
            .iter()
            .enumerate()
            .map(|(i, v)| v + (i % 8) as f32 / 8_000.0)
            .collect();
        let loud: Vec<f32> = x
            .iter()
            .enumerate()
            .map(|(i, v)| v + (i % 8) as f32 / 800.0)
            .collect();
        let s_quiet = snr_db(&x, &quiet).expect("defined");
        let s_loud = snr_db(&x, &loud).expect("defined");
        assert!(
            s_quiet > s_loud + 10.0,
            "quiet {s_quiet} dB should beat loud {s_loud} dB by >10 dB"
        );
    }

    #[test]
    fn snr_is_undefined_for_silent_references_and_mismatched_lengths() {
        let x = sine(997.0, 48_000, 1024, 0.5);
        assert_eq!(snr_db(&[0.0; 1024], &[0.0; 1024]), None);
        assert_eq!(snr_db(&x, &x[..512]), None);
    }

    #[test]
    fn identical_signals_have_zero_spectral_distance() {
        let x = sine(997.0, 48_000, 8192, 0.5);
        let d = log_spectral_distance_db(&x, &x, 1024).expect("defined");
        assert!(d < 1e-9, "identical signals: {d} dB");
    }

    #[test]
    fn spectral_distance_grows_with_spectral_damage() {
        let x = sine(997.0, 48_000, 16_384, 0.5);
        // Damping the highs (here: light one-pole low-pass, y += a(x−y))
        // damages the spectrum more the stronger the filter is.
        let damaged = |a: f32| {
            let mut y = 0f32;
            x.iter()
                .map(|v| {
                    y += a * (v - y);
                    y
                })
                .collect::<Vec<f32>>()
        };
        let mild = log_spectral_distance_db(&x, &damaged(0.9), 1024).expect("defined");
        let harsh = log_spectral_distance_db(&x, &damaged(0.25), 1024).expect("defined");
        assert!(
            harsh > mild + 3.0,
            "harsh damage {harsh} dB should exceed mild {mild} dB by >3 dB"
        );
    }

    #[test]
    fn spectral_distance_rejects_bad_shapes() {
        let x = sine(997.0, 48_000, 2048, 0.5);
        assert_eq!(log_spectral_distance_db(&x, &x[..1024], 1024), None);
        assert_eq!(
            log_spectral_distance_db(&x, &x, 1000),
            None,
            "not a power of two"
        );
        assert_eq!(log_spectral_distance_db(&x, &x, 32), None, "too small");
        assert_eq!(log_spectral_distance_db(&x[..512], &x[..512], 1024), None);
    }

    #[test]
    fn rms_and_bitrate_helpers() {
        assert_eq!(rms_db(&[]), None);
        assert_eq!(rms_db(&[0.0; 64]), None);
        let lvl = rms_db(&[0.5; 64]).expect("defined");
        assert!((lvl - (20.0 * 0.5f64.log10())).abs() < 1e-12);
        assert_eq!(bitrate_bits_per_second(1000, 0, 48_000), None);
        assert_eq!(bitrate_bits_per_second(1000, 500, 48_000), Some(768_000));
    }
}
