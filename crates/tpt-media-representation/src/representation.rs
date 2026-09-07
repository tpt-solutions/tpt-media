//! The swappable representation contract: `T` (analyze) and `T⁻¹` (synthesize).

use crate::volume::{SignalView, SignalViewMut};

/// A representation `T` of a `V(x,y,t)` signal: `analyze` computes
/// `z = T(x)`, `synthesize` computes `x_hat = T⁻¹(z)`.
///
/// `T` is never assumed to be a DCT, wavelet, block transform, or
/// motion-compensated residual — see `docs/representation-research.md`.
///
/// # Contract: `Coeffs` must be complete
///
/// `Coeffs` is the *entire* decoder-required state. Any dictionary, motion
/// parameter, mode choice, or other side information needed to reconstruct
/// the signal lives inside `Coeffs` — never assumed available to the
/// decoder "for free" just because the encoder happened to have it. This is
/// what lets a [`crate::RateEstimator`] over `Coeffs` (via its quantized
/// form) charge every bit the reconstruction actually costs.
///
/// `Config` instead carries only fixed, out-of-band-agreed parameters (e.g.
/// a block size both sides already agree on, analogous to a codec profile
/// constant) — never per-instance data that must be signalled per input.
pub trait Representation {
    type Config;
    type Coeffs;

    /// `z = T(x)`.
    fn analyze(&self, signal: SignalView<'_>, config: &Self::Config) -> Self::Coeffs;

    /// `x_hat = T⁻¹(z)`, written into `out`.
    fn synthesize(&self, coeffs: &Self::Coeffs, config: &Self::Config, out: SignalViewMut<'_>);
}
