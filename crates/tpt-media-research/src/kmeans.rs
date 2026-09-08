//! Deterministic k-means / Lloyd codebook training for `patch_vq`.
//!
//! `tpt-dsp-core` deliberately ships VQ *search* only (a codec owns its own
//! codebook contents — see `tpt-dsp-core/src/vq.rs`), so the research crate
//! that needs per-signal codebooks implements training itself. This is a
//! straightforward seeded Lloyd's algorithm:
//!
//! 1. **Initialisation**: `entries` distinct training vectors chosen by a
//!    seeded [`crate::det_rng::SplitMix64`]. Fully reproducible.
//! 2. **Assignment**: each vector → its nearest centroid under squared
//!    Euclidean distance (ties toward the lowest centroid index, matching
//!    `tpt-dsp-core::vq::nearest_vector`'s determinism).
//! 3. **Update**: each centroid → the mean of its assigned vectors.
//! 4. **Empty clusters**: a centroid with no members is re-seated on the
//!    training vector farthest from its own assigned centroid (ties broken
//!    toward the lowest vector index) — the standard Lloyd heuristic, made
//!    deterministic.
//! 5. Stop when an assignment iteration changes nothing, or after the cap.
//!
//! Results are exact-run-for-exact-run identical (no floating-point
//! accumulation order changes between runs on the same machine).

use crate::det_rng::SplitMix64;

/// Train a `entries × dim` flat codebook on `vectors` (a flat stream of
/// `dim`-vectors), returning `(codebook, assignments)`.
///
/// # Panics
///
/// Panics if `dim == 0`, `vectors.len()` is not a multiple of `dim`,
/// `entries == 0`, `iterations == 0`, or `entries > vectors.len()/dim`.
pub fn kmeans_train(
    vectors: &[f32],
    dim: usize,
    entries: usize,
    iterations: usize,
    seed: u64,
) -> (Vec<f32>, Vec<usize>) {
    assert!(dim > 0, "codebook dimension must be positive");
    assert_eq!(
        vectors.len() % dim,
        0,
        "vectors length must be a multiple of dim"
    );
    let n = vectors.len() / dim;
    assert!(n > 0, "at least one training vector required");
    assert!(entries > 0 && entries <= n, "entries must be in 1..=n");
    assert!(iterations > 0, "iterations must be positive");

    // ---- Initialisation: seeded distinct-vector pick ----
    let mut rng = SplitMix64::new(seed);
    let mut means: Vec<f32> = Vec::with_capacity(entries * dim);
    let mut chosen: Vec<usize> = Vec::with_capacity(entries);
    while chosen.len() < entries {
        let idx = (rng.next_u64() % n as u64) as usize;
        if !chosen.contains(&idx) {
            chosen.push(idx);
            means.extend_from_slice(&vectors[idx * dim..(idx + 1) * dim]);
        }
    }
    debug_assert_eq!(means.len(), entries * dim);

    // ---- Lloyd iterations ----
    let mut assign = vec![0usize; n];
    // `prev_assign` starts impossible (`usize::MAX`), so the first assignment
    // iteration always runs (a zeroed initial value would falsely *equal*
    // `assign` and skip the very first update).
    let mut prev_assign = vec![usize::MAX; n];
    let mut acc = vec![0f32; entries * dim];
    let mut count = vec![0u64; entries];

    for _ in 0..iterations {
        // Assignment.
        for v in 0..n {
            let vec = &vectors[v * dim..(v + 1) * dim];
            let mut best = 0usize;
            let mut best_d = f32::INFINITY;
            for (c, mean_slot) in means.chunks_exact(dim).enumerate() {
                let mut d = 0.0f32;
                for (a, b) in vec.iter().zip(mean_slot.iter()) {
                    let e = a - b;
                    d += e * e;
                }
                if d < best_d {
                    best_d = d;
                    best = c;
                }
            }
            assign[v] = best;
        }

        if assign == prev_assign {
            break;
        }
        prev_assign.copy_from_slice(&assign);

        // Update: sums + counts.
        acc.iter_mut().for_each(|a| *a = 0.0);
        count.iter_mut().for_each(|c| *c = 0);
        for v in 0..n {
            let c = assign[v];
            for k in 0..dim {
                acc[c * dim + k] += vectors[v * dim + k];
            }
            count[c] += 1;
        }

        // Reseat empty clusters deterministically: the vector farthest from
        // its *own* centroid (ties toward lowest vector index).
        for c in 0..entries {
            if count[c] > 0 {
                continue;
            }
            let mut candidate = 0usize;
            let mut best_dist = -1.0f32;
            for v in 0..n {
                let vec = &vectors[v * dim..(v + 1) * dim];
                let own = &means[assign[v] * dim..(assign[v] + 1) * dim];
                let mut d = 0.0f32;
                for (a, b) in vec.iter().zip(own.iter()) {
                    let e = a - b;
                    d += e * e;
                }
                if d > best_dist {
                    // Strict `>` on the first pass and on strictly-larger d:
                    // deterministic (first wins ties).
                    best_dist = d;
                    candidate = v;
                }
            }
            means[c * dim..(c + 1) * dim]
                .copy_from_slice(&vectors[candidate * dim..(candidate + 1) * dim]);
            count[c] = 1;
            // Note: `candidate` stays in its current cluster too. This is
            // Lloyd's standard empty-cluster handling: the split centre will
            // attract its own vectors on the next assignment iteration.
        }

        // Recompute means from sums.
        for c in 0..entries {
            let inv = 1.0 / count[c] as f32;
            for k in 0..dim {
                means[c * dim + k] = acc[c * dim + k] * inv;
            }
        }
    }

    (means, assign)
}

/// Mean squared error of quantising `vectors` with `codebook`
/// (`entries = codebook.len()/dim`), under nearest-codeword search.
pub fn codebook_mse(vectors: &[f32], dim: usize, codebook: &[f32]) -> f64 {
    let n = vectors.len() / dim;
    let entries = codebook.len() / dim;
    assert!(entries > 0, "codebook must be non-empty");
    let mut total = 0.0f64;
    for v in 0..n {
        let vec = &vectors[v * dim..(v + 1) * dim];
        let mut best = f64::INFINITY;
        for mean_slot in codebook.chunks_exact(dim) {
            let mut d = 0.0f64;
            for (a, b) in vec.iter().zip(mean_slot.iter()) {
                let e = (*a - *b) as f64;
                d += e * e;
            }
            if d < best {
                best = d;
            }
        }
        total += best;
    }
    total / dim as f64 / n as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4 tightly-clustered gaussians in 2-D, generated deterministically
    /// (no `rand`): clusters around `(0,0)`, `(5,0)`, `(0,5)`, `(5,5)`.
    fn clustered_vectors() -> Vec<f32> {
        let mut rng = SplitMix64::new(99);
        let mut v = Vec::new();
        for &center in &[(0.0f32, 0.0f32), (5.0, 0.0), (0.0, 5.0), (5.0, 5.0)] {
            for _ in 0..25 {
                let gx = (rng.next_unit_f32() - 0.5) * 0.4;
                let gy = (rng.next_unit_f32() - 0.5) * 0.4;
                v.push(center.0 + gx);
                v.push(center.1 + gy);
            }
        }
        v
    }

    #[test]
    fn kmeans_recovers_four_clusters() {
        let v = clustered_vectors();
        let (cb, _assign) = kmeans_train(&v, 2, 4, 50, 7);
        let mut found: Vec<[f32; 2]> = vec![];
        for e in cb.chunks_exact(2) {
            let mut best = f32::INFINITY;
            let mut point = [0.0f32; 2];
            for &c in &[(0.0f32, 0.0f32), (5.0, 0.0), (0.0, 5.0), (5.0, 5.0)] {
                let d = (e[0] - c.0).powi(2) + (e[1] - c.1).powi(2);
                if d < best {
                    best = d;
                    point = [c.0, c.1];
                }
            }
            found.push(point);
        }
        found.sort_by(|a, b| {
            a[0].partial_cmp(&b[0])
                .unwrap()
                .then(a[1].partial_cmp(&b[1]).unwrap())
        });
        let expected = [[0.0, 0.0], [0.0, 5.0], [5.0, 0.0], [5.0, 5.0]];
        assert_eq!(found.len(), expected.len());
        for (f, e) in found.iter().zip(expected.iter()) {
            assert!(
                (f[0] - e[0]).abs() < 0.2 && (f[1] - e[1]).abs() < 0.2,
                "found={found:?}"
            );
        }
    }

    #[test]
    fn kmeans_is_reproducible_given_seed() {
        let v = clustered_vectors();
        let (cb1, _) = kmeans_train(&v, 2, 4, 50, 3);
        let (cb2, _) = kmeans_train(&v, 2, 4, 50, 3);
        assert_eq!(cb1, cb2);
    }

    #[test]
    fn larger_codebook_reduces_distortion() {
        let v = clustered_vectors();
        let (small, _) = kmeans_train(&v, 2, 2, 30, 11);
        let (large, _) = kmeans_train(&v, 2, 8, 30, 11);
        assert!(codebook_mse(&v, 2, &large) < codebook_mse(&v, 2, &small));
    }

    #[test]
    fn single_entry_codebook_is_the_global_mean() {
        let v = clustered_vectors();
        // With one codeword, any initialisation lands on the mean on the
        // first update and stays (assignment can't change).
        let (cb, _) = kmeans_train(&v, 2, 1, 5, 0);
        let mean0 = v.chunks_exact(2).map(|c| c[0] as f64).sum::<f64>() / 100.0;
        let mean1 = v.chunks_exact(2).map(|c| c[1] as f64).sum::<f64>() / 100.0;
        assert!((cb[0] as f64 - mean0).abs() < 1e-4);
        assert!((cb[1] as f64 - mean1).abs() < 1e-4);
    }
}
