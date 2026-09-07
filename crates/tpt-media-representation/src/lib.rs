//! `tpt-media-representation` — swappable media representation contracts.
//!
//! Formalises `z = T(x)` / `x_hat = T⁻¹(z)` minimising
//! `J = D(x, x_hat) + λ·R(z)`, where `T` is never assumed to be a DCT,
//! wavelet, block transform, or motion-compensated residual (see
//! `docs/representation-research.md` at the repo root, and `todo.md` §56).
//!
//! This crate is deliberately small: it defines the contracts
//! ([`Representation`], [`Quantizer`], [`RateEstimator`], [`DistortionMetric`])
//! and the spatiotemporal signal type ([`SignalView`]/[`SignalBuffer`]) that
//! let concrete representations, quantizers, and metrics be swapped and
//! compared without rewriting the comparison harness. Concrete
//! representations, synthetic signals, and the experiment harness itself
//! live in `tpt-media-research`.
//!
//! **Offline-profile only.** Unlike most of this workspace, nothing here is
//! held to the real-time/no_std contract (`todo.md` §14) — dynamic
//! allocation, larger scratch, and expensive search are all fine, since this
//! is research/analysis code, never a real-time encode/decode path.

mod distortion;
mod quantizer;
mod rate;
mod representation;
mod sparsity;
mod volume;

pub use distortion::{psnr_db, DistortionMetric, Mse, Rmse};
pub use quantizer::Quantizer;
pub use rate::RateEstimator;
pub use representation::Representation;
pub use sparsity::{CoeffMagnitudes, SparsityReport};
pub use volume::{SignalBuffer, SignalDims, SignalView, SignalViewMut};
