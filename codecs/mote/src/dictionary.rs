//! The Mote dictionary: a fixed-size codebook of 8x8 patches, and its
//! deterministic trainer.
//!
//! ## Representation
//!
//! A dictionary is `1..=`[`MAX_DICTIONARY_ENTRIES`] entries of
//! [`PATCH_DIM`] (= 64) unsigned 8-bit samples, stored flat
//! (`entries * 64` bytes). Entries are *bytes*, not floats: what the
//! decoder holds is exactly what the encoder's search ran against, so
//! there is no encoder-side "optimistic" codebook.
//!
//! An entry is addressed on the wire by an index of [`index_bits`] bits
//! (12 for the full 4096-entry dictionary - the todo.md 34 target).
//!
//! ## Search
//!
//! Nearest-entry search is `tpt_dsp_core::nearest_vector_accelerated`
//! under squared-Euclidean distance (todo.md 8: VQ lives in `tpt-dsp`,
//! this crate only owns codebook *contents*). Because every sample is an
//! integer in `0..=255`, every partial sum is an integer at most
//! `64 * 255^2 = 4 161 600 < 2^24`, so each `f32` distance is an *exact*
//! integer: the search is bit-identical on every target and immune to
//! float-summation-order questions (todo.md 39).
//!
//! ## Training (deterministic Lloyd)
//!
//! [`Dictionary::train_patches`] is a seeded, integer-centroid Lloyd
//! iteration:
//!
//! 1. *Subsample*: if there are more than `max_vectors` training patches,
//!    take an evenly strided subset (no randomness).
//! 2. *Initialise*: `entries` distinct training patches chosen by a
//!    partial Fisher-Yates shuffle driven by SplitMix64 from a fixed seed.
//! 3. *Assign*: each patch to its nearest centroid (ties toward the
//!    lowest index, the `tpt-dsp-core` rule).
//! 4. *Update*: each centroid becomes the **rounded integer mean** of its
//!    members (`(2*sum + n) / (2*n)`, pure integer arithmetic).
//! 5. *Empty clusters* are re-seated, in ascending centroid order, on the
//!    training patch farthest from its own centroid (ties toward the
//!    lowest patch index), which is then marked distance 0 so the next
//!    empty cluster picks a different one.
//! 6. Stop when an assignment pass changes nothing or after `iterations`.
//!
//! No `HashMap`, no clock, no thread state: same input, same seed, same
//! bytes. Rounding centroids to integers every iteration costs a little
//! optimality against textbook real-valued Lloyd, and buys exactness.

use alloc::vec;
use alloc::vec::Vec;

use tpt_dsp_core::{nearest_vector_accelerated, DistanceMetric};
use tpt_media_codec::{CodecError, CodecResult};
use tpt_media_core::{PixelFormat, VideoFrame};

use crate::picture::{is_coded_format, PatchGrid, Picture};
use crate::{crc32, MAX_DICTIONARY_ENTRIES, PATCH_DIM};

/// Bits needed to address `entries` dictionary entries: `ceil(log2)`,
/// with a floor of one bit so an index is always at least one bit wide.
pub fn index_bits(entries: usize) -> u32 {
    let mut bits = 1u32;
    while (1usize << bits) < entries {
        bits += 1;
    }
    bits
}

/// Squared error between two equal-length byte slices.
#[inline]
pub(crate) fn sse(a: &[u8], b: &[u8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let d = i32::from(x) - i32::from(y);
            (d * d) as u32
        })
        .sum()
}

/// A trained (or supplied) codebook of 8x8 patches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dictionary {
    /// Flat `entries * PATCH_DIM` bytes.
    entries: Vec<u8>,
    /// Low 16 bits of the CRC-32 over `(entry count as u16 LE) ++ entries`.
    id: u16,
}

impl Dictionary {
    /// Build a dictionary from flat entry bytes. `entries.len()` must be
    /// a non-zero multiple of [`PATCH_DIM`], with at most
    /// [`MAX_DICTIONARY_ENTRIES`] entries.
    pub fn from_entries(entries: Vec<u8>) -> CodecResult<Dictionary> {
        if entries.is_empty() || entries.len() % PATCH_DIM != 0 {
            return Err(CodecError::InvalidConfig(
                "mote dictionary length must be a non-zero multiple of 64",
            ));
        }
        let n = entries.len() / PATCH_DIM;
        if n > MAX_DICTIONARY_ENTRIES {
            return Err(CodecError::InvalidConfig(
                "mote dictionary has more than 4096 entries",
            ));
        }
        let mut buf = Vec::with_capacity(2 + entries.len());
        buf.extend_from_slice(&(n as u16).to_le_bytes());
        buf.extend_from_slice(&entries);
        let id = crc32(&buf) as u16;
        Ok(Dictionary { entries, id })
    }

    /// Number of entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len() / PATCH_DIM
    }

    /// Always false: a dictionary has at least one entry.
    #[inline]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Entry `i` (`i < len()`).
    #[inline]
    pub fn entry(&self, i: usize) -> &[u8] {
        &self.entries[i * PATCH_DIM..(i + 1) * PATCH_DIM]
    }

    /// All entries, flat.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.entries
    }

    /// The 16-bit identifier every frame packet carries, so a decoder
    /// holding a different dictionary refuses the stream instead of
    /// rendering garbage.
    #[inline]
    pub fn id(&self) -> u16 {
        self.id
    }

    /// Width in bits of one codebook index for this dictionary.
    #[inline]
    pub fn index_bits(&self) -> u32 {
        index_bits(self.len())
    }

    /// Size in bytes the dictionary occupies in memory (decoder side).
    pub fn memory_bytes(&self) -> usize {
        self.entries.len()
    }

    /// Entries widened to `f32`, the form `tpt-dsp-core`'s search takes.
    pub(crate) fn to_f32(&self) -> Vec<f32> {
        self.entries.iter().map(|&b| f32::from(b)).collect()
    }

    /// Train on every patch of `frames` (all planes). See the module docs
    /// for the algorithm. All frames must share `format` and dimensions.
    pub fn train_from_frames(frames: &[VideoFrame], cfg: &TrainConfig) -> CodecResult<Dictionary> {
        if frames.is_empty() {
            return Err(CodecError::InvalidConfig("mote training needs a frame"));
        }
        let mut patches: Vec<u8> = Vec::new();
        let mut shape: Option<(u32, u32, PixelFormat)> = None;
        for f in frames {
            if !is_coded_format(f.format) {
                return Err(CodecError::Unsupported("mote pixel format"));
            }
            let this = (f.width, f.height, f.format);
            match shape {
                None => shape = Some(this),
                Some(s) if s != this => {
                    return Err(CodecError::InvalidConfig(
                        "mote training frames must share geometry and format",
                    ))
                }
                _ => {}
            }
            let pic = Picture::from_frame(f)?;
            let grid = PatchGrid::new(pic.width, pic.height, pic.format);
            let mut p = [0u8; PATCH_DIM];
            for g in 0..grid.total() {
                grid.load(&pic, g, &mut p);
                patches.extend_from_slice(&p);
            }
        }
        Dictionary::train_patches(&patches, cfg)
    }

    /// Train on a flat stream of 64-byte patches.
    pub fn train_patches(patches: &[u8], cfg: &TrainConfig) -> CodecResult<Dictionary> {
        if patches.is_empty() || patches.len() % PATCH_DIM != 0 {
            return Err(CodecError::InvalidConfig(
                "mote training data must be whole 64-byte patches",
            ));
        }
        if cfg.entries == 0 || cfg.entries > MAX_DICTIONARY_ENTRIES || cfg.max_vectors == 0 {
            return Err(CodecError::InvalidConfig(
                "mote training entries must be 1..=4096 and max_vectors non-zero",
            ));
        }
        let total = patches.len() / PATCH_DIM;

        // 1. Deterministic strided subsample.
        let n = total.min(cfg.max_vectors);
        let mut vecs: Vec<f32> = Vec::with_capacity(n * PATCH_DIM);
        for i in 0..n {
            let src = if n == total {
                i
            } else {
                ((i as u64 * total as u64) / n as u64) as usize
            };
            vecs.extend(
                patches[src * PATCH_DIM..(src + 1) * PATCH_DIM]
                    .iter()
                    .map(|&b| f32::from(b)),
            );
        }
        let entries = cfg.entries.min(n);

        // 2. Seeded distinct-patch initialisation.
        let mut rng = SplitMix64::new(cfg.seed);
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut means: Vec<f32> = Vec::with_capacity(entries * PATCH_DIM);
        for i in 0..entries {
            let j = i + (rng.next() % (n - i) as u64) as usize;
            order.swap(i, j);
            let v = order[i] as usize;
            means.extend_from_slice(&vecs[v * PATCH_DIM..(v + 1) * PATCH_DIM]);
        }

        // 3-6. Lloyd.
        let mut assign = vec![0u32; n];
        let mut prev = vec![u32::MAX; n];
        let mut dist = vec![0u32; n];
        let mut sums = vec![0u64; entries * PATCH_DIM];
        let mut counts = vec![0u32; entries];
        for _ in 0..cfg.iterations.max(1) {
            for v in 0..n {
                let q = &vecs[v * PATCH_DIM..(v + 1) * PATCH_DIM];
                let (idx, d) = nearest_vector_accelerated(
                    &means,
                    PATCH_DIM,
                    q,
                    DistanceMetric::SquaredEuclidean,
                );
                assign[v] = idx as u32;
                // Exact: see module docs (integer-valued, < 2^24).
                dist[v] = d as u32;
            }
            if assign == prev {
                break;
            }
            prev.copy_from_slice(&assign);

            sums.iter_mut().for_each(|s| *s = 0);
            counts.iter_mut().for_each(|c| *c = 0);
            for v in 0..n {
                let c = assign[v] as usize;
                counts[c] += 1;
                for k in 0..PATCH_DIM {
                    sums[c * PATCH_DIM + k] += vecs[v * PATCH_DIM + k] as u64;
                }
            }
            for c in 0..entries {
                if counts[c] > 0 {
                    let n_c = u64::from(counts[c]);
                    for k in 0..PATCH_DIM {
                        let m = (2 * sums[c * PATCH_DIM + k] + n_c) / (2 * n_c);
                        means[c * PATCH_DIM + k] = m as f32;
                    }
                } else {
                    // Re-seat on the farthest patch (first wins ties).
                    let mut far = 0usize;
                    let mut far_d = 0u32;
                    for (v, &d) in dist.iter().enumerate() {
                        if d > far_d {
                            far_d = d;
                            far = v;
                        }
                    }
                    means[c * PATCH_DIM..(c + 1) * PATCH_DIM]
                        .copy_from_slice(&vecs[far * PATCH_DIM..(far + 1) * PATCH_DIM]);
                    dist[far] = 0;
                }
            }
        }
        Dictionary::from_entries(means.iter().map(|&m| m as u8).collect())
    }
}

/// Training parameters. Every field is part of the determinism contract:
/// the same patches and the same `TrainConfig` always yield the same
/// dictionary bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrainConfig {
    /// Target number of entries, `1..=4096`. Clamped down to the number
    /// of training patches actually available.
    pub entries: usize,
    /// Maximum Lloyd iterations (at least one runs).
    pub iterations: u32,
    /// SplitMix64 seed for the initial centroid choice.
    pub seed: u64,
    /// Cap on the number of training patches used (strided subsample
    /// above it); bounds training time and memory.
    pub max_vectors: usize,
}

impl Default for TrainConfig {
    fn default() -> Self {
        TrainConfig {
            entries: MAX_DICTIONARY_ENTRIES,
            iterations: 6,
            seed: 0x4D4F_5445_0000_0001,
            max_vectors: 32_768,
        }
    }
}

/// SplitMix64 (Steele, Lea & Flood 2014): a tiny seeded generator used
/// only for the training initialisation.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
