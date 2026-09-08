//! Concrete representations for the representation-research platform
//! (`todo.md` §56).
//!
//! Each representation implements the abstract
//! `tpt-media-representation::Representation` contract with its own
//! `Config`/`Coeffs`/`Quantizer`/`RateEstimator`:
//!
//! * [`baseline`] — raw identity, the RD anchor.
//! * [`block_dct`] — Avenue A: separable 2D block DCT-II.
//! * [`temporal_dct`] — previous-frame prediction + block DCT on the
//!   residual (a *third* oracle experiment condition, not one of the three
//!   core representations).
//! * [`patch_vq`] — Avenues C/D: per-signal trained codebook + VQ indices.
//! * [`motion_aligned`] — Avenue F: coordinate warp by velocity before the
//!   block-DCT.
//!
//! The structural "no free decoder-side information" rule lives in the
//! `Coeffs` shapes themselves: any dictionary, motion, or mode state that the
//! decoder needs is *in* `Coeffs` and *charged* by the corresponding
//! `RateEstimator` (see [`super::uniform::EntropyRate`],
//! [`patch_vq::PatchVqRate`], [`temporal_dct::TemporalDctRate`],
//! [`motion_aligned::MotionAlignedRate`]).
//!
//! `tpt-dsp` is not modified for any of this: the 2D DCT is a separable row+
//! column pass over `tpt-dsp-core`'s 1D `dct_ii`/`dct_iii` (see
//! [`crate::representations::blockdct`]), and VQ uses `tpt-dsp-core`'s
//! search/encode/decode with the codebook trained here.

pub mod baseline;
pub mod block_dct;
mod blockdct;
pub mod motion_aligned;
pub mod patch_vq;
pub mod temporal_dct;

pub use baseline::{Baseline, BaselineCoeffs, BaselineConfig};
pub use block_dct::{BlockDct, BlockDctCoeffs, BlockDctConfig};
pub use motion_aligned::{MotionAligned, MotionAlignedCoeffs, MotionAlignedConfig};
pub use patch_vq::{PatchVq, PatchVqCoeffs, PatchVqConfig};
pub use temporal_dct::{TemporalDct, TemporalDctCoeffs, TemporalDctConfig};
