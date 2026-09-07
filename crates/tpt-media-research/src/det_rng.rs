//! A tiny deterministic pseudo-random generator (SplitMix64), used only to
//! seed synthetic texture signals and k-means/Lloyd codebook initialization.
//!
//! Not cryptographic, not a dependency (`rand` is deliberately not pulled
//! in for this) — just a fixed, reproducible source of "random-looking"
//! numbers so results are exactly repeatable across runs, matching this
//! project's determinism principle.

pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// A deterministic value in `[0, 1)`.
    pub(crate) fn next_unit_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// A deterministic index in `[0, bound)`.
    ///
    /// # Panics
    ///
    /// Panics if `bound == 0`.
    pub(crate) fn next_index(&mut self, bound: usize) -> usize {
        assert!(bound > 0, "bound must be positive");
        (self.next_u64() as usize) % bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_reproduces_same_sequence() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = SplitMix64::new(1);
        let mut b = SplitMix64::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn unit_f32_stays_in_range() {
        let mut rng = SplitMix64::new(7);
        for _ in 0..1000 {
            let v = rng.next_unit_f32();
            assert!((0.0..1.0).contains(&v), "v={v}");
        }
    }
}
