//! Sparsity/energy-compaction measurement.
//!
//! **Secondary/diagnostic only.** These numbers help explain *why* an RD
//! curve looks the way it does; they are never the primary result. A
//! representation with a nicer sparsity/retention profile but a worse RD
//! frontier is not a win — see `docs/representation-research.md` and
//! Avenue P ("entropy is not the same as sparsity").

/// Implemented by a [`crate::Representation`]'s `Coeffs` type so
/// cross-cutting sparsity/retention tooling can inspect and prune it
/// generically, without needing to know its concrete shape.
pub trait CoeffMagnitudes {
    /// The magnitude of every coefficient that measurement/retention should
    /// consider (e.g. a representation's residual stream — not any embedded
    /// side information like a dictionary or motion parameters, which are
    /// not "coefficients" in this sense).
    fn coefficient_magnitudes(&self) -> Vec<f32>;

    /// Zero every coefficient whose magnitude is `< threshold`, returning
    /// the number retained. Used only by the diagnostic coefficient-retention
    /// sweep — never by the primary rate/distortion sweep.
    fn retain_above_threshold(&mut self, threshold: f32) -> usize;
}

/// Sparsity/energy-compaction summary of a coefficient magnitude stream.
pub struct SparsityReport {
    pub total: usize,
    pub nonzero: usize,
    /// Cumulative fraction of total energy captured by the top `k`
    /// coefficients (by magnitude), for `k` in `1..=total`.
    pub energy_compaction: Vec<f32>,
}

impl SparsityReport {
    /// `epsilon` is the magnitude below which a coefficient counts as zero.
    pub fn compute(magnitudes: &[f32], epsilon: f32) -> Self {
        let total = magnitudes.len();
        let nonzero = magnitudes.iter().filter(|m| m.abs() > epsilon).count();

        let mut energy: Vec<f32> = magnitudes.iter().map(|m| m * m).collect();
        energy.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let total_energy: f32 = energy.iter().sum();

        let mut cumulative = 0.0f32;
        let energy_compaction = if total_energy > 0.0 {
            energy
                .iter()
                .map(|e| {
                    cumulative += e;
                    cumulative / total_energy
                })
                .collect()
        } else {
            vec![0.0; total]
        };

        SparsityReport {
            total,
            nonzero,
            energy_compaction,
        }
    }

    /// Fewest top coefficients (by magnitude) needed to capture at least
    /// `target_fraction` of total energy. Returns `total` if the target
    /// fraction is never reached (e.g. zero total energy).
    pub fn coefficients_for_energy_fraction(&self, target_fraction: f32) -> usize {
        self.energy_compaction
            .iter()
            .position(|&e| e >= target_fraction)
            .map(|i| i + 1)
            .unwrap_or(self.total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_dominant_coefficient_is_maximally_compact() {
        let mags = [10.0f32, 0.0, 0.0, 0.0];
        let report = SparsityReport::compute(&mags, 1e-6);
        assert_eq!(report.nonzero, 1);
        assert_eq!(report.coefficients_for_energy_fraction(0.99), 1);
    }

    #[test]
    fn equal_magnitude_coefficients_need_all_for_full_energy() {
        let mags = [1.0f32, 1.0, 1.0, 1.0];
        let report = SparsityReport::compute(&mags, 1e-6);
        assert_eq!(report.nonzero, 4);
        assert_eq!(report.coefficients_for_energy_fraction(1.0), 4);
        assert!((report.energy_compaction[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn all_zero_coefficients_report_zero_nonzero_and_full_total() {
        let mags = [0.0f32, 0.0, 0.0];
        let report = SparsityReport::compute(&mags, 1e-6);
        assert_eq!(report.nonzero, 0);
        assert_eq!(report.total, 3);
        assert_eq!(report.coefficients_for_energy_fraction(0.5), 3);
    }

    /// §38: empty input. The report is empty rather than panicking, and the
    /// "never reached" fallback is `total`, which is 0 here — so an empty
    /// stream asks for 0 coefficients, not 1.
    #[test]
    fn an_empty_magnitude_stream_reports_nothing_and_asks_for_no_coefficients() {
        let report = SparsityReport::compute(&[], 1e-6);
        assert_eq!(report.total, 0);
        assert_eq!(report.nonzero, 0);
        assert!(report.energy_compaction.is_empty());
        assert_eq!(report.coefficients_for_energy_fraction(0.5), 0);
    }

    /// §38: edge cases. A target fraction outside `[0, 1]` is a caller error
    /// but the function is public and must not panic: 0 is satisfiable by the
    /// first coefficient, and anything above 1 is unreachable and falls back
    /// to `total`.
    #[test]
    fn a_target_fraction_outside_the_unit_interval_is_clamped_by_the_fallback() {
        let report = SparsityReport::compute(&[1.0, 2.0, 3.0, 4.0], 1e-6);
        // Any fraction at or below the first coefficient's share needs one.
        assert_eq!(report.coefficients_for_energy_fraction(0.0), 1);
        assert_eq!(report.coefficients_for_energy_fraction(0.5), 1);
        // Above 1.0 is never reached, so the documented `total` fallback.
        assert_eq!(report.coefficients_for_energy_fraction(1.5), 4);
    }

    #[test]
    fn energy_compaction_is_a_cumulative_fraction_sorted_by_magnitude() {
        // [1,2,3,4] -> energy [1,4,9,16] sorted desc -> [16,9,4,1]/30.
        let report = SparsityReport::compute(&[1.0, 2.0, 3.0, 4.0], 1e-6);
        assert_eq!(report.energy_compaction.len(), 4);
        for (i, got) in report.energy_compaction.iter().enumerate() {
            let want = [16.0f32, 25.0, 29.0, 30.0][i] / 30.0;
            assert!((got - want).abs() < 1e-6, "k={i}: got {got}, want {want}");
        }
        // Monotonically non-decreasing and terminating at exactly 1.
        assert!(report.energy_compaction.windows(2).all(|w| w[1] >= w[0]));
        assert!((report.energy_compaction[3] - 1.0).abs() < 1e-6);
    }

    /// The epsilon boundary is exclusive (`> epsilon`), so a magnitude exactly
    /// equal to it counts as zero. Pinned because the retention sweep's
    /// results shift if that ever becomes inclusive.
    #[test]
    fn the_nonzero_test_is_strictly_greater_than_epsilon() {
        // Only the last entry clears the threshold; the first sits exactly on
        // it (excluded) and the second is below it.
        let report = SparsityReport::compute(&[1e-6, 1.0e-7, 1.5e-6], 1e-6);
        assert_eq!(report.nonzero, 1, "only 1.5e-6 is strictly above 1e-6");
    }

    #[test]
    fn a_negative_epsilon_counts_everything_including_exact_zeros() {
        // `abs() > -1.0` holds even for 0.0, so every coefficient is
        // "nonzero". Degenerate input, but it must not panic or mis-count.
        let report = SparsityReport::compute(&[0.0, 1.0, -2.0], -1.0);
        assert_eq!(report.nonzero, 3);
    }

    /// `retain_above_threshold` is a trait method with no in-crate
    /// implementor, so its contract ("zero every coefficient whose magnitude
    /// is < threshold, returning the number retained") is only observable
    /// through an implementation. Exercised here with the simplest possible
    /// one, which is also the only way to test the trait from inside the
    /// crate.
    #[test]
    fn retaining_above_a_threshold_zeroes_the_rest_and_counts_the_survivors() {
        #[derive(Default)]
        struct Coeffs(Vec<f32>);

        impl CoeffMagnitudes for Coeffs {
            fn coefficient_magnitudes(&self) -> Vec<f32> {
                self.0.iter().map(|c| c.abs()).collect()
            }

            fn retain_above_threshold(&mut self, threshold: f32) -> usize {
                let mut kept = 0;
                for c in self.0.iter_mut() {
                    if c.abs() >= threshold {
                        kept += 1;
                    } else {
                        *c = 0.0;
                    }
                }
                kept
            }
        }

        let mut c = Coeffs(vec![5.0, -0.1, 2.0, 0.0, 9.0]);
        assert_eq!(c.retain_above_threshold(1.0), 3);
        assert_eq!(c.0, vec![5.0, 0.0, 2.0, 0.0, 9.0]);
        // A threshold nothing clears empties the stream rather than erroring.
        assert_eq!(c.retain_above_threshold(100.0), 0);
        assert_eq!(c.0, vec![0.0; 5]);
        assert_eq!(c.coefficient_magnitudes(), vec![0.0; 5]);
    }
}
