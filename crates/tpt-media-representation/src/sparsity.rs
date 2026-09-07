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
}
