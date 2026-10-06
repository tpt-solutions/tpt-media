//! `tpt-video-vault` - Vault: TPT high-compression video codec.
//!
//! ## Design (v0.1)
//!
//! Vault is the compression-efficiency end of the native video family:
//! where Thrum trades coding gain for latency, Vault spends encoder work
//! to save bits. It is an I/P hybrid with a **quadtree of block sizes**:
//!
//! - **64x64 superblocks, recursively partitioned** into 32x32, 16x16 and
//!   down to a **minimum of 8x8**. A node is either one *leaf* (one
//!   prediction + one residual) or four children. A node that hangs over
//!   the picture edge splits implicitly (no flag), so any dimensions that
//!   are multiples of 8 are coded with no padding. The recursion is
//!   structurally three levels deep ([`MAX_PARTITION_DEPTH`]).
//! - **Motion vectors at half-pel precision.** The encoder searches
//!   full-pel with a shrinking-step pattern search and refines to half-pel
//!   over the eight neighbouring half-pel positions. The interpolation is
//!   deliberately the simplest one (a 2-tap average, and a 4-sample average
//!   on the diagonal) - CONTRIBUTING section 3 rules out longer filters.
//!   Vectors are coded as a difference from a left/top-neighbour
//!   predictor. Chroma vectors are derived (the luma vector halved), not
//!   signalled.
//! - **Up to 16 reference frames** ([`MAX_REFERENCE_FRAMES`]): the last
//!   `max_refs` loop-filtered reconstructions, most recent first. Each
//!   inter leaf names its reference. Skip always means reference 0 with the
//!   predicted vector. Optional B-frames
//!   ([`VaultConfig::b_frames`]) bi-predict from the reference frames on
//!   both sides of their display position and never become references
//!   themselves; display order is recovered from the per-frame `poc`.
//! - **Four intra modes** at every block size (DC, vertical, horizontal,
//!   and a plane-extrapolation mode), chroma sharing the luma mode.
//! - **2D DCT** from `tpt-dsp-core` at 4x4 to 32x32 (a 64x64 leaf is four
//!   32x32 transform tiles), quantised in orthonormal units so one `qp`
//!   means the same step at every size.
//! - **Rate-distortion optimisation**: partition, prediction mode,
//!   reference and vector are chosen by comparing `J = D + lambda * R`
//!   (see `encoder.rs`). **This is not claimed to find the global
//!   optimum** - it is a greedy per-node comparison over a heuristically
//!   pruned candidate set.
//! - **Entropy coding**: static Rice codes ([`tpt_media_bitstream::rice`])
//!   with a per-block parameter, in a run/level scheme over a diagonal
//!   scan. No context-adaptive arithmetic coding (CONTRIBUTING section 3).
//! - **Loop filter**: a boundary-strength-driven 2-strength deblocking
//!   filter that only touches real coding-structure edges (leaf
//!   boundaries, and the 32x32 tile seams inside a 64x64 leaf); the
//!   filtered picture is the next reference. Signalled per frame by a
//!   header bit: from [`VaultConfig::loop_filter`] as-is, or decided
//!   per frame by rate–distortion with
//!   [`VaultConfig::loop_filter_rdo`].
//!
//! ## Pixel formats
//!
//! [`PixelFormat::Gray8`] and planar [`PixelFormat::Yuv420`]; anything else
//! is refused with [`CodecError::Unsupported`]. Width and height are
//! non-zero multiples of [`MIN_BLOCK`] (8).
//!
//! ## Wire format (version 2)
//!
//! ```text
//! 16 bits version | 2 ftype | 6 qp | 5 num_refs | 1 loop_filter | 16 poc
//! superblocks in raster order, each a quadtree of `split` flags and leaves
//! 32 bits CRC-32 (little endian) over everything before it
//! ```
//!
//! The CRC is verified before any other bit is parsed and the version is
//! rejected up front (todo.md section 35). See `docs/codec-design.md`
//! for the full leaf syntax.
//!
//! ## Limits, stated plainly
//!
//! - The encoder is not claimed real-time (`Capabilities` says so); the
//!   measured cost is in `docs/performance.md` — the transform runs on
//!   `tpt-dsp-core`'s FFT-backed `FastDct2dPlan` in std builds, which
//!   brought every §36 corpus scene inside the 33.3 ms QCIF frame
//!   budget (bare metal keeps the direct path).
//! - The largest transform is 32x32; a 64x64 leaf is coded as four such
//!   tiles.
//! - No weighted prediction, no long-term references or reference
//!   reordering, no per-block quantiser adaptation. Rate control,
//!   B-frames, scene-cut detection ([`VaultConfig::scene_cut`]) and
//!   coefficient-level RDO ([`VaultConfig::rdoq`]) exist as
//!   encoder-only knobs; the scene-cut constants and `lambda` are
//!   empirical and untuned on real content.
//!
//! See `docs/provenance.md` for algorithm sources.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![forbid(unsafe_code)]
extern crate alloc;

mod bits;
mod decoder;
mod encoder;
mod filter;
mod picture;
mod pred;
mod syntax;
mod transform;

use alloc::vec;
use alloc::vec::Vec;

use tpt_media_codec::{Capabilities, CodecError, CodecInfo, CodecResult, SupportedFormat};
use tpt_media_core::PixelFormat;

pub use decoder::{FrameStats, VaultDecoder, VaultDecoderFactory, VaultDynDecoder};
pub use encoder::{VaultDynEncoder, VaultEncoder, VaultEncoderFactory, LAMBDA_SCALE};
pub use pred::{intra_mode_available, IntraMode};
pub use transform::ROUND_OFFSET;

/// Codec id for Vault.
pub const CODEC_ID: [u8; 4] = *b"VAUL";

/// Bitstream version.
pub const VERSION: u16 = 2;

/// Size in bytes of the per-packet CRC-32 trailer.
pub const CRC_TRAILER_BYTES: usize = 4;

/// Edge length of a top-level superblock, in luma samples.
pub const SUPERBLOCK: usize = 64;

/// Edge length of the smallest block, in luma samples. Frame dimensions
/// are multiples of this.
pub const MIN_BLOCK: usize = 8;

/// Largest transform edge. A 64x64 leaf is coded as four 32x32 tiles.
pub const MAX_TRANSFORM: usize = 32;

/// Deepest partition level: 64 (0) -> 32 (1) -> 16 (2) -> 8 (3).
pub const MAX_PARTITION_DEPTH: u8 = 3;

/// Most reference frames a stream may use (5 bits on the wire, 16 by the
/// `todo.md` section 33 target).
pub const MAX_REFERENCE_FRAMES: u8 = 16;

/// Default I-frame interval: an I-frame this often, and always the first.
pub const KEYFRAME_INTERVAL: u64 = 60;

/// Smallest legal quantiser index.
pub const MIN_QP: u8 = 1;
/// Largest legal quantiser index (6 bits on the wire).
pub const MAX_QP: u8 = 51;

/// Largest supported bi-predictive depth (`b_frames`).
pub const MAX_B_FRAMES: usize = 4;

/// Frame types on the v2 wire: the header's 2-bit `ftype` field. `Rsv`
/// (3) is rejected as malformed so the encoding has room to grow.
pub mod ftype {
    /// Intra: coded without references; resets the display sequence.
    pub const I: u8 = 0;
    /// Predictive: references earlier display frames.
    pub const P: u8 = 1;
    /// Bi-predictive: references frames on both sides of its display
    /// position; never becomes a reference itself.
    pub const B: u8 = 2;
    /// Reserved; rejected by decoders.
    pub const RSV: u8 = 3;
}
/// Bits used for the `qp` header field.
pub const QP_BITS: u32 = 6;

/// Largest legal `search_range` (full pixels).
pub const MAX_SEARCH_RANGE: u8 = 32;
/// Default motion-search radius (full pixels).
pub const DEFAULT_SEARCH_RANGE: u8 = 16;
/// Largest motion-vector component, in half-pel units: a full-pel search
/// of `MAX_SEARCH_RANGE` plus one half-pel refinement step.
pub const MV_LIMIT: i32 = 2 * MAX_SEARCH_RANGE as i32 + 1;

/// Largest level the quantiser can produce.
///
/// Derivable: a residual sample is in `[-255, 255]`, so by Parseval an
/// orthonormal-unit coefficient of a tile of edge `t <= 32` has magnitude
/// at most `255 * t <= 8160`; dividing by the smallest step (1.122) gives
/// at most 7273 before rounding, so 8192 cannot be exceeded. Clamping to it
/// is what makes [`VaultConfig::max_packet_bits`] a proof, not a hope.
pub(crate) const MAX_LEVEL: i32 = 8192;

/// Decoder-side clamp on a level read off the wire, independent of
/// [`MAX_LEVEL`]: bounds reconstruction and work on hostile input.
pub(crate) const LEVEL_CLAMP: i32 = 32767;

/// Quantiser step for each `qp` in `1..=MAX_QP`, indexed by `qp - 1`: a
/// geometric table (`2^(qp/6)`), tabulated so the step is bit-identical
/// everywhere and needs no float `pow`.
pub const QUANT_STEP: [f32; MAX_QP as usize] = [
    1.122, 1.260, 1.414, 1.587, 1.782, 2.000, 2.245, 2.520, 2.828, 3.175, //
    3.565, 4.000, 4.490, 5.040, 5.657, 6.350, 7.127, 8.000, 8.986, 10.079, //
    11.314, 12.699, 14.267, 16.000, 17.959, 20.159, 22.627, 25.398, 28.508, 32.000, //
    35.918, 40.319, 45.255, 50.797, 57.022, 64.000, 71.836, 80.635, 90.510, 101.594, //
    114.045, 128.000, 143.673, 161.269, 181.019, 203.188, 228.070, 256.000, 287.347,
    322.537, //
    362.039,
];

/// The quantiser step for a `qp`, clamped into `1..=MAX_QP`.
#[inline]
pub fn quant_step(qp: u8) -> f32 {
    let qp = qp.clamp(MIN_QP, MAX_QP);
    QUANT_STEP[(qp - 1) as usize]
}

/// The CRC-32 (IEEE 802.3) of Vault's per-packet integrity trailer, exposed
/// so tests can build a well-formed packet without reaching into internals.
pub fn packet_crc(bytes: &[u8]) -> u32 {
    crc32(bytes)
}

/// CRC-32 (IEEE 802.3, reflected polynomial `0xEDB88320`): the same
/// per-packet integrity convention as the rest of the native family.
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// True for the pixel formats Vault codes.
#[inline]
pub fn is_coded_format(format: PixelFormat) -> bool {
    matches!(format, PixelFormat::Gray8 | PixelFormat::Yuv420)
}

/// Largest legal frame edge.
const MAX_DIMENSION: u32 = 16384;

/// Vault configuration.
///
/// Frame geometry is part of the configuration (and of the track header,
/// see [`VaultConfig::to_codec_private`]) rather than of every packet.
#[derive(Clone, Debug, PartialEq)]
pub struct VaultConfig {
    /// Coded frame width in luma samples; a non-zero multiple of [`MIN_BLOCK`].
    pub width: u32,
    /// Coded frame height in luma samples; a non-zero multiple of [`MIN_BLOCK`].
    pub height: u32,
    /// Coded pixel format: [`PixelFormat::Gray8`] or [`PixelFormat::Yuv420`].
    pub format: PixelFormat,
    /// Quantiser index for every frame, `1..=MAX_QP` (lower is better).
    pub qp: u8,
    /// Full-pel motion-search radius, `0..=MAX_SEARCH_RANGE`.
    pub search_range: u8,
    /// Reference pictures kept and searched, `1..=MAX_REFERENCE_FRAMES`.
    /// A decoder under the default (strict) resource limits accepts up to
    /// 8; more needs `ResourceLimits::permissive()` or a raised
    /// `max_reference_frames`.
    pub max_refs: u8,
    /// Largest leaf the *encoder* may choose: 8, 16, 32 or 64. Encoder-only
    /// (a stream is decodable whatever the value was), so it is not part of
    /// the codec-private bytes.
    pub max_block_size: usize,
    /// Run the loop filter (signalled per frame in the header).
    pub loop_filter: bool,
    /// Coefficient-level rate-distortion optimised quantisation
    /// (RDOQ): drop a quantised level when the Rice-coded bits it
    /// costs buy more distortion than they save under the leaf RDO's
    /// own `lambda`. Encoder-only (a stream is decodable either way).
    ///
    /// Defaults to `false` on the measured evidence: over the whole
    /// §36 corpus at the default `qp` the pass produced byte-identical
    /// output while costing up to +24% encode time on dense scenes
    /// (todo.md §33). It fires at finer quantiser steps — the unit
    /// tests pin the decision at both ends — so the knob is there for
    /// fine-`qp` and real-content encodes, pending a `λ` tuned on
    /// content that actually exists.
    pub rdoq: bool,
    /// Bi-predictive frames between consecutive reference frames.
    /// `0` (the default) codes an I/P-only stream; `2` codes
    /// I P B B P B … with each B predicted from the reference frames
    /// on both sides of its display position. Encoder-only, and
    /// B-frames are never stored as references. Display order is
    /// recovered by the decoder from the per-frame `poc` field, so
    /// consumers see frames in display order with a `b_frames`-period
    /// presentation delay.
    pub b_frames: usize,
    /// Frame-level rate control: when set, each frame is coded at the
    /// quantiser that keeps the stream near this many **bits per
    /// frame** — a bounded search around the previous frame's choice
    /// (at most three encodes), with a 4× allowance on intra frames
    /// while the budget bucket recovers. The frame header already
    /// carries a per-frame `qp`, so this is encoder-only: no wire
    /// change, and `qp` is ignored while this is `Some`.
    pub target_bits_per_frame: Option<u32>,
    /// Scene-cut detection: a reference frame whose mean absolute luma
    /// difference from the most recent reference spikes above both an
    /// absolute floor and a factor times the previous frame's
    /// difference is coded intra instead of predictive, rather than
    /// waiting for the next scheduled keyframe. Encoder-only (an
    /// intra is an intra to the decoder, however it was decided); the
    /// constants are empirical and untuned on real content.
    pub scene_cut: bool,
    /// Decide the per-frame loop-filter flag by rate–distortion
    /// instead of always signalling [`VaultConfig::loop_filter`]:
    /// after the frame is reconstructed the encoder filters a copy,
    /// keeps whichever of the two is closer to the source, and writes
    /// that choice into the header bit the decoder already reads.
    /// Encoder-only; ties keep the filter. Same-build deterministic.
    pub loop_filter_rdo: bool,
}

impl Default for VaultConfig {
    fn default() -> Self {
        VaultConfig {
            width: 128,
            height: 96,
            format: PixelFormat::Yuv420,
            qp: 30,
            search_range: DEFAULT_SEARCH_RANGE,
            max_refs: 4,
            max_block_size: SUPERBLOCK,
            loop_filter: true,
            target_bits_per_frame: None,
            b_frames: 0,
            rdoq: false,
            scene_cut: false,
            loop_filter_rdo: false,
        }
    }
}

impl VaultConfig {
    /// Number of codec-private bytes [`VaultConfig::to_codec_private`]
    /// produces.
    pub const CODEC_PRIVATE_LEN: usize = 5;

    /// Bytes of pixel storage one frame occupies.
    pub fn frame_bytes(&self) -> usize {
        let n = self.width as usize * self.height as usize;
        if self.format == PixelFormat::Yuv420 {
            n + n / 2
        } else {
            n
        }
    }

    /// Number of 8x8 luma units in a frame.
    pub fn unit_count(&self) -> usize {
        (self.width as usize / MIN_BLOCK) * (self.height as usize / MIN_BLOCK)
    }

    /// Upper bound on the bits one frame's packet body can occupy
    /// (excluding the CRC trailer).
    ///
    /// Provable, because every term is bounded:
    ///
    /// - header: 16 + 1 + [`QP_BITS`] + 5 + 1 bits;
    /// - per 8x8 unit, charged as if every block were 8x8 (larger blocks
    ///   have strictly fewer headers per unit of area): 2 bits of split
    ///   flags (a quadtree has fewer than 4/3 nodes per leaf unit), 2 bits
    ///   skip/intra, 4 bits reference index, two motion-vector differences
    ///   of at most `2 * MV_LIMIT` (signed Rice at `k = 1`), 2 bits intra
    ///   mode, and the per-tile `coded`/`k`/`last` fields (10 bits per
    ///   8x8 luma tile, 8 bits per 4x4 chroma tile);
    /// - per sample, one run token and one level token, each at most
    ///   `MAX_RICE_BITS` (136): the encoder picks the cheapest `k`, so a
    ///   block never costs more than at `k = 7`, and `MAX_LEVEL` bounds the
    ///   largest folded value.
    ///
    /// The encoder checks the finished packet against this and returns
    /// [`CodecError::Internal`] rather than emit an over-bound packet.
    pub fn max_packet_bits(&self) -> usize {
        let header = 16 + 1 + u64::from(QP_BITS) + 5 + 1;
        let mvd = u64::from(bits::rice_len_signed(2 * MV_LIMIT, syntax::MVD_K));
        let per_unit_syntax = 2 + 2 + 4 + 2 * mvd + 2;
        let token = 2 * u64::from(transform::MAX_RICE_BITS);
        let (tile_hdr, samples) = if self.format == PixelFormat::Yuv420 {
            (10 + 2 * 8, 64 + 32)
        } else {
            (10, 64)
        };
        let total =
            header + self.unit_count() as u64 * (per_unit_syntax + tile_hdr + samples * token);
        // 64-bit arithmetic, saturating on a 32-bit `usize`.
        usize::try_from(total).unwrap_or(usize::MAX)
    }

    pub(crate) fn validate(&self) -> CodecResult<()> {
        if !is_coded_format(self.format) {
            return Err(CodecError::InvalidConfig(
                "vault supports only Gray8 and Yuv420",
            ));
        }
        if self.width == 0
            || self.height == 0
            || self.width > MAX_DIMENSION
            || self.height > MAX_DIMENSION
            || (self.width as usize) % MIN_BLOCK != 0
            || (self.height as usize) % MIN_BLOCK != 0
        {
            return Err(CodecError::InvalidConfig(
                "vault width/height must be non-zero multiples of 8, at most 16384",
            ));
        }
        if !(MIN_QP..=MAX_QP).contains(&self.qp) {
            return Err(CodecError::InvalidConfig("vault qp must be 1..=51"));
        }
        if self.search_range > MAX_SEARCH_RANGE {
            return Err(CodecError::InvalidConfig(
                "vault search_range must be 0..=32",
            ));
        }
        if self.max_refs == 0 || self.max_refs > MAX_REFERENCE_FRAMES {
            return Err(CodecError::InvalidConfig("vault max_refs must be 1..=16"));
        }
        if !matches!(self.max_block_size, 8 | 16 | 32 | 64) {
            return Err(CodecError::InvalidConfig(
                "vault max_block_size must be 8, 16, 32 or 64",
            ));
        }
        if self.b_frames > MAX_B_FRAMES {
            return Err(CodecError::InvalidConfig("vault b_frames must be 0..=4"));
        }
        Ok(())
    }

    /// Encode the codec-side configuration into the track's private bytes
    /// (`format`, `qp`, `search_range`, `max_refs`, `flags` with bit 0 =
    /// loop filter). Geometry lives in the track header.
    pub fn to_codec_private(&self) -> Vec<u8> {
        vec![
            self.format as u8,
            self.qp,
            self.search_range,
            self.max_refs,
            u8::from(self.loop_filter),
        ]
    }

    /// Inverse of [`VaultConfig::to_codec_private`], with the geometry
    /// supplied from the track header. `max_block_size` is encoder-only and
    /// comes back as its default.
    ///
    /// # Errors
    /// [`CodecError::InvalidConfig`] for an unknown format or a value that
    /// fails validation.
    pub fn from_codec_private(width: u32, height: u32, private: &[u8]) -> CodecResult<VaultConfig> {
        let mut cfg = VaultConfig {
            width,
            height,
            ..VaultConfig::default()
        };
        if private.len() >= VaultConfig::CODEC_PRIVATE_LEN {
            cfg.format = match private[0] {
                0 => PixelFormat::Gray8,
                3 => PixelFormat::Yuv420,
                _ => return Err(CodecError::InvalidConfig("vault pixel format")),
            };
            cfg.qp = private[1];
            cfg.search_range = private[2];
            cfg.max_refs = private[3];
            cfg.loop_filter = private[4] & 1 != 0;
        }
        cfg.validate()?;
        Ok(cfg)
    }
}

/// Map a caller-facing `0..=100` quality onto a `qp` index: 100 is the
/// best quality (`qp = MIN_QP`), 0 the worst (`qp = MAX_QP`). Monotonic and
/// covering the whole `MIN_QP..=MAX_QP` range.
pub fn quality_to_qp(quality: u8) -> u8 {
    let q = u32::from(quality.min(100));
    let span = u32::from(MAX_QP - MIN_QP);
    MIN_QP + (span - (q * span) / 100) as u8
}

/// Static codec information for a geometry.
pub(crate) fn static_info(
    width: u32,
    height: u32,
    format: PixelFormat,
    caps: Capabilities,
) -> CodecInfo {
    CodecInfo {
        name: "Vault",
        codec_id: CODEC_ID,
        version: VERSION,
        capabilities: caps,
        supported_formats: vec![SupportedFormat::Video {
            pixel_format: format,
            width,
            height,
        }],
    }
}

#[cfg(test)]
mod tests;
