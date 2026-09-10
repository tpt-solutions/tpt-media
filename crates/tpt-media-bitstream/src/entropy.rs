//! Entropy utilities — Shannon entropy and probability-distribution helpers.
//!
//! These are the same concepts an arithmetic/range coder measures against
//! a signal: the Shannon entropy (in bits per symbol) of a symbol
//! distribution, and helpers to normalize raw counts into probabilities
//! and to derive the empirical distribution from a sample sequence.
//!
//! They live here (in `tpt-media-bitstream`) rather than in `tpt-dsp`
//! because they are intrinsically a property of the *bitstream* layer:
//! Huffman, Rice, and range coders all converge to this number, and
//! keeping the math in the same crate as the coders avoids a circular
//! dependency from `tpt-dsp` back to bitstream primitives.

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

/// Natural log of 2, pre-computed for the bits-vs-nats consistency test.
#[cfg(test)]
const LN2: f64 = core::f64::consts::LN_2;

/// Compute the Shannon entropy of a discrete distribution, in **bits
/// per symbol**.
///
/// `counts[i]` is the number of times symbol `i` was observed; the
/// distribution is normalised internally, so absolute counts (not
/// probabilities) are accepted. Returns:
///
/// - `0.0` if the input has zero length or contains only zeros (no
///   information), matching the convention that a constant stream
///   carries no entropy per symbol;
/// - `log2(n)` if all `n` non-zero counts are equal (the uniform
///   distribution, which is the maximum-entropy distribution over an
///   `n`-symbol alphabet — useful as a sanity check).
///
/// Symbols with count `0` contribute exactly `0` to the sum, so they
/// need not be trimmed before calling.
pub fn shannon_entropy_bits(counts: &[u64]) -> f64 {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let total_f = total as f64;
    let mut h = 0.0_f64;
    for &c in counts {
        if c == 0 {
            continue;
        }
        let p = c as f64 / total_f;
        // p > 0 is guaranteed (we skipped c == 0), so log2 is defined.
        h -= p * log2(p);
    }
    h
}

/// Same as [`shannon_entropy_bits`], but returns the entropy in **nats**
/// (natural logarithm) instead of bits. `1 nat = 1/ln(2) bits`.
pub fn shannon_entropy_nats(counts: &[u64]) -> f64 {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let total_f = total as f64;
    let mut h = 0.0_f64;
    for &c in counts {
        if c == 0 {
            continue;
        }
        let p = c as f64 / total_f;
        h -= p * libm::log(p);
    }
    h
}

/// Convert a slice of raw counts into a normalised probability
/// distribution.
///
/// The output slice has the same length as the input. If the input sums
/// to `0`, every entry is `0`. Otherwise each entry is the count
/// divided by the total as `f64`.
///
/// This is the helper most entropy-driven code paths need first —
/// computing the histogram of observed symbols, then turning it into a
/// probability distribution that the entropy (and the Huffman / Rice /
/// range coder) consume.
pub fn normalize(counts: &[u64]) -> Vec<f64> {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return vec![0.0_f64; counts.len()];
    }
    let total_f = total as f64;
    counts.iter().map(|&c| c as f64 / total_f).collect()
}

/// Histogram of `values` over the symbol range `0..num_symbols`.
///
/// Counts are clipped to `num_symbols` (out-of-range values are ignored)
/// and the output slice has length `num_symbols`. This is the canonical
/// way to turn a sample sequence into a probability distribution to
/// feed into [`shannon_entropy_bits`] / [`normalize`].
pub fn histogram<T>(values: &[T], num_symbols: usize) -> Vec<u64>
where
    T: Into<u64> + Copy,
{
    let mut hist = vec![0u64; num_symbols];
    for &v in values {
        let s: u64 = v.into();
        if (s as usize) < num_symbols {
            hist[s as usize] += 1;
        }
    }
    hist
}

/// log2 helper — `f64::log2`/`f64::ln` are not available in `core`, so
/// this crate (no_std-capable) routes the math through `libm`.
#[inline]
fn log2(x: f64) -> f64 {
    libm::log2(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_counts_have_zero_entropy() {
        assert_eq!(shannon_entropy_bits(&[]), 0.0);
        assert_eq!(shannon_entropy_bits(&[0, 0, 0]), 0.0);
    }

    #[test]
    fn uniform_distribution_has_log2_n_entropy() {
        // 4 equal symbols => 2.0 bits per symbol.
        assert!((shannon_entropy_bits(&[1, 1, 1, 1]) - 2.0).abs() < 1e-12);
        // 8 equal symbols => 3.0 bits per symbol.
        assert!((shannon_entropy_bits(&[1; 8]) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn constant_stream_has_zero_entropy() {
        assert_eq!(shannon_entropy_bits(&[0, 1000, 0]), 0.0);
    }

    #[test]
    fn skewed_distribution_has_lower_entropy_than_uniform() {
        let h_uniform = shannon_entropy_bits(&[10, 10, 10, 10]);
        let h_skewed = shannon_entropy_bits(&[1000, 1, 1, 1]);
        assert!(h_skewed < h_uniform);
    }

    #[test]
    fn bits_and_nats_agree() {
        let counts = [3u64, 5, 2];
        let h_bits = shannon_entropy_bits(&counts);
        let h_nats = shannon_entropy_nats(&counts);
        // h_bits = h_nats / ln(2), so h_bits * ln(2) == h_nats.
        assert!((h_bits * LN2 - h_nats).abs() < 1e-12);
    }

    #[test]
    fn normalize_sums_to_one() {
        let counts = [2u64, 3, 5];
        let p = normalize(&counts);
        assert_eq!(p.len(), 3);
        let sum: f64 = p.iter().sum();
        assert!((sum - 1.0).abs() < 1e-12);
        assert!((p[0] - 0.2).abs() < 1e-12);
        assert!((p[1] - 0.3).abs() < 1e-12);
        assert!((p[2] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn normalize_zeros_for_empty_input() {
        let p = normalize(&[]);
        assert!(p.is_empty());
        let p = normalize(&[0u64, 0, 0]);
        assert_eq!(p, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn histogram_counts_in_range() {
        let values: Vec<u8> = vec![0, 0, 1, 2, 2, 2, 9 /* ignored */];
        let h = histogram(&values, 3);
        assert_eq!(h, vec![2, 1, 3]);
    }

    #[test]
    fn round_trip_histogram_to_entropy() {
        // Sample sequence with 4 equally-frequent symbols => entropy = log2(4) = 2 bits.
        let values: Vec<u8> = (0..4).flat_map(|i| vec![i; 7]).collect();
        let h = histogram(&values, 4);
        let entropy = shannon_entropy_bits(&h);
        assert!((entropy - 2.0).abs() < 1e-12);
    }
}
