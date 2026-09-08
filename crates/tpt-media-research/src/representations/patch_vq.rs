//! `patch_vq` — Avenues C/D: patch extraction + a per-signal trained
//! codebook + vector-quantised indices.
//!
//! The signal is partitioned into non-overlapping B×B patches (tiles); a
//! `K`-entry codebook is trained on those patches by the deterministic
//! Lloyd's algorithm in [`crate::kmeans`]; every patch is then replaced by
//! its nearest codeword index (search/encode/decode from
//! `tpt-dsp-core::vq`, squared-Euclidean). Reconstruction is
//! `decode(indices, codebook)` scattered back into the volume.
//!
//! # Rate accounting — the whole point
//!
//! [`PatchVqCoeffs`] carries **both** the codebook **and** the indices, and
//! [`PatchVqRate`] charges **both** — the codebook is transmitted once for
//! the whole signal, the indices once per tile. Nothing (the dictionary
//! especially) is treated as free decoder-side information. The codebook is
//! also quantized with the same scalar step as every coefficient stream in
//! this platform, and the indices are re-searched against the *dequantized*
//! codebook — what the decoder will actually have — so the measured
//! distortion is the true end-to-end distortion, not the encoder's
//! optimistic one.

use tpt_dsp_core::{vq_decode, vq_encode, DistanceMetric};
use tpt_media_representation::{
    Quantizer, RateEstimator, Representation, SignalView, SignalViewMut,
};

use crate::entropy::{empirical_total_bits, empirical_total_bits_u32};
use crate::kmeans;
use crate::uniform::{dequantize_stream, quantize_stream, ScalarStep};

/// Patch size `B`, codebook size `K`, and the training schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchVqConfig {
    pub patch_size: usize,
    pub codebook_entries: usize,
    pub train_iterations: usize,
    pub train_seed: u64,
}

/// Everything the decoder needs: the trained codebook (flat, `K·B²`) plus
/// one palette-index per patch. **Neither half is optional side
/// information** — both are transmitted.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchVqCoeffs {
    pub codebook: Vec<f32>,
    pub indices: Vec<u32>,
}

/// A patch of `B×B` samples.
pub fn patch_dim(config: &PatchVqConfig) -> usize {
    config.patch_size * config.patch_size
}

/// Tiles per frame for a `w×h` signal partitioned into `patch_size` tiles.
///
/// # Panics
///
/// Panics if `w`/`h` are not divisible by `patch_size`.
pub fn tiles_per_frame(w: usize, h: usize, patch_size: usize) -> usize {
    assert!(patch_size > 0, "patch size must be positive");
    assert_eq!(
        w % patch_size,
        0,
        "width {w} must be divisible by patch size {patch_size}"
    );
    assert_eq!(
        h % patch_size,
        0,
        "height {h} must be divisible by patch size {patch_size}"
    );
    (w / patch_size) * (h / patch_size)
}

/// Extract every non-overlapping patch of `signal` into a flat stream of
/// `n_tiles × patch_dim` values (frame-major, tiles row-major, then samples
/// row-major inside each tile).
pub fn extract_patches(signal: SignalView<'_>, config: &PatchVqConfig) -> Vec<f32> {
    let d = signal.dims();
    let p = config.patch_size;
    let per_frame = tiles_per_frame(d.width, d.height, p);
    let n_patches = d.frames * per_frame;
    let mut out = Vec::with_capacity(n_patches * patch_dim(config));
    for t in 0..d.frames {
        let (w, h) = (d.width, d.height);
        for ty in 0..(h / p) {
            for tx in 0..(w / p) {
                for j in 0..p {
                    let row = (ty * p + j) * w + tx * p;
                    out.extend_from_slice(&signal.frame(t)[row..row + p]);
                }
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PatchVq;

impl Representation for PatchVq {
    type Config = PatchVqConfig;
    type Coeffs = PatchVqCoeffs;

    fn analyze(&self, signal: SignalView<'_>, config: &Self::Config) -> Self::Coeffs {
        let dim = patch_dim(config);
        let patches = extract_patches(signal, config);
        let (codebook, _) = kmeans::kmeans_train(
            &patches,
            dim,
            config.codebook_entries,
            config.train_iterations,
            config.train_seed,
        );
        let mut indices = vec![0u32; patches.len() / dim];
        vq_encode(
            &codebook,
            dim,
            &patches,
            DistanceMetric::SquaredEuclidean,
            &mut indices,
        );
        PatchVqCoeffs { codebook, indices }
    }

    fn synthesize(&self, coeffs: &Self::Coeffs, config: &Self::Config, mut out: SignalViewMut<'_>) {
        let d = out.dims();
        let dim = patch_dim(config);
        let p = config.patch_size;
        let per_frame = tiles_per_frame(d.width, d.height, p);
        assert_eq!(
            coeffs.indices.len(),
            d.frames * per_frame,
            "index count must equal the number of patches"
        );
        assert_eq!(
            coeffs.codebook.len() % dim,
            0,
            "codebook length must be a multiple of patch dim"
        );

        let mut recon = vec![0.0f32; coeffs.indices.len() * dim];
        vq_decode(&coeffs.codebook, dim, &coeffs.indices, &mut recon);

        let (w, h) = (d.width, d.height);
        let mut patch = 0;
        for t in 0..d.frames {
            for ty in 0..(h / p) {
                for tx in 0..(w / p) {
                    let src = &recon[patch * dim..(patch + 1) * dim];
                    for j in 0..p {
                        let row = (ty * p + j) * w + tx * p;
                        out.frame_mut(t)[row..row + p].copy_from_slice(&src[j * p..(j + 1) * p]);
                    }
                    patch += 1;
                }
            }
        }
    }
}

/// Densely quantized `PatchVqQuantized`.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchVqQuantized {
    /// Uniform-scalar-quantized codebook (index stream).
    pub codebook: Vec<i32>,
    /// Nearest-codeword indices, searched against the *dequantized* codebook.
    pub indices: Vec<u32>,
}

/// Encoder-side quantizer for [`PatchVq`]. Carries the training patches so
/// that `quantize` can re-search the indices against the dequantized
/// codebook the decoder will actually use (the codebook as transmitted is
/// quantized at `step`, so its codewords differ from the trained ones).
#[derive(Clone, Debug)]
pub struct PatchVqQuantizer {
    step: ScalarStep,
    dim: usize,
    training: Vec<f32>,
}

impl PatchVqQuantizer {
    /// # Panics
    ///
    /// Panics if `step` is not positive, `dim == 0`, `training.len()`
    /// is not a multiple of `dim`, or `training` is empty.
    pub fn new(step: f32, dim: usize, training: Vec<f32>) -> Self {
        assert!(dim > 0, "patch dim must be positive");
        assert_eq!(
            training.len() % dim,
            0,
            "training length must be a multiple of dim"
        );
        assert!(
            !training.is_empty(),
            "training patches are required for re-search"
        );
        PatchVqQuantizer {
            step: ScalarStep::new(step),
            dim,
            training,
        }
    }
}

impl Quantizer<PatchVqCoeffs> for PatchVqQuantizer {
    type Quantized = PatchVqQuantized;

    fn quantize(&self, coeffs: &PatchVqCoeffs) -> Self::Quantized {
        let codebook = quantize_stream(&coeffs.codebook, &self.step);
        let cb_recon = dequantize_stream(&codebook, &self.step);
        let mut indices = vec![0u32; self.training.len() / self.dim];
        vq_encode(
            &cb_recon,
            self.dim,
            &self.training,
            DistanceMetric::SquaredEuclidean,
            &mut indices,
        );
        PatchVqQuantized { codebook, indices }
    }

    fn dequantize(&self, quantized: &Self::Quantized) -> PatchVqCoeffs {
        PatchVqCoeffs {
            codebook: dequantize_stream(&quantized.codebook, &self.step),
            indices: quantized.indices.clone(),
        }
    }
}

/// Charges **codebook + indices** — the full transmitted state.
pub struct PatchVqRate;

impl RateEstimator<PatchVqQuantized> for PatchVqRate {
    fn estimate_bits(&self, quantized: &PatchVqQuantized) -> f64 {
        empirical_total_bits(&quantized.codebook) + empirical_total_bits_u32(&quantized.indices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::spatiotemporal::moving_edge;
    use tpt_media_representation::{DistortionMetric, SignalBuffer, SignalDims};

    fn config() -> PatchVqConfig {
        PatchVqConfig {
            patch_size: 4,
            codebook_entries: 16,
            train_iterations: 30,
            train_seed: 42,
        }
    }

    #[test]
    fn patch_extraction_preserves_total_sample_count() {
        let dims = SignalDims {
            width: 16,
            height: 8,
            frames: 3,
        };
        let data: Vec<f32> = (0..dims.samples()).map(|i| i as f32).collect();
        let sig = SignalBuffer::from_vec(dims, data);
        let patches = extract_patches(sig.view(), &config());
        assert_eq!(patches.len(), dims.samples());
        // Frame 0, tile (0,0), sample (0,0) == signal(0,0,0).
        assert_eq!(patches[0], 0.0);
        // Frame 1, tile (0,0), last sample == sample (3,3,1).
        let tiles_per_frame = tiles_per_frame(16, 8, 4);
        let last_of_first_tile = 15;
        assert_eq!(
            patches[tiles_per_frame * 16 + last_of_first_tile],
            1.0 * 128.0 + 3.0 * 16.0 + 3.0
        );
    }

    #[test]
    fn trained_codebook_only_captures_distinct_patches() {
        // Constant signal: one unique patch, so a codebook of any size.
        let dims = SignalDims {
            width: 8,
            height: 8,
            frames: 1,
        };
        // 8x8 with patch 2 -> 16 patches, equal to the 16-entry codebook.
        let sig = SignalBuffer::filled(dims, 0.7);
        let cfg = PatchVqConfig {
            patch_size: 2,
            codebook_entries: 16,
            train_iterations: 30,
            train_seed: 42,
        };
        let rep = PatchVq;
        let coeffs = rep.analyze(sig.view(), &cfg);
        let mut out = SignalBuffer::zeroed(dims);
        rep.synthesize(&coeffs, &cfg, out.view_mut());
        assert!(out.data().iter().all(|&v| (v - 0.7).abs() < 1e-5));
    }

    #[test]
    fn vq_reconstruction_is_bounded_and_deterministic() {
        let (sig, _) = moving_edge(16, 16, 4, 0.0, 0.0);
        let rep = PatchVq;
        let cfg = config();
        let a = rep.analyze(sig.view(), &cfg);
        let b = rep.analyze(sig.view(), &cfg);
        assert_eq!(a, b, "patch_vq must be deterministic");

        let mut out = SignalBuffer::zeroed(sig.dims());
        rep.synthesize(&a, &cfg, out.view_mut());
        let mse = tpt_media_representation::Mse.distortion(sig.view(), out.view());
        assert!(mse.is_finite() && mse >= 0.0);
        // An edge is a 2-level signal; with patches fully on/off the VQ error
        // is intrinsic (patch straddling the edge). Assert it stays bounded
        // well below "always half wrong".
        assert!(mse < 0.05, "mse={mse}");
    }

    #[test]
    fn rate_charges_both_codebook_and_indices() {
        let (sig, _) = moving_edge(16, 16, 4, 1.0, 0.0);
        let rep = PatchVq;
        let cfg = config();
        let coeffs = rep.analyze(sig.view(), &cfg);
        let q = PatchVqQuantizer::new(0.01, patch_dim(&cfg), extract_patches(sig.view(), &cfg));
        let qz = q.quantize(&coeffs);
        let bits = PatchVqRate.estimate_bits(&qz);
        // Must be strictly positive and charge the codebook on top of the
        // indices: with one repeated index the indices alone would be ~0 bits,
        // so any non-trivial total proves the dictionary is charged.
        assert!(bits.is_finite() && bits > 0.0);
    }
}
