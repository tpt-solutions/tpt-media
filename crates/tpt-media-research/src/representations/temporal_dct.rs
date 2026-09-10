//! `temporal_dct` — previous-frame prediction + block DCT on the residual.
//!
//! This is the *third oracle-experiment condition*, deliberately not one of
//! the three "core" representations: raw pixels (baseline), spatial DCT
//! (block_dct), and dictionary/VQ (patch_vq) — the temporal predictor is an
//! extra condition that only exploits the *known* temporal structure of the
//! synthetic moving-edge signal, for comparison against `motion_aligned`.
//!
//! # "Open-loop" prediction — and why the structural rule forces decoder-
//! # closed-loop reconstruction
//!
//! Per `todo.md` §56 the prediction is **open-loop** at the encoder: each
//! residual is computed against the *original* previous frame, never against
//! a reconstructed one. The residual is then block-DCT'd and quantized.
//!
//! The decoder, however, can only reconstruct frame `t` from its own state:
//! `x̂_t = x̂_{t-1} + inverse_DCT(residual_t)`. It never sees the originals.
//! That is exactly the "no free decoder-side information" rule
//! (`tpt-media-representation::Representation` contract docs): everything the
//! decoder needs must live in `Coeffs`, and it does (frame 0's coefficients
//! plus the residual coefficients are the *complete* state). The consequence
//! is *drift*: when quantization is coarse, the encoder's open-loop reference
//! and the decoder's reconstructed reference diverge, and that shows up as
//! extra distortion which the RD curve reports honestly. This representation
//! could be upgraded to closed-loop (predict from the reconstructed previous
//! frame) — which would also match Vault's eventual prediction paradigm (§33)
//! — but that requires the quantizer to participate in `analyze`, which the
//! pipeline split deliberately forbids, so drift stays as documented.

use tpt_media_representation::{
    Quantizer, RateEstimator, Representation, SignalView, SignalViewMut,
};

use crate::entropy::empirical_total_bits;
use crate::uniform::{dequantize_stream, quantize_stream, ScalarStep};

use super::blockdct::BlockDct2D;

/// Block size `B` shared by the frame-0 and residual transforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TemporalDctConfig {
    pub block_size: usize,
}

/// `frame0` is a block-DCT coefficient stream for frame 0 (one value per
/// sample of that frame); `residual` is the concatenated block-DCT stream of
/// the prediction residuals for frames `1..frames`.
#[derive(Clone, Debug, PartialEq)]
pub struct TemporalDctCoeffs {
    pub frame0: Vec<f32>,
    pub residual: Vec<f32>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TemporalDct;

impl Representation for TemporalDct {
    type Config = TemporalDctConfig;
    type Coeffs = TemporalDctCoeffs;

    fn analyze(&self, signal: SignalView<'_>, config: &Self::Config) -> Self::Coeffs {
        let d = signal.dims();
        let mut dct = BlockDct2D::new(config.block_size);
        let frame_len = d.frame_len();

        let mut frame0 = vec![0.0f32; frame_len];
        dct.forward(signal.frame(0), d.width, d.height, &mut frame0);

        let mut residual = vec![0.0f32; (d.frames - 1) * frame_len];
        let mut resid_frame = vec![0.0f32; frame_len];
        for t in 1..d.frames {
            for (i, rf) in resid_frame.iter_mut().enumerate() {
                *rf = signal.frame(t)[i] - signal.frame(t - 1)[i];
            }
            dct.forward(
                &resid_frame,
                d.width,
                d.height,
                &mut residual[(t - 1) * frame_len..t * frame_len],
            );
        }
        TemporalDctCoeffs { frame0, residual }
    }

    fn synthesize(&self, coeffs: &Self::Coeffs, config: &Self::Config, mut out: SignalViewMut<'_>) {
        let d = out.dims();
        let frame_len = d.frame_len();
        assert_eq!(
            coeffs.frame0.len(),
            frame_len,
            "frame0 coefficient count must equal one frame's samples"
        );
        assert_eq!(
            coeffs.residual.len(),
            (d.frames - 1) * frame_len,
            "residual coefficient count must equal (frames-1) one-frame streams"
        );

        let mut dct = BlockDct2D::new(config.block_size);
        let mut prev = vec![0.0f32; frame_len];
        let mut cur = vec![0.0f32; frame_len];

        dct.inverse(&coeffs.frame0, d.width, d.height, &mut prev);
        out.frame_mut(0).copy_from_slice(&prev);
        for t in 1..d.frames {
            dct.inverse(
                &coeffs.residual[(t - 1) * frame_len..t * frame_len],
                d.width,
                d.height,
                &mut cur,
            );
            for i in 0..frame_len {
                cur[i] += prev[i];
            }
            out.frame_mut(t).copy_from_slice(&cur);
            prev.copy_from_slice(&cur);
            // Zero `cur` so the next frame's inverse starts clean (inverse
            // overwrites every element, so this is for clarity only).
            cur.iter_mut().for_each(|c| *c = 0.0);
        }
    }
}

impl tpt_media_representation::CoeffMagnitudes for TemporalDctCoeffs {
    fn coefficient_magnitudes(&self) -> Vec<f32> {
        // The "coefficient stream" is the prediction-error stream plus the
        // intra anchor (frame 0). Side information proper (none here) is
        // excluded; both of these are transmitted coefficients.
        self.frame0
            .iter()
            .chain(self.residual.iter())
            .map(|c| c.abs())
            .collect()
    }

    fn retain_above_threshold(&mut self, threshold: f32) -> usize {
        let mut retained = 0;
        for stream in [&mut self.frame0, &mut self.residual] {
            for c in stream.iter_mut() {
                if c.abs() < threshold {
                    *c = 0.0;
                } else {
                    retained += 1;
                }
            }
        }
        retained
    }
}

/// Quantized form: both coefficient streams are uniformly scalar-quantized
/// with the same step; nothing else exists to transmit.
#[derive(Clone, Debug, PartialEq)]
pub struct TemporalDctQuantized {
    pub frame0: Vec<i32>,
    pub residual: Vec<i32>,
}

#[derive(Clone, Copy, Debug)]
pub struct TemporalDctQuantizer {
    step: ScalarStep,
}

impl TemporalDctQuantizer {
    /// # Panics
    ///
    /// Panics if `step` is not positive and finite.
    pub fn new(step: f32) -> Self {
        TemporalDctQuantizer {
            step: ScalarStep::new(step),
        }
    }
}

impl Quantizer<TemporalDctCoeffs> for TemporalDctQuantizer {
    type Quantized = TemporalDctQuantized;

    fn quantize(&self, coeffs: &TemporalDctCoeffs) -> Self::Quantized {
        TemporalDctQuantized {
            frame0: quantize_stream(&coeffs.frame0, &self.step),
            residual: quantize_stream(&coeffs.residual, &self.step),
        }
    }

    fn dequantize(&self, quantized: &Self::Quantized) -> TemporalDctCoeffs {
        TemporalDctCoeffs {
            frame0: dequantize_stream(&quantized.frame0, &self.step),
            residual: dequantize_stream(&quantized.residual, &self.step),
        }
    }
}

/// Sums **both** transmitted streams (frame-0 anchor + residuals) — the whole
/// quantized state.
pub struct TemporalDctRate;

impl RateEstimator<TemporalDctQuantized> for TemporalDctRate {
    fn estimate_bits(&self, quantized: &TemporalDctQuantized) -> f64 {
        empirical_total_bits(&quantized.frame0) + empirical_total_bits(&quantized.residual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::spatiotemporal::moving_edge;
    use tpt_media_representation::{SignalBuffer, SignalDims};

    #[test]
    fn residual_round_trip_reconstructs_within_tolerance() {
        // Constant-velocity edge: after the motion-free predictor the residual
        // is non-trivial only on the leading/trailing strip of coefficients.
        let (sig, _) = moving_edge(16, 8, 5, 1.0, 0.0);
        let rep = TemporalDct;
        let config = TemporalDctConfig { block_size: 4 };
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
    fn temporal_prediction_cuts_signal_domain_residual_energy() {
        // vx=1 pure translation of a step edge: the *signal-domain* prediction
        // residual is nonzero only in the one-column strip framed between t-1
        // and t (the edge moved one pixel), so its energy is a tiny fraction
        // of a whole frame's. (Measured in the signal domain, not on the DCT
        // coefficients — the 2D DCT engine is not energy-normalized, so
        // coefficient energy isn't comparable to raw sample energy.)
        let (sig, _) = moving_edge(16, 8, 4, 1.0, 0.0);
        let mut resid_energy = 0.0f32;
        for t in 1..sig.dims().frames {
            for (a, b) in sig
                .view()
                .frame(t)
                .iter()
                .zip(sig.view().frame(t - 1).iter())
            {
                resid_energy += (a - b) * (a - b);
            }
        }
        let frame_energy: f32 = sig.data().iter().map(|s| s * s).sum();
        assert!(
            resid_energy < 0.2 * frame_energy,
            "residual {resid_energy} should be a small fraction of frame energy {frame_energy}"
        );
    }

    #[test]
    fn single_frame_temporal_dct_matches_block_dct() {
        let dims = SignalDims {
            width: 8,
            height: 8,
            frames: 1,
        };
        let data: Vec<f32> = (0..dims.samples())
            .map(|i| (i as f32 * 0.7).sin())
            .collect();
        let sig = SignalBuffer::from_vec(dims, data);

        let bd = super::super::block_dct::BlockDct;
        let bd_cfg = super::super::block_dct::BlockDctConfig { block_size: 4 };
        let bd_coeffs = bd.analyze(sig.view(), &bd_cfg);

        let td = TemporalDct;
        let td_cfg = TemporalDctConfig { block_size: 4 };
        let td_coeffs = td.analyze(sig.view(), &td_cfg);

        assert_eq!(td_coeffs.frame0, bd_coeffs.0);
        assert!(td_coeffs.residual.is_empty());
    }
}
