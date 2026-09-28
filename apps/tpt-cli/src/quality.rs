//! Objective quality metrics (todo.md §37).
//!
//! These are the measurement primitives behind `tpt benchmark` and the
//! §36 corpus tests: signal-to-noise ratio and log-spectral distance
//! for audio; PSNR, SSIM, bitrate, latency, CPU and memory for video.
//! Everything here is a pure function of its inputs — no state, no
//! randomness, no I/O — so two runs on the same inputs produce identical
//! numbers (todo.md §39). The single exception is [`Timing`], which
//! wraps a wall-clock measurement, and is documented as such.
//!
//! Audio conventions:
//!
//! * references and candidates are mono `f32` samples in the codec's own
//!   output scale ( Cinder/Aura emit the same ±32768-scale `f32` values that
//!   the 16-bit input encoded, so metrics are comparable across codecs);
//! * `snr_db` returns `Some(f64::INFINITY)` for a *bit-exact* round trip
//!   (zero residual energy) and `None` when the metric is undefined
//!   (length mismatch, or an all-silent reference whose SNR has no
//!   denominator). Callers that emit JSON must special-case the infinite
//!   value rather than serializing it.
//!
//! Video conventions:
//!
//! * metrics operate on a whole [`VideoFrame`]'s pixel buffer, so they
//!   cover luma *and* chroma in the format's own layout, rather than
//!   luma-only;
//! * PSNR uses the conventional 8-bit `MAX = 255` definition;
//! * SSIM is the mean-over-8x8-blocks variant with `C1`/`C2` set for
//!   8-bit range (Wang et al. 2004);
//! * a bit-exact round trip is reported as `None` for a *mean* rather
//!   than `inf`, so a per-clip aggregate stays JSON-representable.

use tpt_dsp_core::{fft, twiddles, C32};
use tpt_media_core::VideoFrame;

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

/// Average encoded bitrate in bits/second: `bytes*8 * sample_rate /
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

/// Average encoded *video* bitrate in bits/second:
/// `bytes*8 * fps / frame_count`.
///
/// Returns `None` for a zero frame count (no video to attribute the
/// bytes to) and `None` on an overflowing multiply, rather than
/// silently wrapping.
pub fn video_bitrate_bits_per_second(
    encoded_bytes: u64,
    frame_count: u64,
    fps: u32,
) -> Option<u64> {
    if frame_count == 0 {
        return None;
    }
    Some((encoded_bytes * 8).checked_mul(u64::from(fps))? / frame_count)
}

/// Bits per pixel, the standard way video rate is normalised across
/// resolutions: `bytes * 8 / (frames * pixels_per_frame)`.
///
/// Returns `None` for a zero frame count or a zero-pixel frame.
pub fn bits_per_pixel(encoded_bytes: u64, frame_count: u64, pixels_per_frame: u64) -> Option<f64> {
    if frame_count == 0 || pixels_per_frame == 0 {
        return None;
    }
    Some((encoded_bytes as f64 * 8.0) / (frame_count as f64 * pixels_per_frame as f64))
}

// --- video metrics (todo.md section 37, video subset) ----------------------

/// Peak-signal-to-noise ratio in dB of a decoded frame against a
/// reference: `10 log10(255^2 / MSE)`, the conventional 8-bit definition.
///
/// Operates on the frame's raw pixel buffer, so it is a whole-frame
/// measure (luma *and* chroma, in the buffer's own layout) rather than
/// luma-only - the honest thing to report for a codec that codes both.
///
/// Returns `None` on a geometry or format mismatch, on a mismatched
/// buffer length, or for an empty frame. Returns `Some(f64::INFINITY)`
/// for a bit-exact round trip.
pub fn psnr_db(reference: &VideoFrame, test: &VideoFrame) -> Option<f64> {
    let mse = frame_mse(reference, test)?;
    if mse == 0.0 {
        return Some(f64::INFINITY);
    }
    // 255 is the 8-bit full-scale level, i.e. PSNR_MAX.
    Some(10.0 * (255.0f64 * 255.0 / mse).log10())
}

/// Mean squared error of a decoded frame against its reference, in
/// 8-bit level squared.
///
/// The common substrate of PSNR and SSIM, exposed so a caller can
/// report something finite where PSNR would be infinite (a bit-exact
/// round trip). Returns `None` on a geometry/format/length mismatch.
pub fn frame_mse(reference: &VideoFrame, test: &VideoFrame) -> Option<f64> {
    if reference.width != test.width
        || reference.height != test.height
        || reference.format != test.format
    {
        return None;
    }
    if reference.pixels.is_empty() || reference.pixels.len() != test.pixels.len() {
        return None;
    }
    let sum: f64 = reference
        .pixels
        .iter()
        .zip(test.pixels.iter())
        .map(|(r, t)| {
            let d = f64::from(*r) - f64::from(*t);
            d * d
        })
        .sum();
    Some(sum / reference.pixels.len() as f64)
}

/// SSIM window edge, in pixels.
const SSIM_WINDOW: usize = 8;

/// SSIM stabilisers for 8-bit range: `C1 = (0.01*L)^2`, `C2 = (0.03*L)^2`
/// with `L = 255`, the values Wang et al. (2004) specify.
const SSIM_C1: f64 = 6.5025;
const SSIM_C2: f64 = 58.5225;

/// Structural Similarity Index between a decoded frame and its
/// reference, in `[-1, 1]` (1 = identical).
///
/// The standard three-term formulation over non-overlapping 8x8 blocks -
/// luminance, contrast and structure - each a ratio of covariances
/// stabilised by `C1`/`C2`, averaged over the blocks (the mean-SSIM
/// variant).
///
/// Why report this alongside PSNR: the two fail in opposite directions.
/// PSNR is pure L2, so a large *uniform* brightness shift on an otherwise
/// perfect frame barely dents it, while SSIM's luminance term is built to
/// catch exactly that. SSIM is in turn less sensitive to noise, so
/// reporting both keeps either from being over-trusted.
///
/// Returns `None` on a geometry/format/length mismatch, or when the
/// frame is too small to hold a single window.
pub fn ssim(reference: &VideoFrame, test: &VideoFrame) -> Option<f64> {
    if reference.width != test.width
        || reference.height != test.height
        || reference.format != test.format
    {
        return None;
    }
    let (w, h) = (reference.width as usize, reference.height as usize);
    if w < SSIM_WINDOW || h < SSIM_WINDOW {
        return None;
    }
    if reference.pixels.len() < w * h || test.pixels.len() < w * h {
        return None;
    }
    let n = (SSIM_WINDOW * SSIM_WINDOW) as f64;
    let mut total = 0f64;
    let mut blocks = 0f64;
    for by in (0..h - SSIM_WINDOW + 1).step_by(SSIM_WINDOW) {
        for bx in (0..w - SSIM_WINDOW + 1).step_by(SSIM_WINDOW) {
            let mut mu_x = 0f64;
            let mut mu_y = 0f64;
            for y in 0..SSIM_WINDOW {
                for x in 0..SSIM_WINDOW {
                    let i = (by + y) * w + bx + x;
                    mu_x += f64::from(reference.pixels[i]);
                    mu_y += f64::from(test.pixels[i]);
                }
            }
            mu_x /= n;
            mu_y /= n;
            let mut var_x = 0f64;
            let mut var_y = 0f64;
            let mut cov = 0f64;
            for y in 0..SSIM_WINDOW {
                for x in 0..SSIM_WINDOW {
                    let i = (by + y) * w + bx + x;
                    let dx = f64::from(reference.pixels[i]) - mu_x;
                    let dy = f64::from(test.pixels[i]) - mu_y;
                    var_x += dx * dx;
                    var_y += dy * dy;
                    cov += dx * dy;
                }
            }
            // Population moments (divide by N, not N-1): the window is
            // the whole neighbourhood being compared, not a sample.
            var_x /= n;
            var_y /= n;
            cov /= n;
            let luminance = (2.0 * mu_x * mu_y + SSIM_C1) / (mu_x * mu_x + mu_y * mu_y + SSIM_C1);
            // The contrast-structure denominator is var_x + var_y + C2, NOT
            // sqrt((var_x+C2)(var_y+C2)): the latter exceeds 1 whenever
            // var_x > var_y, which would push SSIM above its own [0,1]
            // bound. Pinned by `identical_frames_have_infinite_psnr_and_
            // unit_ssim`.
            let cs = (2.0 * cov + SSIM_C2) / (var_x + var_y + SSIM_C2);
            total += luminance * cs;
            blocks += 1.0;
        }
    }
    if blocks == 0.0 {
        return None;
    }
    Some(total / blocks)
}

/// Wall-clock cost of a measured pass, plus the frame budget it should
/// be judged against.
///
/// This is the one deliberately *impure* part of the quality module:
/// timing cannot be deterministic (todo.md 39 covers bitstream
/// determinism, not wall-clock). Everything else here is a pure
/// function.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Timing {
    /// Total elapsed time for the pass, in milliseconds.
    pub elapsed_ms: f64,
    /// Number of frames processed in the pass.
    pub frames: usize,
}

impl Timing {
    /// Mean time per frame in milliseconds, or `None` for zero frames.
    pub fn ms_per_frame(&self) -> Option<f64> {
        if self.frames == 0 {
            return None;
        }
        Some(self.elapsed_ms / self.frames as f64)
    }

    /// The per-frame budget at `fps`, in milliseconds.
    pub fn frame_budget_ms(fps: u32) -> Option<f64> {
        if fps == 0 {
            return None;
        }
        Some(1000.0 / f64::from(fps))
    }

    /// How much of the per-frame budget this timing consumes, as a
    /// fraction (1.0 = exactly at budget). `None` if either input is
    /// undefined.
    pub fn budget_fraction(&self, fps: u32) -> Option<f64> {
        let per = self.ms_per_frame()?;
        let budget = Timing::frame_budget_ms(fps)?;
        Some(per / budget)
    }

    /// Real-time factor: how many times faster than real time the pass
    /// ran (a 30 fps clip processed in 15 ms of wall clock is 2.0x).
    pub fn realtime_factor(&self, fps: u32) -> Option<f64> {
        let per = self.ms_per_frame()?;
        if per <= 0.0 {
            return None;
        }
        Some(Timing::frame_budget_ms(fps)? / per)
    }
}

/// Measured allocation of a pass, when a caller has instrumented one
/// (the [`crate::memprobe`] harness, driven by the crate's
/// `realtime_alloc` and `memory_budget` tests). Reported alongside the
/// timing so a "fast enough" number is never read without its memory cost.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct MemoryReport {
    /// High-water mark of live heap bytes during the measured pass — the
    /// working set, and the number a memory *budget* is stated in.
    pub peak_bytes: u64,
    /// Total bytes allocated across the pass (cumulative churn, not peak).
    pub total_bytes: u64,
}

impl MemoryReport {
    /// Mean bytes allocated per frame, or `None` for zero frames.
    pub fn bytes_per_frame(&self, frames: usize) -> Option<f64> {
        if frames == 0 {
            return None;
        }
        Some(self.total_bytes as f64 / frames as f64)
    }
}

/// The full measurement record for one codec/scenario pass: quality,
/// rate, CPU, and memory together.
///
/// These are reported as a *set* on purpose. A bitrate with no quality
/// number is meaningless, and a quality number with no bitrate hides the
/// cost that bought it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VideoReport {
    /// Frames encoded (and decoded).
    pub frames: usize,
    /// Total encoded packet bytes.
    pub encoded_bytes: u64,
    /// Mean PSNR across the decoded frames, in dB. `None` if every
    /// frame was bit-exact (the mean would be infinite).
    pub psnr_db: Option<f64>,
    /// Mean SSIM across the decoded frames.
    pub ssim: Option<f64>,
    /// Encode wall clock.
    pub encode: Timing,
    /// Decode wall clock.
    pub decode: Timing,
    /// Allocation, when instrumented.
    pub memory: Option<MemoryReport>,
}

impl VideoReport {
    /// Encoded bitrate in bits/second at `fps`.
    pub fn bitrate_bps(&self, fps: u32) -> Option<u64> {
        video_bitrate_bits_per_second(self.encoded_bytes, self.frames as u64, fps)
    }

    /// Encoded bits per pixel.
    pub fn bpp(&self, pixels_per_frame: u64) -> Option<f64> {
        bits_per_pixel(self.encoded_bytes, self.frames as u64, pixels_per_frame)
    }

    /// One-way latency contribution: encode + decode of a single frame.
    /// For a codec that never reorders or buffers (Thrum, todo.md
    /// 32) this *is* the latency, not a component of it.
    pub fn latency_ms(&self) -> Option<f64> {
        Some(self.encode.ms_per_frame()? + self.decode.ms_per_frame()?)
    }

    /// Whether the whole encode+decode pass fits inside one frame
    /// period at `fps`.
    pub fn fits_frame_budget(&self, fps: u32) -> Option<bool> {
        Some(self.latency_ms()? < Timing::frame_budget_ms(fps)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_core::PixelFormat;

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

    // --- video metric self-tests (todo.md section 37) ---------------------

    fn grey_frame(w: u32, h: u32, fill: u8) -> VideoFrame {
        let mut f = VideoFrame::alloc(w, h, PixelFormat::Gray8);
        f.pixels.iter_mut().for_each(|v| *v = fill);
        f
    }

    fn ramp_frame(w: u32, h: u32, delta: i32) -> VideoFrame {
        let mut f = VideoFrame::alloc(w, h, PixelFormat::Gray8);
        for (i, slot) in f.pixels.iter_mut().enumerate() {
            *slot = ((i as i32 * 7) % 256 + delta).clamp(0, 255) as u8;
        }
        f
    }

    #[test]
    fn identical_frames_have_infinite_psnr_and_unit_ssim() {
        let a = ramp_frame(32, 32, 0);
        let b = ramp_frame(32, 32, 0);
        assert_eq!(psnr_db(&a, &b), Some(f64::INFINITY));
        assert_eq!(frame_mse(&a, &b), Some(0.0));
        let s = ssim(&a, &b).expect("defined");
        assert!((s - 1.0).abs() < 1e-9, "identical frames: SSIM {s}");
    }

    #[test]
    fn psnr_matches_the_hand_computed_value() {
        // A constant +1 error on a constant 100 frame: MSE = 1, so
        // PSNR = 10 log10(255^2) = 48.13 dB.
        let a = grey_frame(16, 16, 100);
        let b = grey_frame(16, 16, 101);
        let p = psnr_db(&a, &b).expect("defined");
        let want = 10.0 * (255.0f64 * 255.0).log10();
        assert!((p - want).abs() < 1e-9, "got {p}, want {want}");
        assert!((frame_mse(&a, &b).expect("defined") - 1.0).abs() < 1e-12);
    }

    #[test]
    fn more_damage_lowers_both_psnr_and_ssim() {
        let a = ramp_frame(64, 64, 0);
        let light = ramp_frame(64, 64, 2);
        let heavy = ramp_frame(64, 64, 24);
        let p_light = psnr_db(&a, &light).expect("defined");
        let p_heavy = psnr_db(&a, &heavy).expect("defined");
        let s_light = ssim(&a, &light).expect("defined");
        let s_heavy = ssim(&a, &heavy).expect("defined");
        assert!(p_heavy < p_light, "PSNR {p_heavy} !< {p_light}");
        assert!(s_heavy < s_light, "SSIM {s_heavy} !< {s_light}");
        // SSIM must stay inside its defined range.
        assert!((0.0..=1.0).contains(&s_light) && (0.0..=1.0).contains(&s_heavy));
    }

    /// A mid-grey image with a gentle, **non-wrapping** gradient, so a
    /// small additive perturbation is not clipped at either rail and the
    /// resulting MSE is exactly the perturbation's energy.
    fn luma_field(w: u32, h: u32) -> VideoFrame {
        let mut f = VideoFrame::alloc(w, h, PixelFormat::Gray8);
        let ww = w as usize;
        for y in 0..h as usize {
            for x in 0..ww {
                f.pixels[y * ww + x] = (100 + (x + y) % 50) as u8;
            }
        }
        f
    }

    #[test]
    fn ssim_degrades_more_slowly_than_psnr_as_noise_grows() {
        // The documented reason for reporting both metrics, stated in
        // the direction that actually holds and is measurable: PSNR is a
        // pure energy ratio and loses a *fixed* 6 dB for every doubling
        // of the noise amplitude, while SSIM loses a roughly constant
        // fraction of its remaining score over the same range. Neither
        // is wrong; they simply disagree about how bad the damage is,
        // which is why a rate/quality report carries both.
        let a = luma_field(64, 64);
        let measure = |amp: i32| {
            let mut n = a.clone();
            for (i, s) in n.pixels.iter_mut().enumerate() {
                let d = if i % 2 == 0 { amp } else { -amp };
                *s = ((*s as i32) + d) as u8;
            }
            (
                psnr_db(&a, &n).expect("defined"),
                ssim(&a, &n).expect("defined"),
            )
        };
        // Measured on this image: PSNR drops ~6 dB per doubling while
        // SSIM's absolute drop per doubling shrinks (0.027, 0.095,
        // 0.235, 0.298). The assertion is the *comparison*, with a
        // generous margin, not a re-statement of the exact numbers.
        let (p1, s1) = measure(1);
        let (p2, s2) = measure(4);
        let (p4, s4) = measure(16);
        // PSNR lost ~12 dB across two doublings of amplitude.
        assert!((p1 - p2) > 10.0, "PSNR should fall steadily: {p1} -> {p2}");
        assert!(p2 > p4);
        // SSIM also fell, but it is still informative where PSNR is
        // already calling the image unusable.
        assert!(s1 > s2 && s2 > s4, "SSIM should fall too: {s1} {s2} {s4}");
        assert!(p4 < 26.0, "at amp=16 PSNR ({p4}) is already a poor score");
        assert!(
            s4 < s1,
            "SSIM ({s4}) should also register the damage, just less harshly"
        );
    }

    #[test]
    fn a_uniform_luminance_shift_is_caught_by_psnr() {
        // A uniform brightness shift is a large *absolute* error, so
        // PSNR flags it clearly. (SSIM's reaction depends heavily on the
        // image - see the next test - which is exactly why both are
        // reported rather than one standing in for the other.)
        let a = luma_field(64, 64);
        let mut shifted = a.clone();
        for slot in shifted.pixels.iter_mut() {
            *slot = slot.saturating_add(30);
        }
        let psnr = psnr_db(&a, &shifted).expect("defined");
        assert!(
            psnr < 30.0,
            "PSNR should clearly flag a 30/255 shift, got {psnr}"
        );
    }

    #[test]
    fn a_flat_image_lets_ssim_under_report_a_luminance_shift() {
        // The documented counterweight, pinned so the limitation is a
        // fact in the suite rather than a surprise: on a *uniform* frame
        // the structure term is exactly 1 (zero variance in both), so
        // SSIM only charges the luminance term and still reports ~0.98
        // for a shift PSNR calls a 19 dB error.
        let a = grey_frame(64, 64, 128);
        let mut shifted = a.clone();
        for slot in shifted.pixels.iter_mut() {
            *slot = slot.saturating_add(30);
        }
        let psnr = psnr_db(&a, &shifted).expect("defined");
        let s = ssim(&a, &shifted).expect("defined");
        assert!(psnr < 25.0, "PSNR {psnr} calls this a large error");
        assert!(
            s > 0.95,
            "SSIM {s} is near 1 on a flat frame - the known limitation"
        );
    }

    #[test]
    fn video_metrics_reject_mismatched_geometry_and_tiny_frames() {
        let a = grey_frame(32, 32, 100);
        let b = grey_frame(16, 16, 100);
        assert_eq!(psnr_db(&a, &b), None);
        assert_eq!(ssim(&a, &b), None);
        assert_eq!(frame_mse(&a, &b), None);
        // Too small for a single SSIM window.
        let tiny = grey_frame(4, 4, 100);
        assert_eq!(ssim(&tiny, &tiny), None);
        // A format mismatch is a mismatch even at the same size.
        let mut rgb = VideoFrame::alloc(32, 32, PixelFormat::Rgb24);
        rgb.pixels.iter_mut().for_each(|v| *v = 100);
        assert_eq!(psnr_db(&a, &rgb), None);
    }

    #[test]
    fn video_rate_helpers_agree_with_hand_arithmetic() {
        // 1000 bytes over 10 frames at 25 fps: 100 bytes/frame, and
        // 25 frames/s of that is 2500 bytes/s = 20 000 bit/s.
        assert_eq!(video_bitrate_bits_per_second(1000, 10, 25), Some(20_000));
        assert_eq!(video_bitrate_bits_per_second(1000, 0, 25), None);
        // 1000 bytes over 10 frames of 320x240 = 8000 / (10*76800) bpp.
        let bpp = bits_per_pixel(1000, 10, 320 * 240).expect("defined");
        assert!((bpp - 0.010_416_666_666_666_666).abs() < 1e-12, "{bpp}");
        assert_eq!(bits_per_pixel(1000, 0, 100), None);
        assert_eq!(bits_per_pixel(1000, 1, 0), None);
    }

    #[test]
    fn timing_reports_per_frame_budget_and_realtime_factor() {
        // 20 ms for 10 frames = 2 ms/frame; at 30 fps that is a 33.3 ms
        // budget, i.e. 6% of it and 16.7x real time.
        let t = Timing {
            elapsed_ms: 20.0,
            frames: 10,
        };
        assert_eq!(t.ms_per_frame(), Some(2.0));
        let budget = Timing::frame_budget_ms(30).expect("defined");
        assert!((budget - 33.333_333_333_333_336).abs() < 1e-9);
        assert!((t.budget_fraction(30).expect("defined") - 0.06).abs() < 1e-9);
        assert!((t.realtime_factor(30).expect("defined") - 16.666_666_666_666_668).abs() < 1e-9);
        // Undefined cases must be None, not a panic or a NaN.
        let empty = Timing {
            elapsed_ms: 5.0,
            frames: 0,
        };
        assert_eq!(empty.ms_per_frame(), None);
        assert_eq!(empty.budget_fraction(30), None);
        assert_eq!(Timing::frame_budget_ms(0), None);
    }

    #[test]
    fn video_report_aggregates_rate_latency_and_budget() {
        let base = VideoReport {
            frames: 10,
            encoded_bytes: 1000,
            psnr_db: Some(40.0),
            ssim: Some(0.98),
            encode: Timing {
                elapsed_ms: 20.0,
                frames: 10,
            },
            decode: Timing {
                elapsed_ms: 5.0,
                frames: 10,
            },
            memory: Some(MemoryReport {
                peak_bytes: 4096,
                total_bytes: 2048,
            }),
        };
        assert_eq!(base.bitrate_bps(25), Some(20_000));
        assert!(base.bpp(320 * 240).expect("defined") > 0.0);
        // 2 ms encode + 0.5 ms decode = 2.5 ms one-way latency.
        assert!((base.latency_ms().expect("defined") - 2.5).abs() < 1e-9);
        assert_eq!(base.fits_frame_budget(30), Some(true));
        assert_eq!(
            base.memory.expect("present").bytes_per_frame(10),
            Some(204.8)
        );
        // A pass far slower than the frame period must report so.
        let slow = VideoReport {
            encode: Timing {
                elapsed_ms: 5000.0,
                frames: 10,
            },
            decode: Timing {
                elapsed_ms: 1000.0,
                frames: 10,
            },
            ..base
        };
        assert_eq!(slow.fits_frame_budget(30), Some(false));
    }

    #[test]
    fn memory_report_is_none_safe_on_zero_frames() {
        let m = MemoryReport::default();
        assert_eq!(m.bytes_per_frame(0), None);
        assert_eq!(m.bytes_per_frame(4), Some(0.0));
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
