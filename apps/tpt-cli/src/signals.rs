//! Deterministic synthetic audio corpus (todo.md §36, audio subset).
//!
//! No real speech/music recordings are available in this repository (and
//! binary fixtures are deliberately avoided), so the corpus is *generated*:
//! every signal here is a closed-form or seeded function of its arguments,
//! reproducible byte-for-byte on every platform (todo.md §39). The set
//! covers the corpus categories that can be synthesized honestly:
//!
//! * silence — the degenerate input every codec must pass through
//! * synthetic tonal material — pure sines, harmonic squares
//! * sweeps — energy that moves across the band
//! * noise — full-band, worst-case for transform quantizers
//! * transient-heavy material — sparse impulse/click trains
//! * pathological inputs — full-scale DC, full-scale alternating
//!   (Nyquist) square, i.e. the extremes of the representable range
//!
//! Real speech and music are *not* approximated here; §36's speech/music
//! items stay open until a real corpus is committed under `tests/corpus/`.

/// SplitMix64 step function: the corpus's only randomness source. Seeded
/// and closed-form, so any "random" signal is fully reproducible.
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform `f32` in `[-amp, amp)` from a SplitMix64 stream.
fn uniform(state: &mut u64, amp: f32) -> f32 {
    // Top 24 bits → uniform u24 → scale into [-amp, amp).
    let u = (splitmix64(state) >> 40) as f32 / (1u32 << 24) as f32;
    (u * 2.0 - 1.0) * amp
}

/// Digital silence, `n` samples.
pub fn silence(n: usize) -> Vec<f32> {
    vec![0.0; n]
}

/// Pure sine at `freq` Hz, amplitude `amp`, `n` samples at `rate`.
pub fn sine(rate: u32, freq: f32, n: usize, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| amp * (core::f32::consts::TAU * freq * i as f32 / rate as f32).sin())
        .collect()
}

/// Harmonic-rich square wave at `freq` Hz (first 15 odd partials summed),
/// amplitude `amp` — tonal material with real spectral content, unlike the
/// single-partial [`sine`].
pub fn square(rate: u32, freq: f32, n: usize, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let mut v = 0f32;
            for k in 1..=15 {
                let f = 2.0 * k as f32 - 1.0;
                v += (core::f32::consts::TAU * freq * f * t).sin() / f;
            }
            amp * v * (2.0 / core::f32::consts::PI)
        })
        .collect()
}

/// Linear chirp from `f0` to `f1` Hz over `n` samples at `rate`.
pub fn sweep(rate: u32, f0: f32, f1: f32, n: usize, amp: f32) -> Vec<f32> {
    let duration = n as f32 / rate as f32;
    let mut phase = 0f64;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let f = f0 + (f1 - f0) * t / duration;
            phase += core::f64::consts::TAU * f as f64 / rate as f64;
            amp * (phase as f32).sin()
        })
        .collect()
}

/// Full-band uniform noise, amplitude `amp`, from SplitMix64 `seed`.
pub fn noise(seed: u64, n: usize, amp: f32) -> Vec<f32> {
    let mut state = seed;
    (0..n).map(|_| uniform(&mut state, amp)).collect()
}

/// Transient-heavy material: sparse broadband clicks (a sharp impulse
/// followed by a short exponential decay), roughly `per_second` clicks per
/// second, amplitudes from SplitMix64 `seed`.
pub fn click_train(rate: u32, seed: u64, n: usize, per_second: u32, amp: f32) -> Vec<f32> {
    let mut state = seed;
    let mut out = vec![0f32; n];
    let gap = (rate as usize / per_second.max(1) as usize).max(1);
    let decay_len = (rate as usize / 50).max(8); // ~20 ms ring-down
    let mut i = gap / 2;
    while i < n {
        let click_amp = uniform(&mut state, amp);
        for (k, slot) in out[i..(i + decay_len).min(n)].iter_mut().enumerate() {
            let decay = (-(k as f32) / (decay_len as f32 / 6.0)).exp();
            *slot += click_amp * decay * (k as f32 * 0.7).sin();
        }
        i += gap;
    }
    out
}

/// Pathological: full-scale constant (DC at ~full scale). Worst case for
/// transform codecs that window across the discontinuity at block edges.
pub fn dc_fullscale(n: usize) -> Vec<f32> {
    vec![0.98; n]
}

/// Pathological: alternating ±0.98 — a full-scale Nyquist square. The
/// largest inter-sample swing a 16-bit path can carry; windowed transforms
/// see maximal energy at exactly the band edge.
pub fn alternating_fullscale(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| if i % 2 == 0 { 0.98 } else { -0.98 })
        .collect()
}

/// A little bit of everything: sine + square + noise + clicks mixed at
/// low level — the closest synthetic stand-in for "real programme
/// material" (still honest about *not* being music).
pub fn mixed(rate: u32, seed: u64, n: usize) -> Vec<f32> {
    let mut state = seed;
    let tone = sine(rate, 440.0, n, 0.35);
    let harmonics = square(rate, 220.0, n, 0.15);
    let hiss = noise(0xDEAD_BEEF ^ seed, n, 0.05);
    let clicks = click_train(rate, seed ^ 0x5555_5555, n, 2, 0.4);
    tone.iter()
        .zip(harmonics.iter())
        .zip(hiss.iter())
        .zip(clicks.iter())
        .map(|(((t, h), s), c)| {
            let v = t + h + s + c + uniform(&mut state, 0.005);
            v.clamp(-1.0, 1.0)
        })
        .collect()
}

/// Convert mono `f32` (±1.0 nominal) to little-endian S16 bytes, clamping
/// at the 16-bit rails — the exact convention `tpt encode` uses when it
/// hands WAV samples to the codecs.
pub fn to_s16_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Convert little-endian S16 bytes back to mono `f32` (±1.0 nominal),
/// inverse of [`to_s16_le_bytes`] up to the 16-bit quantization.
pub fn from_s16_le_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32767.0)
        .collect()
}

/// The named corpus: every generator above at `rate` Hz, each
/// `samples_per_signal` long. Order is fixed so test loops and benchmark
/// logs are reproducible.
pub fn corpus(seed: u64, rate: u32, samples_per_signal: usize) -> Vec<(&'static str, Vec<f32>)> {
    let n = samples_per_signal;
    vec![
        ("silence", silence(n)),
        ("sine_1khz", sine(rate, 1000.0, n, 0.5)),
        ("square_220hz", square(rate, 220.0, n, 0.5)),
        ("sweep_100_8k", sweep(rate, 100.0, 8_000.0, n, 0.5)),
        ("noise", noise(seed, n, 0.5)),
        ("clicks", click_train(rate, seed ^ 0xABCD, n, 8, 0.8)),
        ("mixed", mixed(rate, seed, n)),
        ("dc_fullscale", dc_fullscale(n)),
        ("alternating_fullscale", alternating_fullscale(n)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generators_are_deterministic() {
        let a = corpus(42, 48_000, 2048);
        let b = corpus(42, 48_000, 2048);
        for ((na, sa), (nb, sb)) in a.iter().zip(b.iter()) {
            assert_eq!(na, nb);
            assert_eq!(sa, sb, "signal {na} must be reproducible");
        }
        let c = corpus(43, 48_000, 2048);
        assert_ne!(a[4].1, c[4].1, "different seeds must change the noise");
    }

    #[test]
    fn generators_stay_in_range() {
        for (name, s) in corpus(7, 44_100, 4096) {
            assert!(s.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "{name}");
        }
    }

    #[test]
    fn s16_conversion_round_trips_and_clamps() {
        let x = vec![0.0, 0.5, -0.5, 2.0, -2.0];
        let bytes = to_s16_le_bytes(&x);
        assert_eq!(bytes.len(), x.len() * 2);
        let back = from_s16_le_bytes(&bytes);
        assert_eq!(back[0], 0.0);
        assert!((back[1] - 0.5).abs() < 1e-4);
        assert_eq!(back[3], 1.0, "positive values clamp at the rail");
        assert_eq!(back[4], -1.0, "negative values clamp at the rail");
    }

    #[test]
    fn click_train_has_transients() {
        let c = click_train(48_000, 1, 48_000, 8, 0.9);
        let peak = c.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.5, "clicks must actually peak: {peak}");
        let silent = c.iter().filter(|v| v.abs() < 1e-4).count();
        assert!(silent > c.len() / 2, "clicks must be sparse, not a tone");
    }
}
