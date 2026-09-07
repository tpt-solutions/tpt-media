//! Quantisation contract: coefficients/parameters to a transmissible form.

/// Quantises `C` (a [`crate::Representation`]'s `Coeffs`) to a transmissible
/// `Quantized` form and back.
///
/// `Quantized` must, like `Coeffs`, carry everything needed to reconstruct
/// `C` — `dequantize` takes no other input.
pub trait Quantizer<C> {
    type Quantized;

    fn quantize(&self, coeffs: &C) -> Self::Quantized;
    fn dequantize(&self, quantized: &Self::Quantized) -> C;
}
