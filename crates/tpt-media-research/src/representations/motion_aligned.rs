//! `motion_aligned` — Avenue F: a coordinate warp `x' = x - v·t` **before**
//! the block-DCT representation.
//!
//! The encoder knows (or has estimated) a velocity `v`; each frame is sampled
//! at `(x + v_x·t, y + v_y·t)` (bilinear, toroidal) once for `analyze`, which
//! "undoes" uniform translation — a signal moving at exactly `v` becomes
//! stationary in the warped volume, and the block-DCT then operates on a
//! mostly-unmoving signal (cheap, sparse, low-entropy). `synthesize` undoes
//! the warp by sampling back at `(x - v_x·t, y - v_y·t)`.
//!
//! # `v` is *not* free
//!
//! The velocity is embedded in [`MotionAlignedCoeffs`]. [`MotionAlignedRate`]
//! charges it at full `f32` precision (64 bits total) on top of the DCT
//! coefficient entropy, so the oracle experiment handing this representation
//! the *true* velocity still pays for transmitting it — knowing the answer
//! exactly does not exempt it from the rate axis (`todo.md` §56).

use tpt_media_representation::{
    Quantizer, RateEstimator, Representation, SignalBuffer, SignalView, SignalViewMut,
};

use crate::entropy::empirical_total_bits;
use crate::uniform::{dequantize_stream, quantize_stream, ScalarStep};

use super::blockdct::BlockDct2D;

/// Block size `B` for the aligned volume's spatial DCT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotionAlignedConfig {
    /// Block edge `B` in pixels (square blocks).
    pub block_size: usize,
}

/// The velocity **and** the aligned volume's block-DCT coefficients — the
/// complete decoder state.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionAlignedCoeffs {
    /// The volume's (single) displacement.
    pub velocity: [f32; 2],
    /// Block-DCT coefficients of the velocity-aligned volume.
    pub dct: Vec<f32>,
}

/// The representation instance carries the encoder's velocity estimate.
/// For the oracle experiment this is the *true* velocity; in general use it
/// would come from a motion estimator. Whatever its source, the cost of
/// transmitting it is still paid via [`MotionAlignedRate`].
#[derive(Clone, Copy, Debug)]
pub struct MotionAligned {
    /// Per-block displacement applied before the transform.
    pub velocity: [f32; 2],
}

impl MotionAligned {
    /// A representation with every block moving by `(vx, vy)`.
    pub fn with_velocity(vx: f32, vy: f32) -> Self {
        MotionAligned { velocity: [vx, vy] }
    }
}

fn warp_volume(signal: SignalView<'_>, velocity: [f32; 2]) -> SignalBuffer {
    let d = signal.dims();
    let src = SignalBuffer::from_vec(d, signal.data().to_vec());
    let mut warped = SignalBuffer::zeroed(d);
    for t in 0..d.frames {
        let vx = velocity[0] * t as f32;
        let vy = velocity[1] * t as f32;
        for y in 0..d.height {
            for x in 0..d.width {
                let s = src.sample_bilinear_toroidal(x as f32 + vx, y as f32 + vy, t);
                warped.view_mut().set_sample(x, y, t, s);
            }
        }
    }
    warped
}

fn unwarp_volume(warped: &SignalBuffer, velocity: [f32; 2], mut out: SignalViewMut<'_>) {
    let d = out.dims();
    for t in 0..d.frames {
        let vx = velocity[0] * t as f32;
        let vy = velocity[1] * t as f32;
        for y in 0..d.height {
            for x in 0..d.width {
                let s = warped.sample_bilinear_toroidal(x as f32 - vx, y as f32 - vy, t);
                out.set_sample(x, y, t, s);
            }
        }
    }
}

impl Representation for MotionAligned {
    type Config = MotionAlignedConfig;
    type Coeffs = MotionAlignedCoeffs;

    fn analyze(&self, signal: SignalView<'_>, config: &Self::Config) -> Self::Coeffs {
        let d = signal.dims();
        let warped = warp_volume(signal, self.velocity);
        let mut dct = BlockDct2D::new(config.block_size);
        let mut stream = vec![0.0f32; d.samples()];
        let frame_len = d.frame_len();
        for t in 0..d.frames {
            dct.forward(
                warped.view().frame(t),
                d.width,
                d.height,
                &mut stream[t * frame_len..(t + 1) * frame_len],
            );
        }
        MotionAlignedCoeffs {
            velocity: self.velocity,
            dct: stream,
        }
    }

    fn synthesize(&self, coeffs: &Self::Coeffs, config: &Self::Config, out: SignalViewMut<'_>) {
        let d = out.dims();
        assert_eq!(
            coeffs.dct.len(),
            d.samples(),
            "motion_aligned coefficient stream must be one value per sample"
        );
        let mut dct = BlockDct2D::new(config.block_size);
        let frame_len = d.frame_len();
        let mut warped = SignalBuffer::zeroed(d);
        for t in 0..d.frames {
            dct.inverse(
                &coeffs.dct[t * frame_len..(t + 1) * frame_len],
                d.width,
                d.height,
                warped.view_mut().frame_mut(t),
            );
        }
        unwarp_volume(&warped, coeffs.velocity, out);
    }
}

impl tpt_media_representation::CoeffMagnitudes for MotionAlignedCoeffs {
    fn coefficient_magnitudes(&self) -> Vec<f32> {
        // The aligned-volume DCT stream; the embedded velocity is side
        // information, not a "coefficient" in the retention sense.
        self.dct.iter().map(|c| c.abs()).collect()
    }

    fn retain_above_threshold(&mut self, threshold: f32) -> usize {
        let mut retained = 0;
        for c in self.dct.iter_mut() {
            if c.abs() < threshold {
                *c = 0.0;
            } else {
                retained += 1;
            }
        }
        retained
    }
}

/// Coeffs are quantized exactly like any linear stream; the velocity passes
/// through at full `f32` precision (it is already "as transmitted").
#[derive(Clone, Debug, PartialEq)]
pub struct MotionAlignedQuantized {
    /// This block's displacement (as transmitted).
    pub velocity: [f32; 2],
    /// Transform-domain coefficients (quantized).
    pub dct: Vec<i32>,
}

#[derive(Clone, Copy, Debug)]
/// Quantizer for [`MotionAligned`] coefficients: displacement is
/// passed through at full `f32` precision (it is already "as
/// transmitted"), only the DCT coefficients are scalar-stepped.
pub struct MotionAlignedQuantizer {
    step: ScalarStep,
}

impl MotionAlignedQuantizer {
    /// # Panics
    ///
    /// Panics if `step` is not positive and finite.
    pub fn new(step: f32) -> Self {
        MotionAlignedQuantizer {
            step: ScalarStep::new(step),
        }
    }
}

impl Quantizer<MotionAlignedCoeffs> for MotionAlignedQuantizer {
    type Quantized = MotionAlignedQuantized;

    fn quantize(&self, coeffs: &MotionAlignedCoeffs) -> Self::Quantized {
        MotionAlignedQuantized {
            velocity: coeffs.velocity,
            dct: quantize_stream(&coeffs.dct, &self.step),
        }
    }

    fn dequantize(&self, quantized: &Self::Quantized) -> MotionAlignedCoeffs {
        MotionAlignedCoeffs {
            velocity: quantized.velocity,
            dct: dequantize_stream(&quantized.dct, &self.step),
        }
    }
}

/// Velocity at full `f32` precision (32 bits/component) **plus** the DC/T
/// coefficient stream's empirical entropy — the whole transmitted state.
pub struct MotionAlignedRate;

impl RateEstimator<MotionAlignedQuantized> for MotionAlignedRate {
    fn estimate_bits(&self, quantized: &MotionAlignedQuantized) -> f64 {
        64.0 + empirical_total_bits(&quantized.dct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::spatiotemporal::moving_edge;
    use tpt_media_representation::SignalBuffer;

    #[test]
    fn integer_velocity_warp_round_trips_losslessly() {
        // vx=1.0 moves the edge exactly one pixel per frame — the warp is a
        // pure sample permutation (bilinear at integer offsets == exact
        // samples), so the transform round trip is the only error source.
        let (sig, v) = moving_edge(16, 8, 5, 1.0, 0.0);
        let rep = MotionAligned::with_velocity(v[0], v[1]);
        let config = MotionAlignedConfig { block_size: 4 };
        let coeffs = rep.analyze(sig.view(), &config);
        let mut out = SignalBuffer::zeroed(sig.dims());
        rep.synthesize(&coeffs, &config, out.view_mut());
        let max_err = sig
            .data()
            .iter()
            .zip(out.data().iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 1e-3, "max_err={max_err}");
    }

    #[test]
    fn aligned_moving_edge_is_stationary_in_warped_domain() {
        let (sig, v) = moving_edge(16, 8, 5, 1.0, 0.0);
        let rep = MotionAligned::with_velocity(v[0], v[1]);
        let config = MotionAlignedConfig { block_size: 4 };
        let coeffs = rep.analyze(sig.view(), &config);
        // Frame-major coefficient stream: frame k's slice should equal frame
        // 0's slice (the edge is frozen in the aligned domain).
        let fl = sig.dims().frame_len();
        for t in 1..sig.dims().frames {
            let frame_t = &coeffs.dct[t * fl..(t + 1) * fl];
            let frame_0 = &coeffs.dct[..fl];
            let max_d = frame_t
                .iter()
                .zip(frame_0.iter())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(max_d < 1e-2, "frame {t} deviates by {max_d}");
        }
    }

    #[test]
    fn velocity_is_charged_by_rate_estimator() {
        // Entropy of a well-quantized aligned stream is near zero, so the
        // total bits must still be >= the 64-bit velocity charge.
        let (sig, v) = moving_edge(16, 8, 5, 1.0, 0.0);
        let rep = MotionAligned::with_velocity(v[0], v[1]);
        let config = MotionAlignedConfig { block_size: 4 };
        let coeffs = rep.analyze(sig.view(), &config);
        let q = MotionAlignedQuantizer::new(0.25).quantize(&coeffs);
        let bits = MotionAlignedRate.estimate_bits(&q);
        assert!(bits >= 64.0, "velocity must be charged: {bits}");
    }

    #[test]
    fn deterministic() {
        let (sig, v) = moving_edge(16, 8, 5, 1.0, 0.0);
        let rep = MotionAligned::with_velocity(v[0], v[1]);
        let config = MotionAlignedConfig { block_size: 4 };
        let a = rep.analyze(sig.view(), &config);
        let b = rep.analyze(sig.view(), &config);
        assert_eq!(a, b);
    }
}
