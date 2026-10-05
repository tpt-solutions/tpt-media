//! Quantisation contract: coefficients/parameters to a transmissible form.

/// Quantises `C` (a [`crate::Representation`]'s `Coeffs`) to a transmissible
/// `Quantized` form and back.
///
/// `Quantized` must, like `Coeffs`, carry everything needed to reconstruct
/// `C` — `dequantize` takes no other input.
pub trait Quantizer<C> {
    /// The quantized representation of one coefficient block.
    type Quantized;

    /// Map a coefficient block to its quantized form.
    fn quantize(&self, coeffs: &C) -> Self::Quantized;
    /// Reconstruct a (lossy) coefficient block from its quantized
    /// form; takes no other input, so `Quantized` is
    /// self-contained.
    fn dequantize(&self, quantized: &Self::Quantized) -> C;
}
