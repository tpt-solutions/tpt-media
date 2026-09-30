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

use std::collections::BTreeMap;

/// Empirical (zeroth-order) Shannon entropy of `symbols`, in bits/symbol:
/// `H = -Σ p(s)·log2 p(s)` over the observed frequency distribution.
///
/// Returns `0.0` for an empty slice or a slice of one repeated symbol.
///
/// Counts are accumulated in a [`BTreeMap`] (sorted key order), so the
/// floating-point summation order is identical run to run — this is part of
/// the crate's byte-identical determinism contract.
pub fn empirical_entropy_bits(symbols: &[i32]) -> f64 {
    if symbols.is_empty() {
        return 0.0;
    }
    let mut counts: BTreeMap<i32, u64> = BTreeMap::new();
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
    let mut counts: BTreeMap<u32, u64> = BTreeMap::new();
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

    /// §38: empty input. Every entry point answers 0 rather than `0/0` — the
    /// crate's finiteness contract depends on it, and a rate of `NaN` would
    /// silently order as "less than" any real rate in an RD comparison.
    #[test]
    fn an_empty_stream_costs_nothing() {
        assert_eq!(empirical_entropy_bits(&[]), 0.0);
        assert_eq!(empirical_total_bits(&[]), 0.0);
        assert_eq!(empirical_entropy_bits_u32(&[]), 0.0);
        assert_eq!(empirical_total_bits_u32(&[]), 0.0);
    }

    /// §38: maximum values. Symbol *values* are irrelevant to entropy — only
    /// their frequency matters — so the extremes of both integer types must
    /// behave like any other symbol rather than overflowing a count.
    #[test]
    fn extreme_symbol_values_are_just_symbols() {
        let extremes = [i32::MIN, i32::MAX, -1, 0];
        assert_eq!(empirical_entropy_bits(&extremes), 2.0, "4 distinct");
        assert_eq!(empirical_total_bits(&extremes), 8.0);
        let uextremes = [u32::MIN, u32::MAX];
        assert_eq!(empirical_entropy_bits_u32(&uextremes), 1.0);
        assert_eq!(empirical_total_bits_u32(&uextremes), 2.0);
    }

    /// Entropy of an `n`-symbol uniform distribution is exactly `log2 n`, and
    /// a distribution over `k` distinct symbols can never exceed `log2 k`.
    /// The second bound is the one a measurement can silently violate if the
    /// accumulation drifts, so it is checked over a range of `k`.
    #[test]
    fn entropy_never_exceeds_the_bound_set_by_the_symbol_count() {
        for k in 2..=12usize {
            // `k` distinct symbols, each appearing `k` times.
            let symbols: Vec<i32> = (0..k as i32)
                .flat_map(|s| std::iter::repeat_n(s, k))
                .collect();
            let h = empirical_entropy_bits(&symbols);
            assert!(h <= (k as f64).log2() + 1e-9, "k={k} h={h}");
            assert!(h >= 0.0, "k={k} h={h}");
            assert!(h.is_finite(), "k={k} h={h}");
        }
    }

    /// The counts are accumulated in a `BTreeMap` specifically so the
    /// floating-point summation order is identical run to run. That is the
    /// crate's byte-identical determinism contract, so it is asserted rather
    /// than assumed.
    #[test]
    fn entropy_is_bit_identical_across_repeated_calls() {
        let symbols: Vec<i32> = (0..1000).map(|i| (i * 7919 % 13) - 6).collect();
        let first = empirical_entropy_bits(&symbols);
        for _ in 0..8 {
            assert_eq!(empirical_entropy_bits(&symbols), first);
        }
        // The same multiset in a different input order must give the same
        // answer too — that is what the sorted accumulation buys.
        let mut shuffled = symbols.clone();
        shuffled.reverse();
        assert_eq!(empirical_entropy_bits(&shuffled), first);
    }
}
