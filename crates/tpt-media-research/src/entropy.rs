//! Empirical/Shannon entropy estimation over quantized symbol streams.
//!
//! Complements, but is distinct from, the *sparsity* diagnostics in
//! `tpt_media_representation::sparsity`: a representation with many
//! non-zero coefficients can still have low entropy (and so compress well)
//! if those coefficients are highly predictable — see Avenue P in
//! `docs/representation-research.md`. This is analytic bit *estimation*,
//! not a real entropy coder; wiring the actual `tpt-media-bitstream` coder
//! so the rate axis is measured rather than estimated is a natural
//! follow-on, not required to answer the RD/sparsity questions this
//! milestone asks.

use std::collections::HashMap;

/// Empirical (zeroth-order) Shannon entropy of `symbols`, in bits/symbol:
/// `H = -Σ p(s)·log2 p(s)` over the observed frequency distribution.
///
/// Returns `0.0` for an empty slice or a slice of one repeated symbol.
pub fn empirical_entropy_bits(symbols: &[i32]) -> f64 {
    if symbols.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<i32, u64> = HashMap::new();
    for &s in symbols {
        *counts.entry(s).or_insert(0) += 1;
    }
    let n = symbols.len() as f64;
    counts.values().fold(0.0, |acc, &c| {
        let p = c as f64 / n;
        acc - p * p.log2()
    })
}

/// Same as [`empirical_entropy_bits`] but over `u32` symbols (the natural
/// type for VQ codebook indices).
pub fn empirical_entropy_bits_u32(symbols: &[u32]) -> f64 {
    if symbols.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<u32, u64> = HashMap::new();
    for &s in symbols {
        *counts.entry(s).or_insert(0) += 1;
    }
    let n = symbols.len() as f64;
    counts.values().fold(0.0, |acc, &c| {
        let p = c as f64 / n;
        acc - p * p.log2()
    })
}

/// Total estimated bits to transmit `symbols` under their own empirical
/// distribution: `symbols.len() * empirical_entropy_bits(symbols)`.
pub fn empirical_total_bits(symbols: &[i32]) -> f64 {
    symbols.len() as f64 * empirical_entropy_bits(symbols)
}

/// `u32` counterpart of [`empirical_total_bits`].
pub fn empirical_total_bits_u32(symbols: &[u32]) -> f64 {
    symbols.len() as f64 * empirical_entropy_bits_u32(symbols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_stream_has_zero_entropy() {
        let symbols = vec![5; 100];
        assert_eq!(empirical_entropy_bits(&symbols), 0.0);
        assert_eq!(empirical_total_bits(&symbols), 0.0);
    }

    #[test]
    fn uniform_distribution_over_n_symbols_has_log2_n_entropy() {
        // 4 equally likely symbols -> exactly 2 bits/symbol.
        let symbols = vec![0, 1, 2, 3, 0, 1, 2, 3];
        let h = empirical_entropy_bits(&symbols);
        assert!((h - 2.0).abs() < 1e-9, "h={h}");
    }

    #[test]
    fn empty_stream_has_zero_entropy() {
        assert_eq!(empirical_entropy_bits(&[]), 0.0);
    }

    #[test]
    fn skewed_distribution_has_lower_entropy_than_uniform() {
        let skewed = vec![0, 0, 0, 0, 0, 0, 0, 1];
        let uniform = vec![0, 0, 0, 0, 1, 1, 1, 1];
        assert!(empirical_entropy_bits(&skewed) < empirical_entropy_bits(&uniform));
    }
}
