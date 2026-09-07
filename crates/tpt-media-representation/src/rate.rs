//! Rate estimation contract: bits required to transmit a quantized
//! representation.

/// Estimates the number of bits required to transmit a quantized
/// representation.
///
/// # The "no free decoder-side information" rule
///
/// Implementations MUST sum bits over the *entire* `Quantized` value —
/// coefficients/residuals plus any embedded dictionary, motion, or mode side
/// information — never only part of it. Nothing may be treated as free just
/// because the encoder had it; the decoder needs every bit `Quantized`
/// carries in order to reconstruct, so every bit of it must be charged.
pub trait RateEstimator<Q> {
    /// Estimated bits to transmit `quantized`.
    fn estimate_bits(&self, quantized: &Q) -> f64;
}
