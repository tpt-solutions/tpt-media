//! `block_dct` — Avenue A: separable 2D block DCT-II.
//!
//! Each B×B tile of every frame is transformed by the single-frame separable
//! 2D DCT-II in the sibling `blockdct::BlockDct2D` (row pass + column pass of
//! `tpt-dsp-core`'s 1D `dct_ii`; inverse via `dct_iii` and the `4/B²` scale).
//! The coefficient stream is one real value per signal sample — the same
//! density as `baseline` — laid out tile-by-tile in each frame's own spatial
//! coordinates. Energy compaction is expected (a locally smooth field needs
//! few significant coefficients), which is exactly the hypothesis Phase 0
//! measures rather than assumes.

use tpt_media_representation::{Representation, SignalView, SignalViewMut};

use crate::uniform::LinearCoeffs;

use super::blockdct::BlockDct2D;

/// Block size `B` (a square tile; must divide the signal's width and height).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockDctConfig {
    pub block_size: usize,
}

/// One block-DCT coefficient per signal sample, frame-major.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockDctCoeffs(pub Vec<f32>);

impl LinearCoeffs for BlockDctCoeffs {
    fn stream(&self) -> &[f32] {
        &self.0
    }

    fn stream_mut(&mut self) -> &mut [f32] {
        &mut self.0
    }

    fn from_stream(stream: Vec<f32>) -> Self {
        BlockDctCoeffs(stream)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BlockDct;

impl Representation for BlockDct {
    type Config = BlockDctConfig;
    type Coeffs = BlockDctCoeffs;

    fn analyze(&self, signal: SignalView<'_>, config: &Self::Config) -> Self::Coeffs {
        let d = signal.dims();
        let mut dct = BlockDct2D::new(config.block_size);
        let mut stream = vec![0.0f32; d.samples()];
        let frame_len = d.frame_len();
        for t in 0..d.frames {
            dct.forward(
                signal.frame(t),
                d.width,
                d.height,
                &mut stream[t * frame_len..(t + 1) * frame_len],
            );
        }
        BlockDctCoeffs(stream)
    }

    fn synthesize(&self, coeffs: &Self::Coeffs, config: &Self::Config, mut out: SignalViewMut<'_>) {
        let d = out.dims();
        assert_eq!(
            coeffs.0.len(),
            d.samples(),
            "block_dct coefficient stream must be one value per sample"
        );
        let mut dct = BlockDct2D::new(config.block_size);
        let frame_len = d.frame_len();
        for t in 0..d.frames {
            dct.inverse(
                &coeffs.0[t * frame_len..(t + 1) * frame_len],
                d.width,
                d.height,
                out.frame_mut(t),
            );
        }
    }
}

impl tpt_media_representation::CoeffMagnitudes for BlockDctCoeffs {
    fn coefficient_magnitudes(&self) -> Vec<f32> {
        self.0.iter().map(|c| c.abs()).collect()
    }

    fn retain_above_threshold(&mut self, threshold: f32) -> usize {
        let mut retained = 0;
        for c in self.0.iter_mut() {
            if c.abs() < threshold {
                *c = 0.0;
            } else {
                retained += 1;
            }
        }
        retained
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_representation::{SignalBuffer, SignalDims};

    fn smooth_volume(w: usize, h: usize, frames: usize) -> SignalBuffer {
        let dims = SignalDims {
            width: w,
            height: h,
            frames,
        };
        let data: Vec<f32> = (0..dims.samples())
            .map(|i| {
                let t = (i / (w * h)) as f32;
                let y = ((i / w) % h) as f32;
                let x = (i % w) as f32;
                let moving = ((x - 0.9 * t) / w as f32).rem_euclid(1.0);
                0.4 + 0.6 * (moving * std::f32::consts::TAU).sin() + 0.1 * (y / h as f32)
            })
            .collect();
        SignalBuffer::from_vec(dims, data)
    }

    #[test]
    fn block_dct_round_trips_in_tight_tolerance() {
        let sig = smooth_volume(16, 16, 3);
        let rep = BlockDct;
        let config = BlockDctConfig { block_size: 4 };
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
    fn smooth_field_has_lower_entropy_than_raw_samples() {
        let sig = smooth_volume(16, 16, 3);
        let rep = BlockDct;
        let config = BlockDctConfig { block_size: 8 };
        let coeffs = rep.analyze(sig.view(), &config);
        let raw = sig.data().iter().map(|s| s.abs()).collect::<Vec<f32>>();

        let step = crate::uniform::ScalarStep::new(0.05);
        let c_q: Vec<i32> = coeffs.0.iter().map(|c| step.index(*c)).collect();
        let r_q: Vec<i32> = raw.iter().map(|s| step.index(*s)).collect();
        let h_c = crate::entropy::empirical_entropy_bits(&c_q);
        let h_r = crate::entropy::empirical_entropy_bits(&r_q);
        assert!(
            h_c < h_r,
            "expected DCT entropy {h_c} below raw entropy {h_r}"
        );
    }
}
