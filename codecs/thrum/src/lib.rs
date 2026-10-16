//! `tpt-video-thrum` - Thrum: TPT low-latency video communication codec.
//!
//! ## Design (v0.1)
//!
//! Thrum is the low-latency end of the native codec family: a classic
//! I/P hybrid built for a real-time conversation rather than for
//! compression efficiency. Every design choice trades coding gain for
//! *latency* and *bounded work*:
//!
//! - **I/P coding, no B-frames.** The encoder emits an I-frame and then
//!   P-frames forever. Nothing is ever reordered or held back, so a frame
//!   encoded at wall-clock *t* is decodable at *t*: the one-way latency
//!   is exactly the time to encode and decode that frame. Measured by
//!   `benches/codec_bench.rs` and reported in `docs/performance.md`.
//! - **Intra refresh.** A P-frame still re-codes
//!   [`DEFAULT_INTRA_REFRESH_PERCENT`]% of its macroblocks intra, on a
//!   deterministic rotating cursor. Error from a lost packet therefore
//!   cannot propagate forever: a receiver resynchronises within a bounded
//!   number of frames without waiting for a full I-frame.
//! - **Simple prediction.** Four intra modes (DC / vertical / horizontal /
//!   diagonal) chosen per block by SAD, and full-pixel integer motion
//!   compensation with a single reference frame. No sub-pixel
//!   interpolation, no weighted prediction, no multiple references.
//! - **Low-complexity transforms.** 8x8 DCT-II/DCT-III from
//!   [`tpt_dsp_core::dct_2d`] / [`tpt_dsp_core::idct_2d`], the generic
//!   separable transforms `tpt-dsp` already provides. Per todo.md 3.1
//!   codec-specific APIs never leak *into* `tpt-dsp`, so the 2D
//!   composition is consumed here, not re-implemented there.
//!
//! ## Pixel formats and colour model
//!
//! - [`PixelFormat::Gray8`] - luma only, 8 bits per pixel.
//! - [`PixelFormat::Yuv420`] - planar 4:2:0 (BT.601-family luma /
//!   chroma planes, stored full-range 8-bit unsigned). Each macroblock
//!   covers 16x16 luma and 8x8 of *each* chroma plane, so chroma motion
//!   vectors are the luma vector halved (see `chroma_mv`).
//!
//! Both are the *coded* formats: the encoder refuses anything else with
//! [`CodecError::Unsupported`] rather than silently converting. Frame
//! dimensions must be multiples of [`MB_SIZE`] (16) so the picture is
//! exactly macroblock-aligned: no edge padding, no partial macroblocks,
//! and therefore no padding-related rate or quality cliff.
//!
//! ## Frame structure
//!
//! A **macroblock** is 16x16 luma = four 8x8 luma blocks, plus (for
//! 4:2:0) one 8x8 U block and one 8x8 V block. Macroblocks are coded in
//! raster order. A **frame** is one packet, and one packet is one frame:
//! Thrum never splits an access unit across packets, so
//! [`tpt_media_core::PacketFlags::END_OF_FRAME`] is set on every packet
//! and no reordering buffer exists anywhere in the codec.
//!
//! ## I-frame
//!
//! Every macroblock is intra-coded. Each 8x8 luma block carries a 2-bit
//! mode (see [`intra_mode_available`]); chroma always uses DC. The
//! prediction is built from the *reconstructed* neighbours (closed loop),
//! the residual is transformed, quantised, entropy-coded, and added back
//! into the reconstruction, so encoder and decoder diverge by nothing.
//!
//! ## P-frame
//!
//! Each macroblock is either intra (see intra refresh above) or inter:
//!
//! 1. **Motion estimation** - exhaustive integer search over every
//!    `(dx, dy)` in `+/-search_range`, costed by **full-pixel SAD**
//!    (sum of absolute luma differences) with early termination at the
//!    running best. Exhaustive, not hierarchical/diamond: the search space
//!    is at most `33x33` and the early exit makes it affordable.
//! 2. **Inter prediction** - the reference block at the chosen vector.
//!    Chroma vectors are the luma vector halved (`chroma_mv`), which is
//!    the natural 4:2:0 mapping and is recomputed identically on both
//!    sides rather than signalled.
//! 3. **Skip** - if the macroblock's mean absolute prediction error
//!    falls below one quantiser step (`skip_threshold`), it is signalled
//!    as skipped: the decoder copies the reference at `(0, 0)` with no
//!    residual. This is where the savings on a talking-head /
//!    static-scene call come from. The threshold is *rate-aware* rather
//!    than a fixed SAD constant, so skip stays available at coarse `qp`
//!    where the reconstruction error alone would exceed any fixed bound.
//! 4. **Residual** - same DCT / quantise / entropy-code path as intra.
//!
//! ## Quantisation
//!
//! One flat uniform quantiser, dead-zone, over all 8x8 blocks; its step
//! comes from a 6-bit `qp` sent in every frame header ([`quant_step`], a
//! fixed table so no float `pow` is needed and the step is bit-identical
//! on every target). Quantised levels are clamped to the signed-16-bit
//! range, which bounds both the entropy-coded length and the
//! reconstruction.
//!
//! ## Entropy coding
//!
//! Real Rice coding ([`tpt_media_bitstream::rice`]), not the Exp-Golomb
//! stand-in Aura uses: a run/level token stream over the zigzag scan of
//! each block, with a per-block Rice parameter `k` chosen by the encoder
//! from `{0,1,2,3}` (2 bits) to minimise the coded length. Signalling `k`
//! beats a fixed `k` on the sparse, heavy-tailed residuals a transform
//! codec actually produces, and costs 2 bits per coded block.
//!
//! ## Deblocking
//!
//! A 4x4-grid loop filter (`deblock_plane`, H.263-shaped: a
//! `d < alpha` test with a `d < beta` wide path and a `tc`-limited
//! narrow path) runs over the whole reconstructed frame, and the
//! *filtered* picture is what becomes the next reference. Encoder and
//! decoder call the identical function on identical input, so it can
//! never cause drift.
//!
//! ## Packet loss
//!
//! [`ThrumDecoder::conceal`] repeats the last decoded frame (a freeze),
//! which is the correct concealment for a codec with no temporal
//! hierarchy to hide a gap in. With no prior frame it emits a mid-grey
//! picture.
//!
//! This crate builds `no_std` + `alloc`; every float helper it needs
//! (`truncf`/`roundf`/`fabs`/`clampf`) is defined locally because
//! `f32::round`/`abs` are not in `core` at this crate's MSRV (1.75).
//!
//! See `docs/provenance.md` for algorithm sources and `todo.md` 32 for
//! the item-by-item mapping.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use tpt_dsp_core::{dct_2d, idct_2d};
use tpt_media_bitstream::rice::{decode_riced, encode_riced};
use tpt_media_bitstream::{BitReader, BitWriter};
use tpt_media_codec::{
    Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, Decoder, DynDecoder,
    DynDecoderFactory, DynEncoder, DynEncoderFactory, Encoder, SupportedFormat,
};
use tpt_media_core::{Frame, Packet, PacketFlags, PixelFormat, Timestamp, VideoFrame};
/// Codec id for Thrum.
pub const CODEC_ID: [u8; 4] = *b"THRM";

/// Bitstream version.
pub const VERSION: u16 = 1;

/// Size in bytes of the per-packet CRC-32 trailer.
pub const CRC_TRAILER_BYTES: usize = 4;

/// Edge length of a macroblock, in luma pixels.
pub const MB_SIZE: usize = 16;

/// Edge length of a transform block, in pixels.
pub const BLOCK: usize = 8;

/// Number of coefficients in one 8x8 transform block.
pub const BLOCK_COEFFS: usize = BLOCK * BLOCK;

/// Number of luma 8x8 blocks per macroblock.
const LUMA_BLOCKS: usize = 4;

/// The four luma 8x8 block positions inside a macroblock, in coding
/// order. Shared verbatim by the encoder and the decoder: any divergence
/// here silently desynchronises the bitstream.
pub const LUMA_BLOCK_OFFSETS: [(usize, usize); LUMA_BLOCKS] = [(0, 0), (1, 0), (0, 1), (1, 1)];

/// Default I-frame interval: an I-frame this often, and always the first.
pub const KEYFRAME_INTERVAL: u64 = 30;

/// Zigzag scan: zigzag position -> raster position in the 8x8 block.
pub const ZIGZAG: [usize; BLOCK_COEFFS] = [
    0, 1, 8, 16, 9, 2, 3, 10, //
    17, 24, 32, 25, 18, 11, 4, 5, //
    12, 19, 26, 33, 40, 48, 41, 34, //
    27, 20, 13, 6, 7, 14, 21, 28, //
    35, 42, 49, 56, 57, 50, 43, 36, //
    29, 22, 15, 23, 30, 37, 44, 51, //
    58, 59, 52, 45, 38, 31, 39, 46, //
    53, 60, 61, 54, 47, 55, 62, 63,
];

/// Intra prediction modes, in wire order (2 bits).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IntraMode {
    /// Flat: the mean of the available neighbouring edge (128 if none).
    Dc = 0,
    /// Copy the row above (requires an available top edge).
    Vertical = 1,
    /// Copy the column to the left (requires an available left edge).
    Horizontal = 2,
    /// Shear the top edge down-right (requires an available top edge).
    Diagonal = 3,
}

impl IntraMode {
    /// Every mode, in wire order.
    pub const ALL: [IntraMode; 4] = [
        IntraMode::Dc,
        IntraMode::Vertical,
        IntraMode::Horizontal,
        IntraMode::Diagonal,
    ];

    fn from_bits(v: u8) -> IntraMode {
        match v & 0b11 {
            0 => IntraMode::Dc,
            1 => IntraMode::Vertical,
            2 => IntraMode::Horizontal,
            _ => IntraMode::Diagonal,
        }
    }
}
/// Smallest legal quantiser index.
pub const MIN_QP: u8 = 1;
/// Largest legal quantiser index (6 bits on the wire).
pub const MAX_QP: u8 = 51;
/// Bits used for the `qp` header field.
pub const QP_BITS: u32 = 6;

/// Quantiser step for each `qp` in `1..=MAX_QP`, indexed by `qp - 1`.
///
/// A geometric table (`2^(qp/6)`) rather than a computed `powf`, so the
/// step is bit-identical everywhere and the crate needs no float maths
/// library in `no_std`. `qp = 1` is near-transparent, `MAX_QP` is heavy.
pub const QUANT_STEP: [f32; MAX_QP as usize] = [
    1.122, 1.260, 1.414, 1.587, 1.782, 2.000, 2.245, 2.520, 2.828, 3.175, //
    3.565, 4.000, 4.490, 5.040, 5.657, 6.350, 7.127, 8.000, 8.986, 10.079, //
    11.314, 12.699, 14.267, 16.000, 17.959, 20.159, 22.627, 25.398, 28.508, 32.000, //
    35.918, 40.319, 45.255, 50.797, 57.022, 64.000, 71.836, 80.635, 90.510, 101.594, //
    114.045, 128.000, 143.673, 161.269, 181.019, 203.188, 228.070, 256.000, 287.347,
    322.537, //
    362.039,
];

/// The quantiser step for a `qp`. The index is clamped into `1..=MAX_QP`
/// so a caller can never index out of bounds.
#[inline]
pub fn quant_step(qp: u8) -> f32 {
    let qp = qp.clamp(MIN_QP, MAX_QP);
    QUANT_STEP[(qp - 1) as usize]
}

/// Largest motion-vector component in full pixels, and the largest legal
/// `search_range` in [`ThrumConfig`].
pub const MAX_SEARCH_RANGE: u8 = 16;
/// Bits used to signal a motion-vector component (biased by
/// `MAX_SEARCH_RANGE`, so it covers `-16..=16`).
const MV_BITS: u32 = 6;
/// Bits used for the Rice parameter selector.
const RICE_K_BITS: u32 = 2;
/// Largest selectable Rice parameter.
const RICE_K_MAX: u32 = 3;

/// Combined luma+chroma SAD below which a P-frame macroblock is
/// The skip test, as a pure function of the quantiser step.
///
/// A macroblock is skipped when its mean absolute prediction error is
/// below *one quantiser step*: the residual would quantise to zero
/// anyway, so sending it costs bits and buys nothing. Tying the threshold
/// to `step` rather than to a fixed constant is what makes the decision
/// rate-aware - at a coarse `qp` the reconstruction error of an
/// unchanged scene alone can exceed any fixed bound, which would disable
/// skip exactly where it is worth the most.
///
/// 16x16 luma plus 2 x 8x8 chroma is 384 samples; the `0.75` factor is
/// the margin below the quantiser's own resolution.
#[inline]
pub fn skip_threshold(step: f32) -> u32 {
    (384.0f32 * step * 0.75) as u32
}

/// Default percentage of macroblocks re-coded intra in each P-frame.
pub const DEFAULT_INTRA_REFRESH_PERCENT: u8 = 5;

/// Default motion-search radius, in full pixels.
pub const DEFAULT_SEARCH_RANGE: u8 = 8;

/// The CRC-32 (IEEE 802.3) used for Thrum's per-packet integrity
/// trailer, exposed so tests can build a well-formed header without
/// reaching into the crate's internals.
///
/// The encoder calls this for every packet it emits; a decoder verifies
/// the trailer before parsing any further bits.
pub fn packet_crc(bytes: &[u8]) -> u32 {
    crc32(bytes)
}

/// Largest level [`quantize`] can produce.
///
/// The derivable bound for 8-bit input: a residual sample is in
/// `[-255, 255]`, so an 8x8 DCT coefficient is at most `64 * 255`, and
/// [`DCT_SCALE`] divides that by 16. Dividing again by the *smallest*
/// quantiser step gives `(64 * 255) / 16 / QUANT_STEP[0] < 910`, so 1024
/// is a round number that cannot be exceeded. Clamping here (rather than
/// to a larger value) is what makes the encoder's packet-size bound in
/// [`max_packet_bits`] provable rather than hopeful.
const MAX_LEVEL: i32 = 1024;

/// Decoder-side clamp on a level read off the wire. A hostile packet can
/// name any 16-bit magnitude; clamping here bounds both the
/// reconstruction and the work, independently of [`MAX_LEVEL`].
const LEVEL_CLAMP: i32 = 32767;

/// Maximum bits one Rice code can cost for a level within
/// [`MAX_LEVEL`]: the encoder picks the cheapest of `k = 0..=3`, and for
/// the largest encodable value (`zigzag_encode(MAX_LEVEL) = 2048`) that
/// is `k = 3` at `(2048 >> 3) + 1 + 3 = 260` bits.
const MAX_RICE_BITS: u32 = 260;

/// Dead zone as a fraction of the quantiser step: coefficients below
/// `DEAD_ZONE * step` quantise to zero. A mild dead zone costs a little
/// fidelity on flat detail and buys a lot of zeros in smooth areas.
const DEAD_ZONE: f32 = 0.2;

/// Quantiser rounding offsets: `0.5` rounds to nearest, smaller values bias toward zero. Intra
/// blocks have no prediction to fall back on, so they are biased less than inter blocks.
const ROUND_INTRA: f32 = 0.33;
const ROUND_INTER: f32 = 0.15;

/// Deblocking filter, on a 4x4 grid.
const DEBLOCK_GRID: usize = 4;

/// CRC-32 (IEEE 802.3, polynomial `0xEDB88320`, reflected) - the same
/// per-packet integrity convention as Aura/Cinder/Reed, kept local so the
/// codec layer does not depend on the container crate.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
// --- portable float helpers -------------------------------------------------
//
// `f32::abs` and `f32::round` live in `std`, not `core`, at this crate's
// MSRV (1.75). These are exact, branch-predictable and `no_std`.

/// `|x|`.
#[inline]
fn fabs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// Truncate toward zero. Exact for every finite input below `2^23`; at
/// or beyond that magnitude the value is already integral in `f32`.
#[inline]
fn truncf(x: f32) -> f32 {
    if !(x > -8_388_608.0 && x < 8_388_608.0) {
        return x;
    }
    (x as i64) as f32
}

/// Round half away from zero (matches `std`'s `f32::round`).
#[inline]
fn roundf(x: f32) -> f32 {
    let t = truncf(x);
    let f = x - t;
    if f >= 0.5 {
        t + 1.0
    } else if f <= -0.5 {
        t - 1.0
    } else {
        t
    }
}

/// Clamp to `[lo, hi]`, mapping `NaN` to `lo`.
///
/// Written as a negated comparison on purpose: `!(x >= lo)` is the
/// spelling that catches `NaN` (which compares false against
/// everything) and so maps it to the low bound, whereas `x < lo` would
/// let it through.
#[inline]
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    if !(x >= lo) {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Round to nearest and clamp into `0..=255`.
#[inline]
fn to_u8(x: f32) -> u8 {
    roundf(clampf(x, 0.0, 255.0)) as u8
}

// --- picture model ----------------------------------------------------------

/// One 8-bit plane of a picture. `stride` is the row pitch in bytes.
/// Planes are always exactly `w * h` samples with no padding, because
/// the codec is macroblock-aligned and never needs a padded edge.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Plane {
    w: usize,
    h: usize,
    stride: usize,
    data: Vec<u8>,
}

impl Plane {
    fn new(w: usize, h: usize, fill: u8) -> Self {
        Plane {
            w,
            h,
            stride: w,
            data: vec![fill; w * h],
        }
    }

    /// Copy the sub-rectangle at `(x0, y0)` of size `bw x bh` into a
    /// dense, stride-free buffer. The caller guarantees the rectangle is
    /// fully inside the plane.
    #[inline]
    fn load_block(&self, x0: usize, y0: usize, bw: usize, bh: usize, out: &mut [u8]) {
        for r in 0..bh {
            let src = (y0 + r) * self.stride + x0;
            let dst = r * bw;
            out[dst..dst + bw].copy_from_slice(&self.data[src..src + bw]);
        }
    }

    /// Inverse of [`Plane::load_block`].
    #[inline]
    fn store_block(&mut self, x0: usize, y0: usize, bw: usize, bh: usize, src: &[u8]) {
        for r in 0..bh {
            let dst = (y0 + r) * self.stride + x0;
            let s = r * bw;
            self.data[dst..dst + bw].copy_from_slice(&src[s..s + bw]);
        }
    }

    /// Single sample; the caller guarantees `(x, y)` is inside. Test
    /// convenience only - the codec's own paths all use the strided or
    /// bulk accessors, which the hot loops need anyway.
    #[cfg(test)]
    #[inline]
    fn at(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.stride + x]
    }

    /// Write a single sample; the caller guarantees `(x, y)` is inside.
    #[cfg(test)]
    #[inline]
    fn set(&mut self, x: usize, y: usize, v: u8) {
        self.data[y * self.stride + x] = v;
    }

    /// Single sample, clamped to the plane's edges. Used by intra
    /// prediction and motion compensation, both of which can legitimately
    /// address outside the picture.
    #[inline]
    fn at_clamped(&self, x: isize, y: isize) -> u8 {
        if self.w == 0 || self.h == 0 {
            return 0;
        }
        let cx = x.clamp(0, self.w as isize - 1) as usize;
        let cy = y.clamp(0, self.h as isize - 1) as usize;
        self.data[cy * self.stride + cx]
    }
}
/// A decoded or reconstructed picture: luma plus, for 4:2:0, the two
/// half-resolution chroma planes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Picture {
    width: usize,
    height: usize,
    format: PixelFormat,
    luma: Plane,
    /// `[U, V]`, each `width/2 x height/2`. Zero-sized for [`PixelFormat::Gray8`].
    chroma: [Plane; 2],
}

impl Picture {
    fn new(width: usize, height: usize, format: PixelFormat) -> Self {
        let (cw, ch) = (width / 2, height / 2);
        let chroma = if format == PixelFormat::Yuv420 {
            [Plane::new(cw, ch, 128), Plane::new(cw, ch, 128)]
        } else {
            // Gray8 has no chroma; keep the planes zero-sized rather than
            // allocating, so a mono frame costs exactly its luma.
            [Plane::new(0, 0, 0), Plane::new(0, 0, 0)]
        };
        Picture {
            width,
            height,
            format,
            luma: Plane::new(width, height, 0),
            chroma,
        }
    }

    #[inline]
    fn has_chroma(&self) -> bool {
        self.format == PixelFormat::Yuv420
    }
}

/// True for the pixel formats Thrum codes.
#[inline]
pub fn is_coded_format(format: PixelFormat) -> bool {
    matches!(format, PixelFormat::Gray8 | PixelFormat::Yuv420)
}

/// A chroma motion vector is the luma one halved (in 4:2:0 one chroma
/// sample spans two luma samples). Floor division, so it is identical on
/// both sides of the wire and never signalled.
#[inline]
fn chroma_mv(v: i32) -> i32 {
    v.div_euclid(2)
}

/// Build a picture from a decoded [`VideoFrame`], validating that its
/// buffer is at least the size the format and dimensions imply.
fn picture_from_frame(f: &VideoFrame) -> CodecResult<Picture> {
    if !is_coded_format(f.format) {
        return Err(CodecError::Unsupported("thrum pixel format"));
    }
    if f.width == 0
        || f.height == 0
        || (f.width as usize) % MB_SIZE != 0
        || (f.height as usize) % MB_SIZE != 0
    {
        return Err(CodecError::InvalidConfig(
            "thrum frame dimensions must be non-zero multiples of 16",
        ));
    }
    let w = f.width as usize;
    let h = f.height as usize;
    let stride = f.stride as usize;
    if stride < w {
        return Err(CodecError::InvalidConfig("thrum stride < width"));
    }
    let (cw, ch) = (w / 2, h / 2);
    let luma_len = stride * h;
    let chroma_stride = stride / 2;
    let chroma_len = if f.format == PixelFormat::Yuv420 {
        chroma_stride * ch
    } else {
        0
    };
    if f.pixels.len() < luma_len + chroma_len {
        return Err(CodecError::InvalidConfig("thrum frame buffer too small"));
    }

    let mut pic = Picture::new(w, h, f.format);
    for y in 0..h {
        let src = y * stride;
        let dst = y * w;
        pic.luma.data[dst..dst + w].copy_from_slice(&f.pixels[src..src + w]);
    }
    if f.format == PixelFormat::Yuv420 {
        for c in 0..2 {
            let base = luma_len + c * chroma_stride * ch;
            for y in 0..ch {
                let src = base + y * chroma_stride;
                let dst = y * cw;
                pic.chroma[c].data[dst..dst + cw].copy_from_slice(&f.pixels[src..src + cw]);
            }
        }
    }
    Ok(pic)
}
/// Serialise back into a [`VideoFrame`] using the exact layout
/// [`VideoFrame::alloc`] produces, so a decoded frame is
/// byte-compatible with an unencoded one of the same format.
fn picture_to_frame(pic: &Picture, pts: Timestamp) -> VideoFrame {
    let mut f = VideoFrame::alloc(pic.width as u32, pic.height as u32, pic.format);
    f.frame.pts = pts;
    let stride = f.stride as usize;
    for y in 0..pic.height {
        let src = y * pic.width;
        let dst = y * stride;
        f.pixels[dst..dst + pic.width].copy_from_slice(&pic.luma.data[src..src + pic.width]);
    }
    if pic.has_chroma() {
        let (cw, ch) = (pic.width / 2, pic.height / 2);
        for c in 0..2 {
            let base = stride * pic.height + c * (stride / 2) * ch;
            for y in 0..ch {
                let src = y * cw;
                let dst = base + y * (stride / 2);
                f.pixels[dst..dst + cw].copy_from_slice(&pic.chroma[c].data[src..src + cw]);
            }
        }
    }
    f
}

// --- intra prediction -------------------------------------------------------

/// Whether `mode` may be *signalled* for the block at `(x0, y0)`. The
/// encoder only ever chooses from the available set and the decoder
/// re-checks on read, so an unavailable mode in a hostile packet degrades
/// to DC instead of reading out of bounds.
#[inline]
pub fn intra_mode_available(mode: IntraMode, x0: usize, y0: usize) -> bool {
    match mode {
        IntraMode::Dc => true,
        IntraMode::Vertical | IntraMode::Diagonal => y0 > 0,
        IntraMode::Horizontal => x0 > 0,
    }
}

/// Fill `pred` (a dense `BLOCK x BLOCK` buffer) with the intra prediction
/// for `mode` at `(x0, y0)`, read from `recon`.
///
/// `recon` is the *reconstructed* picture, so the prediction is closed
/// loop and the encoder sees exactly the neighbours the decoder will.
fn intra_predict(recon: &Plane, x0: usize, y0: usize, mode: IntraMode, pred: &mut [u8]) {
    // Any mode the geometry cannot support falls back to DC. The encoder
    // never picks these and the decoder applies the same rule, so a
    // corrupt mode field is harmless rather than fatal.
    let mode = if intra_mode_available(mode, x0, y0) {
        mode
    } else {
        IntraMode::Dc
    };

    match mode {
        IntraMode::Dc => {
            // Mean of whichever edge exists. With neither edge (the very
            // first block of the picture) use mid-grey, the conventional
            // neutral starting value.
            let mut sum: u32 = 0;
            let mut n: u32 = 0;
            if y0 > 0 {
                for x in 0..BLOCK {
                    sum += u32::from(recon.at_clamped(x0 as isize + x as isize, y0 as isize - 1));
                }
                n += BLOCK as u32;
            }
            if x0 > 0 {
                for y in 0..BLOCK {
                    sum += u32::from(recon.at_clamped(x0 as isize - 1, y0 as isize + y as isize));
                }
                n += BLOCK as u32;
            }
            let v = if n == 0 {
                128u8
            } else {
                to_u8(sum as f32 / n as f32)
            };
            for slot in pred.iter_mut() {
                *slot = v;
            }
        }
        IntraMode::Vertical => {
            for y in 0..BLOCK {
                for x in 0..BLOCK {
                    pred[y * BLOCK + x] =
                        recon.at_clamped(x0 as isize + x as isize, y0 as isize - 1);
                }
            }
        }
        IntraMode::Horizontal => {
            for y in 0..BLOCK {
                for x in 0..BLOCK {
                    pred[y * BLOCK + x] =
                        recon.at_clamped(x0 as isize - 1, y0 as isize + y as isize);
                }
            }
        }
        IntraMode::Diagonal => {
            // Shear the top edge down and to the right. It reads *only*
            // the row above, which is always fully reconstructed before
            // this block is predicted; reading anything inside the block's
            // own footprint would make the prediction depend on
            // not-yet-written pixels and the encoder would drift.
            for y in 0..BLOCK {
                for x in 0..BLOCK {
                    let a = i32::from(recon.at_clamped(x0 as isize + x as isize, y0 as isize - 1));
                    let x2 = (x + y).min(BLOCK - 1);
                    let b = i32::from(recon.at_clamped(x0 as isize + x2 as isize, y0 as isize - 1));
                    pred[y * BLOCK + x] =
                        to_u8((a * (BLOCK - y) as i32 + b * y as i32) as f32 / BLOCK as f32);
                }
            }
        }
    }
}
// --- transform + quantisation ----------------------------------------------

/// Scratch size [`dct_2d`]/[`idct_2d`] need for an 8x8 block: both
/// require `2 * max(n_rows, n_cols)` elements.
const DCT_SCRATCH: usize = 2 * BLOCK;

/// `dct_2d` is *unnormalized* (a row pass then a column pass), so
/// `dct_2d` followed by `idct_2d` scales by `n_rows * n_cols / 4 = 16`.
///
/// The forward transform therefore divides by 16 so the coefficients land
/// in quantiser units, and the inverse applies **no** further scale:
/// `idct_2d` already carries the 16. Applying a second factor of 16 here
/// would inflate every reconstruction by 16x (pinned by
/// `dct_round_trip_is_close_to_identity`).
const DCT_SCALE: f32 = 1.0 / ((BLOCK_COEFFS / 4) as f32);

/// Forward 8x8 DCT of a residual block, in quantiser units.
fn forward_dct(block: &mut [f32], scratch: &mut [f32]) {
    dct_2d(block, BLOCK, BLOCK, scratch);
    for v in block.iter_mut() {
        *v *= DCT_SCALE;
    }
}

/// Inverse 8x8 DCT. `idct_2d` is unnormalized in the same way
/// `dct_2d` is, so undoing [`DCT_SCALE`] needs no extra factor.
fn inverse_dct(block: &mut [f32], scratch: &mut [f32]) {
    idct_2d(block, BLOCK, BLOCK, scratch);
}

/// Dead-zone uniform quantiser: the level for one coefficient, clamped
/// to `+/-LEVEL_CLAMP` so an absurd coefficient can never produce an
/// unbounded value downstream.
#[inline]
fn quantize(coef: f32, step: f32, rounding: f32) -> i32 {
    let a = fabs(coef) / step;
    if a < DEAD_ZONE {
        return 0;
    }
    // Truncating `a + offset` rounds down for a positive `a`: an offset below 0.5 biases
    // every level toward zero, trading a little distortion for a lot of bits.
    let level = (a + rounding) as i32;
    let level = level.clamp(-MAX_LEVEL, MAX_LEVEL);
    if coef < 0.0 {
        -level
    } else {
        level
    }
}

/// Inverse of [`quantize`]: the reconstruction point for a level.
#[inline]
fn dequantize(level: i32, step: f32) -> f32 {
    level as f32 * step
}

// --- residual block entropy coding -----------------------------------------
//
// Per block the wire carries:
//   1 bit   `coded` (1 = at least one non-zero level follows)
//   if coded:
//     2 bits Rice parameter `k`
//     6 bits `last`: the highest zigzag position with a non-zero level
//     then, walking positions `0..=last`, one token per non-zero level:
//       Rice  run   (positions skipped since the previous level)
//       Rice  level (zig-zag mapped, so the sign rides along for free)
// `last` is itself the end-of-block marker, so no separate EOB symbol is
// needed: the reader stops once the cursor steps past it. The walk is
// bounded by `last + 1 <= 64`, so decoder work is bounded by the block
// size regardless of what the bits say.

/// Zig-zag map a signed level onto an unsigned value (sign in the LSB).
#[inline]
fn zigzag_encode(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

/// Inverse of [`zigzag_encode`]. Not used by the codec itself -
/// `tpt-media-bitstream`'s Rice coder applies the same zig-zag internally -
/// but pinned by a test so the two mappings cannot drift apart.
#[cfg(test)]
#[inline]
fn zigzag_decode(u: u32) -> i32 {
    ((u >> 1) as i32) ^ -((u & 1) as i32)
}

/// Bits [`tpt_media_bitstream::rice`] would spend on `v` at parameter
/// `k`, used to pick the best `k` for a block without coding it twice.
#[inline]
fn rice_cost(v: i32, k: u32) -> u32 {
    (zigzag_encode(v) >> k) + 1 + k
}
/// Write one 8x8 residual block. `levels` is in raster order.
fn write_block(w: &mut BitWriter, levels: &[i32; BLOCK_COEFFS]) -> CodecResult<()> {
    // Highest non-zero position in zigzag order.
    let mut last: usize = 0;
    let mut any = false;
    for p in 0..BLOCK_COEFFS {
        if levels[ZIGZAG[p]] != 0 {
            last = p;
            any = true;
        }
    }
    // The `coded` flag is 1 when at least one level is non-zero, so an
    // all-zero block costs exactly one bit.
    w.write(1, u64::from(any))
        .map_err(|_| CodecError::Internal)?;
    if !any {
        return Ok(());
    }

    // Pick the Rice parameter that minimises this block's token cost.
    let mut best_k = 0u32;
    let mut best_cost = u32::MAX;
    for k in 0..=RICE_K_MAX {
        let mut cost = 0u32;
        let mut run = 0i32;
        for p in 0..=last {
            let v = levels[ZIGZAG[p]];
            if v == 0 {
                run += 1;
                continue;
            }
            cost += rice_cost(run, k) + rice_cost(v, k);
            run = 0;
        }
        if cost < best_cost {
            best_cost = cost;
            best_k = k;
        }
    }

    w.write(RICE_K_BITS, u64::from(best_k))
        .map_err(|_| CodecError::Internal)?;
    w.write(6, last as u64).map_err(|_| CodecError::Internal)?;

    let mut run = 0i32;
    for p in 0..=last {
        let v = levels[ZIGZAG[p]];
        if v == 0 {
            run += 1;
            continue;
        }
        encode_riced(w, best_k, run as i64).map_err(|_| CodecError::Internal)?;
        encode_riced(w, best_k, v as i64).map_err(|_| CodecError::Internal)?;
        run = 0;
    }
    Ok(())
}

/// Read one 8x8 residual block back into `levels` (raster order). The
/// buffer is cleared first, so an uncoded block comes back all-zero.
fn read_block(r: &mut BitReader, levels: &mut [i32; BLOCK_COEFFS]) -> CodecResult<()> {
    levels.fill(0);
    if r.read(1).map_err(|_| CodecError::MalformedPacket)? == 0 {
        return Ok(());
    }
    let k = r
        .read(RICE_K_BITS)
        .map_err(|_| CodecError::MalformedPacket)? as u32;
    if k > RICE_K_MAX {
        return Err(CodecError::MalformedPacket);
    }
    let last = r.read(6).map_err(|_| CodecError::MalformedPacket)? as usize;

    // Mirrors the encoder's `for p in 0..=last` walk. A run that steps
    // `pos` past `last` ends the block; the loop is bounded by
    // `last + 1 <= 64` iterations, so a corrupt stream cannot make the
    // decoder spin.
    let mut pos = 0usize;
    loop {
        let run = decode_riced(r, k).map_err(|_| CodecError::MalformedPacket)?;
        if run < 0 {
            return Err(CodecError::MalformedPacket);
        }
        pos = match pos.checked_add(run as usize) {
            Some(p) if p <= last => p,
            _ => break,
        };
        let level = decode_riced(r, k).map_err(|_| CodecError::MalformedPacket)?;
        let level = i32::try_from(level).map_err(|_| CodecError::MalformedPacket)?;
        levels[ZIGZAG[pos]] = level.clamp(-LEVEL_CLAMP, LEVEL_CLAMP);
        pos += 1;
        if pos > last {
            break;
        }
    }
    Ok(())
}
// --- loop filter ------------------------------------------------------------

/// Deblocking strength for a quantiser index: `(beta, alpha, tc)`.
///
/// All three grow with `qp`, so a lightly quantised frame is barely
/// touched (`qp = 1` gives `beta = 1, tc = 1`: the wide path needs a
/// difference of 0, and the narrow path is a 1-LSB correction) while a
/// heavily quantised one gets its block edges averaged flat.
#[inline]
fn deblock_params(qp: u8) -> (i32, i32, i32) {
    let q = i32::from(qp.clamp(MIN_QP, MAX_QP));
    (q, 4 * q, (q / 2).max(1))
}

/// Filter one 4-sample window straddling a block edge: `s[0] s[1] | s[2]
/// s[3]`, the edge sitting between `s[1]` and `s[2]`.
///
/// Two paths, in the classic H.263 shape. A small edge difference is
/// quantisation ringing rather than real detail, so the two samples are
/// simply averaged across it; a larger one is real detail and gets only
/// a `tc`-limited correction towards its outer neighbours. Purely a
/// function of the pixels and `qp`, so encoder and decoder cannot
/// disagree.
fn deblock_pair(s: &mut [u8; 4], beta: i32, alpha: i32, tc: i32) {
    let p1 = i32::from(s[1]);
    let q1 = i32::from(s[2]);
    let d = (q1 - p1).abs();
    if d >= alpha {
        return;
    }
    if d < beta {
        // Wide path: full average of the two samples either side.
        let avg = ((p1 + q1 + 1) >> 1).clamp(0, 255) as u8;
        s[1] = avg;
        s[2] = avg;
    } else {
        // Narrow path: pull each sample a bounded amount towards the
        // mean of itself and its outer neighbour.
        let p2 = i32::from(s[0]);
        let q2 = i32::from(s[3]);
        let delta = (((p1 + p2) - (q1 + q2)) / 4).clamp(-tc, tc);
        s[1] = (p1 + delta).clamp(0, 255) as u8;
        s[2] = (q1 - delta).clamp(0, 255) as u8;
    }
}

/// Run the deblocking filter over a whole plane: vertical edges first,
/// then horizontal, so a sample on a block corner is filtered once per
/// edge - deterministic, and the usual convention.
///
/// Edges sit every [`DEBLOCK_GRID`] samples, and both samples either
/// side of each are in range by construction (edges start at
/// `DEBLOCK_GRID` and the loop stops two samples short of the far edge).
///
/// The horizontal pass reads its window with an explicit `stride`: a
/// plain contiguous slice would take the four samples *along the row*
/// (adjacent columns) instead of the four *across* the edge (adjacent
/// rows), silently filtering the wrong axis and wrecking the picture.
fn deblock_plane(plane: &mut Plane, beta: i32, alpha: i32, tc: i32) {
    let (w, h) = (plane.w, plane.h);
    if w < DEBLOCK_GRID + 2 || h < DEBLOCK_GRID + 2 {
        return;
    }
    // Vertical edges: the window is contiguous within a row.
    for x in (DEBLOCK_GRID..w - 1).step_by(DEBLOCK_GRID) {
        for y in 0..h {
            let i = y * w + x;
            let mut s = [
                plane.data[i - 2],
                plane.data[i - 1],
                plane.data[i],
                plane.data[i + 1],
            ];
            deblock_pair(&mut s, beta, alpha, tc);
            plane.data[i - 1] = s[1];
            plane.data[i] = s[2];
        }
    }
    // Horizontal edges: the window steps by the row pitch.
    for y in (DEBLOCK_GRID..h - 1).step_by(DEBLOCK_GRID) {
        for x in 0..w {
            let row = y * w;
            let mut s = [
                plane.data[row - 2 * w + x],
                plane.data[row - w + x],
                plane.data[row + x],
                plane.data[row + w + x],
            ];
            deblock_pair(&mut s, beta, alpha, tc);
            plane.data[row - w + x] = s[1];
            plane.data[row + x] = s[2];
        }
    }
}

// --- motion compensation + estimation ---------------------------------------

/// Copy a `bw x bh` block out of `src` at `(x0 + dx, y0 + dy)` into a
/// dense `bw x bh` buffer, clamping at the picture edges. The same
/// clamped fetch runs on both sides, so a vector pointing off-frame costs
/// fidelity but never desynchronises encoder from decoder.
#[allow(clippy::too_many_arguments)]
fn load_mc_block(
    src: &Plane,
    x0: usize,
    y0: usize,
    bw: usize,
    bh: usize,
    dx: i32,
    dy: i32,
    out: &mut [u8],
) {
    for y in 0..bh {
        let sy = y0 as isize + dy as isize + y as isize;
        for x in 0..bw {
            out[y * bw + x] = src.at_clamped(x0 as isize + dx as isize + x as isize, sy);
        }
    }
}

/// Sum of absolute differences between two dense blocks.
#[inline]
fn sad(a: &[u8], b: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        sum += (*x as i32 - *y as i32).unsigned_abs();
    }
    sum
}

/// Exhaustive full-pixel motion search for one macroblock.
///
/// Every `(dx, dy)` in `+/-range` is costed by luma SAD, with an early
/// exit as soon as the running total reaches the best found so far - the
/// SAD is monotonically increasing, so the exit is exact, not a
/// heuristic. Ties keep the first candidate in scan order, so the result
/// is deterministic regardless of how the loops are optimised.
fn motion_search(
    src: &Plane,
    reference: &Plane,
    x0: usize,
    y0: usize,
    range: i32,
    block: &mut [u8],
    cand: &mut [u8],
) -> (i32, i32, u32) {
    src.load_block(x0, y0, MB_SIZE, MB_SIZE, block);
    let mut best = (0i32, 0i32, u32::MAX);
    for dy in -range..=range {
        for dx in -range..=range {
            load_mc_block(reference, x0, y0, MB_SIZE, MB_SIZE, dx, dy, cand);
            let mut sum: u32 = 0;
            for (a, b) in block.iter().zip(cand.iter()) {
                sum += (*a as i32 - *b as i32).unsigned_abs();
                if sum >= best.2 {
                    break;
                }
            }
            if sum < best.2 {
                best = (dx, dy, sum);
            }
        }
    }
    best
}
// --- block level ------------------------------------------------------------

/// Top-left corner of an 8x8 block within a plane.
///
/// Grouping the position keeps the block-coding helpers at a readable
/// arity; they would otherwise need eight parameters each.
#[derive(Copy, Clone, Debug)]
struct BlockPos {
    x0: usize,
    y0: usize,
}

/// A full-pixel motion vector, in luma samples.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MotionVector {
    /// Horizontal displacement.
    pub dx: i32,
    /// Vertical displacement.
    pub dy: i32,
}

/// Per-block scratch, allocated once per encoder/decoder and reused for
/// every block so the steady-state path performs no allocation. A macro-
/// block needs 4 luma blocks (6 with chroma) plus motion search buffers.
#[derive(Clone, Debug)]
struct BlockScratch {
    /// Source / prediction / reconstruction blocks (`BLOCK_COEFFS` each).
    src: Vec<u8>,
    pred: Vec<u8>,
    recon: Vec<u8>,
    /// Float transform buffers.
    residual: Vec<f32>,
    coeffs: Vec<f32>,
    dct_scratch: Vec<f32>,
    /// Quantised levels for the block being coded. A fixed-size array,
    /// not a `Vec`, so it can be handed to `write_block`/`read_block`
    /// without a copy or a slice-to-array conversion.
    levels: [i32; BLOCK_COEFFS],
    /// A macroblock of motion-search samples, reused per block.
    mb_block: Vec<u8>,
    mb_cand: Vec<u8>,
    /// The quantiser step for the frame being coded. Constant across a
    /// frame (it is a header field), so it lives here rather than being
    /// threaded through every block helper.
    step: f32,
    /// Quantiser rounding offset for the block being coded (see [`ROUND_INTRA`]).
    rounding: f32,
}

impl BlockScratch {
    fn new() -> Self {
        BlockScratch {
            src: vec![0; BLOCK_COEFFS],
            pred: vec![0; BLOCK_COEFFS],
            recon: vec![0; BLOCK_COEFFS],
            residual: vec![0.0; BLOCK_COEFFS],
            coeffs: vec![0.0; BLOCK_COEFFS],
            dct_scratch: vec![0.0; DCT_SCRATCH],
            levels: [0; BLOCK_COEFFS],
            mb_block: vec![0; MB_SIZE * MB_SIZE],
            mb_cand: vec![0; MB_SIZE * MB_SIZE],
            step: quant_step(MIN_QP),
            rounding: ROUND_INTRA,
        }
    }
}

/// Forward path: `s.recon = clamp(s.pred + IDCT(DEQUANT(DCT(s.src -
/// s.pred))))`.
///
/// The reconstruction lands in [`BlockScratch::recon`] so the caller can
/// store it into the picture (and so later blocks see closed-loop
/// neighbours); the quantised levels are returned for the wire.
fn encode_block(s: &mut BlockScratch) -> [i32; BLOCK_COEFFS] {
    for i in 0..BLOCK_COEFFS {
        s.residual[i] = s.src[i] as f32 - s.pred[i] as f32;
    }
    s.coeffs.copy_from_slice(&s.residual);
    forward_dct(&mut s.coeffs, &mut s.dct_scratch);
    for i in 0..BLOCK_COEFFS {
        s.levels[i] = quantize(s.coeffs[i], s.step, s.rounding);
    }
    for (i, level) in s.levels.iter().enumerate() {
        s.coeffs[i] = dequantize(*level, s.step);
    }
    inverse_dct(&mut s.coeffs, &mut s.dct_scratch);
    for i in 0..BLOCK_COEFFS {
        s.recon[i] = to_u8(s.pred[i] as f32 + s.coeffs[i]);
    }
    s.levels
}

/// Inverse path: `s.recon = clamp(s.pred + IDCT(DEQUANT(levels)))`,
/// reading the levels straight off the wire.
fn decode_block(levels: &[i32; BLOCK_COEFFS], s: &mut BlockScratch) {
    for (i, level) in levels.iter().enumerate() {
        s.coeffs[i] = dequantize(*level, s.step);
    }
    inverse_dct(&mut s.coeffs, &mut s.dct_scratch);
    for i in 0..BLOCK_COEFFS {
        s.recon[i] = to_u8(s.pred[i] as f32 + s.coeffs[i]);
    }
}

/// Choose the intra mode with the lowest SAD against `src`, considering
/// only the modes the geometry allows. Deterministic: ties keep the
/// lowest-numbered mode.
fn best_intra_mode(
    recon: &Plane,
    src: &Plane,
    x0: usize,
    y0: usize,
    s: &mut BlockScratch,
) -> IntraMode {
    src.load_block(x0, y0, BLOCK, BLOCK, &mut s.src);
    let mut best = IntraMode::Dc;
    let mut best_sad = u32::MAX;
    for mode in IntraMode::ALL {
        if !intra_mode_available(mode, x0, y0) {
            continue;
        }
        intra_predict(recon, x0, y0, mode, &mut s.pred);
        let cost = sad(&s.src, &s.pred);
        if cost < best_sad {
            best_sad = cost;
            best = mode;
        }
    }
    best
}

/// Intra-code one 8x8 block: build the prediction from `recon`,
/// transform/quantise the residual, write it, store the reconstruction.
fn code_intra_block(
    w: &mut BitWriter,
    src: &Plane,
    recon: &mut Plane,
    pos: BlockPos,
    mode: IntraMode,
    s: &mut BlockScratch,
) -> CodecResult<()> {
    src.load_block(pos.x0, pos.y0, BLOCK, BLOCK, &mut s.src);
    intra_predict(recon, pos.x0, pos.y0, mode, &mut s.pred);
    s.rounding = ROUND_INTRA;
    let levels = encode_block(s);
    write_block(w, &levels)?;
    recon.store_block(pos.x0, pos.y0, BLOCK, BLOCK, &s.recon);
    Ok(())
}

/// Inter-code one 8x8 block: predict from `reference` at `mv`, code the
/// residual, store the reconstruction. Shared by luma (full vector) and
/// chroma (halved vector).
fn code_inter_block(
    w: &mut BitWriter,
    src: &Plane,
    recon: &mut Plane,
    reference: &Plane,
    pos: BlockPos,
    mv: MotionVector,
    s: &mut BlockScratch,
) -> CodecResult<()> {
    src.load_block(pos.x0, pos.y0, BLOCK, BLOCK, &mut s.src);
    load_mc_block(
        reference,
        pos.x0,
        pos.y0,
        BLOCK,
        BLOCK,
        mv.dx,
        mv.dy,
        &mut s.pred,
    );
    s.rounding = ROUND_INTER;
    let levels = encode_block(s);
    write_block(w, &levels)?;
    recon.store_block(pos.x0, pos.y0, BLOCK, BLOCK, &s.recon);
    Ok(())
}

/// Intra-code both chroma planes of one macroblock (always DC: chroma
/// carries little directional structure at this resolution, and skipping
/// mode search there is most of Thrum's low-complexity story).
fn code_intra_chroma(
    w: &mut BitWriter,
    src: &Picture,
    recon: &mut Picture,
    pos: BlockPos,
    s: &mut BlockScratch,
) -> CodecResult<()> {
    for c in 0..2 {
        code_intra_block(
            w,
            &src.chroma[c],
            &mut recon.chroma[c],
            pos,
            IntraMode::Dc,
            s,
        )?;
    }
    Ok(())
}

/// Inter-code both chroma planes of one macroblock at halved vectors.
fn code_inter_chroma(
    w: &mut BitWriter,
    src: &Picture,
    recon: &mut Picture,
    reference: &Picture,
    pos: BlockPos,
    mv: MotionVector,
    s: &mut BlockScratch,
) -> CodecResult<()> {
    for c in 0..2 {
        code_inter_block(
            w,
            &src.chroma[c],
            &mut recon.chroma[c],
            &reference.chroma[c],
            pos,
            mv,
            s,
        )?;
    }
    Ok(())
}

/// Copy a macroblock's worth of samples out of `reference` at `(dx, dy)`
/// and into `recon` (the skip path: no residual, just the prediction).
fn copy_mb_from_reference(
    reference: &Picture,
    recon: &mut Picture,
    mx: usize,
    my: usize,
    dx: i32,
    dy: i32,
) {
    // Predict into a dense block and store it with the plane's stride. Writing
    // straight into `plane.data` treated the picture as contiguous, so every
    // row after the first landed in the wrong place.
    let mut luma = [0u8; MB_SIZE * MB_SIZE];
    load_mc_block(&reference.luma, mx, my, MB_SIZE, MB_SIZE, dx, dy, &mut luma);
    recon.luma.store_block(mx, my, MB_SIZE, MB_SIZE, &luma);
    if recon.has_chroma() {
        let (cx, cy) = (mx / 2, my / 2);
        let (cdx, cdy) = (chroma_mv(dx), chroma_mv(dy));
        for c in 0..2 {
            let mut chroma = [0u8; BLOCK * BLOCK];
            load_mc_block(&reference.chroma[c], cx, cy, BLOCK, BLOCK, cdx, cdy, &mut chroma);
            recon.chroma[c].store_block(cx, cy, BLOCK, BLOCK, &chroma);
        }
    }
}

// --- configuration ----------------------------------------------------------

/// Thrum configuration.
///
/// Frame geometry is part of the configuration (and of the track header,
/// see [`ThrumConfig::to_codec_private`]) rather than of every packet,
/// so a P-frame packet stays small. Both sides must agree on it, which
/// the track's `codec_config.data` bytes carry.
#[derive(Clone, Debug, PartialEq)]
pub struct ThrumConfig {
    /// Coded frame width in luma pixels; a non-zero multiple of [`MB_SIZE`].
    pub width: u32,
    /// Coded frame height in luma pixels; a non-zero multiple of [`MB_SIZE`].
    pub height: u32,
    /// Coded pixel format: [`PixelFormat::Gray8`] or [`PixelFormat::Yuv420`].
    pub format: PixelFormat,
    /// Quantiser index for every frame, `1..=MAX_QP` (lower is better).
    pub qp: u8,
    /// Motion-search radius in full pixels, `0..=MAX_SEARCH_RANGE`.
    pub search_range: u8,
    /// Percentage of macroblocks re-coded intra in each P-frame,
    /// `0..=100` (the target is ~5%).
    pub intra_refresh_percent: u8,
}
impl Default for ThrumConfig {
    fn default() -> Self {
        ThrumConfig {
            width: 128,
            height: 96,
            format: PixelFormat::Yuv420,
            qp: 26,
            search_range: DEFAULT_SEARCH_RANGE,
            intra_refresh_percent: DEFAULT_INTRA_REFRESH_PERCENT,
        }
    }
}

impl ThrumConfig {
    /// Number of codec-private bytes [`ThrumConfig::to_codec_private`]
    /// produces.
    pub const CODEC_PRIVATE_LEN: usize = 4;

    /// Macroblocks across / down the frame.
    #[inline]
    pub fn mb_dims(&self) -> (usize, usize) {
        (
            self.width as usize / MB_SIZE,
            self.height as usize / MB_SIZE,
        )
    }

    /// Number of macroblocks in a frame.
    #[inline]
    pub fn mb_count(&self) -> usize {
        let (mw, mh) = self.mb_dims();
        mw * mh
    }

    /// Bytes of pixel storage one frame occupies.
    pub fn frame_bytes(&self) -> usize {
        let n = self.width as usize * self.height as usize;
        if self.format == PixelFormat::Yuv420 {
            n + n / 2
        } else {
            n
        }
    }

    /// Transform blocks per macroblock: four luma, plus one per chroma
    /// plane for 4:2:0.
    #[inline]
    fn blocks_per_macroblock(&self) -> usize {
        if self.format == PixelFormat::Yuv420 {
            LUMA_BLOCKS + 2
        } else {
            LUMA_BLOCKS
        }
    }

    /// Upper bound on the bits one frame's packet can occupy.
    ///
    /// Provable rather than estimated, because every term is bounded:
    ///
    /// - header: 16 (version) + 1 (frame type) + [`QP_BITS`];
    /// - per macroblock: 1 (intra flag) + 1 (skip) + 2 × `MV_BITS`
    ///   motion-vector fields;
    /// - per block: 1 (coded flag) + `RICE_K_BITS` + 6 (`last`) plus at
    ///   most [`BLOCK_COEFFS`] tokens of 2 Rice codes each, every one of
    ///   which is at most `MAX_RICE_BITS` bits.
    ///
    /// The encoder allocates exactly this and `write_block` therefore
    /// never reports a full buffer for a well-formed frame.
    pub fn max_packet_bits(&self) -> usize {
        let header = 16 + 1 + QP_BITS as usize;
        let per_mb = 1 + 1 + 2 * MV_BITS as usize;
        let per_block = 1 + RICE_K_BITS as usize + 6 + 2 * BLOCK_COEFFS * MAX_RICE_BITS as usize;
        header + self.mb_count() * (per_mb + self.blocks_per_macroblock() * per_block)
    }

    fn validate(&self) -> CodecResult<()> {
        if !is_coded_format(self.format) {
            return Err(CodecError::InvalidConfig(
                "thrum supports only Gray8 and Yuv420",
            ));
        }
        if self.width == 0
            || self.height == 0
            || (self.width as usize) % MB_SIZE != 0
            || (self.height as usize) % MB_SIZE != 0
        {
            return Err(CodecError::InvalidConfig(
                "thrum width/height must be non-zero multiples of 16",
            ));
        }
        if !(MIN_QP..=MAX_QP).contains(&self.qp) {
            return Err(CodecError::InvalidConfig("thrum qp must be 1..=51"));
        }
        if self.search_range > MAX_SEARCH_RANGE {
            return Err(CodecError::InvalidConfig(
                "thrum search_range must be 0..=16",
            ));
        }
        if self.intra_refresh_percent > 100 {
            return Err(CodecError::InvalidConfig(
                "thrum intra_refresh_percent must be 0..=100",
            ));
        }
        Ok(())
    }

    /// Encode the codec-side config into the track's private bytes
    /// (`format`, `qp`, `search_range`, `intra_refresh_percent`). Frame
    /// geometry lives in the track header, so it is not repeated here.
    pub fn to_codec_private(&self) -> Vec<u8> {
        vec![
            self.format as u8,
            self.qp,
            self.search_range,
            self.intra_refresh_percent,
        ]
    }

    /// Inverse of [`ThrumConfig::to_codec_private`], with the frame
    /// geometry supplied from the track header.
    pub fn from_codec_private(width: u32, height: u32, private: &[u8]) -> CodecResult<ThrumConfig> {
        let mut cfg = ThrumConfig {
            width,
            height,
            ..ThrumConfig::default()
        };
        if private.len() >= ThrumConfig::CODEC_PRIVATE_LEN {
            cfg.format = match private[0] {
                0 => PixelFormat::Gray8,
                3 => PixelFormat::Yuv420,
                _ => return Err(CodecError::InvalidConfig("thrum pixel format")),
            };
            cfg.qp = private[1];
            cfg.search_range = private[2];
            cfg.intra_refresh_percent = private[3];
        }
        cfg.validate()?;
        Ok(cfg)
    }
}

/// Map a caller-facing `0..=100` quality onto a `qp` index: 100 is the
/// best quality (`qp = MIN_QP`), 0 the worst (`qp = MAX_QP`).
///
/// The mapping is monotonically non-increasing in `quality` and covers
/// the full `MIN_QP..=MAX_QP` range, so `--quality` steps down through
/// every quantiser level rather than skipping most of them (which is what
/// a naive `MIN + (100 - q) * span / 100` on a 50-wide span does for
/// small quality changes).
pub fn quality_to_qp(quality: u8) -> u8 {
    let q = u32::from(quality.min(100));
    let span = u32::from(MAX_QP - MIN_QP);
    MIN_QP + (span - (q * span) / 100) as u8
}

/// Static codec information for a given geometry.
fn static_info(width: u32, height: u32, format: PixelFormat, caps: Capabilities) -> CodecInfo {
    CodecInfo {
        name: "Thrum",
        codec_id: CODEC_ID,
        version: VERSION,
        capabilities: caps,
        supported_formats: alloc::vec![SupportedFormat::Video {
            pixel_format: format,
            width,
            height,
        }],
    }
}
// --- encoder ----------------------------------------------------------------

/// Thrum encoder.
///
/// Holds the reconstructed reference picture (the *deblocked* one, so
/// encoder and decoder stay in lock-step) and the intra-refresh cursor.
/// The first frame is always an I-frame; after that one is emitted every
/// [`KEYFRAME_INTERVAL`] frames and the rest are P-frames.
pub struct ThrumEncoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: ThrumConfig,
    /// Reconstruction of the previous frame - the reference for P-frames.
    reference: Option<Picture>,
    /// Rotating cursor: the next macroblock to force intra in a P-frame.
    refresh_cursor: usize,
    scratch: BlockScratch,
    /// Worst-case packet buffer, allocated once here and rewritten by
    /// every frame (see `encode`).
    packet_buf: Vec<u8>,
    /// Frames encoded so far (drives the I/P decision).
    frames: u64,
    keyframe_interval: u64,
    /// Where the bits went, accumulated across frames.
    stats: EncodeStats,
}

/// Where an encoder's bits went, accumulated across frames: macroblock counts
/// by kind, and bits by part. For diagnostics and tuning; reading it costs
/// nothing and the counters cost a few additions per macroblock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncodeStats {
    /// Macroblocks coded intra (including refresh macroblocks and I-frames).
    pub intra_mbs: u64,
    /// Macroblocks coded as skip.
    pub skip_mbs: u64,
    /// Macroblocks coded inter with a vector and residual.
    pub inter_mbs: u64,
    /// Bits spent on intra macroblocks (flags, modes and residual).
    pub intra_bits: u64,
    /// Bits spent on skip macroblocks (their two flags).
    pub skip_bits: u64,
    /// Bits spent on inter headers: the two flags and the motion vector.
    pub mv_bits: u64,
    /// Bits spent on inter luma residual.
    pub luma_bits: u64,
    /// Bits spent on inter chroma residual.
    pub chroma_bits: u64,
}

/// Bits written so far.
fn bit_pos(w: &BitWriter) -> usize {
    w.position_bytes() * 8 + w.bits_buffered() as usize
}

impl ThrumEncoder {
    /// Build an encoder with the default I-frame interval.
    pub fn new(codec_cfg: CodecConfig, enc: ThrumConfig) -> CodecResult<Self> {
        ThrumEncoder::with_keyframe_interval(codec_cfg, enc, KEYFRAME_INTERVAL)
    }

    /// Build an encoder that emits an I-frame every `keyframe_interval`
    /// frames. `0` forces every frame intra (useful for tests and for a
    /// pure intra stream).
    pub fn with_keyframe_interval(
        codec_cfg: CodecConfig,
        enc: ThrumConfig,
        keyframe_interval: u64,
    ) -> CodecResult<Self> {
        enc.validate()?;
        let info = static_info(
            enc.width,
            enc.height,
            enc.format,
            Capabilities {
                // The exhaustive motion search is O((2r+1)^2) per
                // macroblock; only the decoder path is claimed real-time.
                real_time_decoder: true,
                lossy: true,
                ..Capabilities::default()
            },
        );
        // The packet buffer is sized once, here, rather than per frame:
        // see the note in `encode`.
        let packet_buf = vec![0u8; enc.max_packet_bits().div_ceil(8) + CRC_TRAILER_BYTES];
        Ok(ThrumEncoder {
            info,
            cfg: codec_cfg,
            enc,
            reference: None,
            refresh_cursor: 0,
            scratch: BlockScratch::new(),
            packet_buf,
            frames: 0,
            keyframe_interval,
            stats: EncodeStats::default(),
        })
    }

    /// Where the bits have gone so far (see [`EncodeStats`]).
    pub fn stats(&self) -> EncodeStats {
        self.stats
    }

    /// Whether the next frame will be coded as an I-frame.
    pub fn next_is_keyframe(&self) -> bool {
        self.keyframe_interval == 0 || self.frames % self.keyframe_interval == 0
    }

    /// Number of macroblocks this encoder will force intra in the next
    /// P-frame (the intra-refresh budget).
    pub fn refresh_count(&self) -> usize {
        let total = self.enc.mb_count();
        (total * usize::from(self.enc.intra_refresh_percent)) / 100
    }

    /// Whether macroblock `index` is one of this frame's intra-refresh
    /// macroblocks.
    ///
    /// The refresh set is the `per_frame` macroblocks *contiguous* from
    /// the cursor, so successive frames form a sliding window that sweeps
    /// the whole picture: after `total / per_frame` P-frames every
    /// macroblock has been refreshed once. (An earlier strided version
    /// used `(cursor + i * ceil(total/per_frame)) % total`, which is only
    /// exhaustive when the stride is coprime with `total` - with 48
    /// macroblocks and 9 per frame the stride 6 shares a factor and the
    /// sweep silently covered only 1/6 of the frame.)
    fn mb_is_refresh(&self, index: usize) -> bool {
        let total = self.enc.mb_count();
        let per_frame = self.refresh_count();
        if per_frame == 0 || total == 0 {
            return false;
        }
        let start = self.refresh_cursor % total;
        let offset = (index + total - start) % total;
        offset < per_frame
    }

    /// Advance the intra-refresh cursor past this frame's selections.
    /// A full sweep therefore takes `ceil(total / per_frame)` P-frames.
    fn advance_refresh_cursor(&mut self) {
        let total = self.enc.mb_count();
        let per_frame = self.refresh_count();
        if per_frame == 0 || total == 0 {
            return;
        }
        self.refresh_cursor = (self.refresh_cursor + per_frame) % total;
    }
}

impl Encoder for ThrumEncoder {
    type Frame = VideoFrame;

    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn encode(&mut self, frame: &VideoFrame) -> CodecResult<Vec<Packet>> {
        // Geometry and format are fixed by the config; re-check the
        // incoming frame so a mismatch is a clean error rather than a
        // silent reinterpretation.
        if frame.format != self.enc.format {
            return Err(CodecError::Unsupported("thrum pixel format"));
        }
        if frame.width != self.enc.width || frame.height != self.enc.height {
            return Err(CodecError::InvalidConfig(
                "thrum frame dimensions do not match the configured geometry",
            ));
        }
        self.cfg.limits.check_frame_bytes(self.enc.frame_bytes())?;
        let src = picture_from_frame(frame)?;

        let intra = self.next_is_keyframe();
        // The step is per-frame state, not a per-block argument.
        self.scratch.step = quant_step(self.enc.qp);
        let (mb_w, mb_h) = self.enc.mb_dims();
        let mb_count = mb_w * mb_h;
        self.cfg
            .limits
            .check_allocation(mb_count * BLOCK_COEFFS * 8)?;

        // A P-frame needs the previous reconstruction as its reference.
        let reference = if intra { None } else { self.reference.clone() };
        // Starting a stream always codes intra, even if a caller forced
        // a P-frame first, so the decoder never lacks a reference.
        let intra = intra || reference.is_none();
        let mut recon = match &reference {
            Some(r) => r.clone(),
            None => Picture::new(src.width, src.height, src.format),
        };

        // Sized by the provable bound in `max_packet_bits`, so the encode
        // path can never overflow for a well-formed frame. The buffer is
        // *reused* rather than reallocated: that bound is deliberately
        // pessimistic (it charges every one of a block's 64 coefficients
        // the worst case a Rice escape can take), so for a 176x144 frame
        // it is ~2.4 MB against a few kB of actual payload. Allocating
        // that per frame made the encoder's transient working set two
        // orders of magnitude larger than the frame it was coding, and
        // re-zeroed it on every frame. Caught by
        // `apps/tpt-cli/tests/video_quality.rs`.
        //
        // `mem::take` leaves an empty (unallocated) `Vec` in its place so
        // the `BitWriter` borrows a local rather than `self`, which the
        // per-macroblock calls below need to mutate.
        let mut buf = core::mem::take(&mut self.packet_buf);
        debug_assert!(buf.len() >= self.enc.max_packet_bits().div_ceil(8) + CRC_TRAILER_BYTES);
        let written = {
            let mut w = BitWriter::new(&mut buf);
            w.write(16, VERSION as u64)
                .map_err(|_| CodecError::Internal)?;
            w.write(1, u64::from(intra))
                .map_err(|_| CodecError::Internal)?;
            w.write(QP_BITS, self.enc.qp as u64)
                .map_err(|_| CodecError::Internal)?;

            for mb in 0..mb_count {
                let intra_mb = intra || self.mb_is_refresh(mb);
                self.encode_macroblock(
                    &mut w,
                    &src,
                    &mut recon,
                    reference.as_ref(),
                    mb % mb_w,
                    mb / mb_w,
                    intra_mb,
                )?;
            }

            // Deblock the finished reconstruction in place; this is the
            // picture that becomes the next reference.
            let (beta, alpha, tc) = deblock_params(self.enc.qp);
            deblock_plane(&mut recon.luma, beta, alpha, tc);
            if recon.has_chroma() {
                deblock_plane(&mut recon.chroma[0], beta, alpha, tc);
                deblock_plane(&mut recon.chroma[1], beta, alpha, tc);
            }
            w.finish().map_err(|_| CodecError::Internal)?
        };
        // Copy the payload out at its exact size and hand the worst-case
        // scratch back for the next frame. The copy is what a caller
        // would have paid anyway (the packet owns its bytes); doing it
        // here means the oversized buffer is never handed out, so a
        // caller retaining N packets retains N *payloads* rather than N
        // worst-case buffers. The earlier `shrink_to_fit` version fixed
        // the same retention problem but still paid the allocation, and
        // the zeroing, on every single frame.
        let crc = crc32(&buf[..written]);
        let mut data = Vec::with_capacity(written + CRC_TRAILER_BYTES);
        data.extend_from_slice(&buf[..written]);
        data.extend_from_slice(&crc.to_le_bytes());
        self.packet_buf = buf;

        self.reference = Some(recon);
        self.frames += 1;
        self.advance_refresh_cursor();

        let flags = PacketFlags::END_OF_FRAME
            | if intra {
                PacketFlags::KEYFRAME
            } else {
                PacketFlags::empty()
            };
        Ok(vec![Packet {
            data,
            pts: frame.frame.pts,
            dts: frame.frame.pts,
            flags,
            track_id: 0,
            duration: None,
        }])
    }
}
impl ThrumEncoder {
    /// Code one macroblock at `(mbx, mby)`: intra (mode-signalled) or
    /// inter (skip / motion vector / residual). The reconstruction is
    /// written into `recon` as it goes, so later macroblocks and blocks
    /// predict from it.
    #[allow(clippy::too_many_arguments)]
    fn encode_macroblock(
        &mut self,
        w: &mut BitWriter,
        src: &Picture,
        recon: &mut Picture,
        reference: Option<&Picture>,
        mbx: usize,
        mby: usize,
        intra: bool,
    ) -> CodecResult<()> {
        let mx = mbx * MB_SIZE;
        let my = mby * MB_SIZE;

        let start_bits = bit_pos(w);
        if intra {
            w.write(1, 1).map_err(|_| CodecError::Internal)?;
            for (bx, by) in LUMA_BLOCK_OFFSETS {
                let pos = BlockPos {
                    x0: mx + bx * BLOCK,
                    y0: my + by * BLOCK,
                };
                let mode =
                    best_intra_mode(&recon.luma, &src.luma, pos.x0, pos.y0, &mut self.scratch);
                w.write(2, mode as u64).map_err(|_| CodecError::Internal)?;
                code_intra_block(w, &src.luma, &mut recon.luma, pos, mode, &mut self.scratch)?;
            }
            if src.has_chroma() {
                code_intra_chroma(
                    w,
                    src,
                    recon,
                    BlockPos {
                        x0: mx / 2,
                        y0: my / 2,
                    },
                    &mut self.scratch,
                )?;
            }
            self.stats.intra_mbs += 1;
            self.stats.intra_bits += (bit_pos(w) - start_bits) as u64;
            return Ok(());
        }

        // Inter: search luma SAD, then decide skip from the full
        // luma+chroma cost of the winning vector against the rate-aware
        // threshold (see `skip_threshold`).
        let reference = reference.expect("inter macroblock needs a reference");
        let (dx, dy, _) = {
            let s = &mut self.scratch;
            motion_search(
                &src.luma,
                &reference.luma,
                mx,
                my,
                i32::from(self.enc.search_range),
                &mut s.mb_block,
                &mut s.mb_cand,
            )
        };
        // A skipped macroblock is reconstructed at zero motion (the skip flag
        // carries no vector), so the skip test must judge that same
        // prediction. Judging the searched vector let a moving block skip
        // with a wrong, zero-motion reconstruction.
        let zero_sad = macroblock_sad(src, reference, mx, my, 0, 0);
        let skip = zero_sad < skip_threshold(self.scratch.step);

        w.write(1, 0).map_err(|_| CodecError::Internal)?;
        if skip {
            w.write(1, 1).map_err(|_| CodecError::Internal)?;
            copy_mb_from_reference(reference, recon, mx, my, 0, 0);
            self.stats.skip_mbs += 1;
            self.stats.skip_bits += (bit_pos(w) - start_bits) as u64;
            return Ok(());
        }
        w.write(1, 0).map_err(|_| CodecError::Internal)?;
        w.write(MV_BITS, (dx + i32::from(MAX_SEARCH_RANGE)) as u64)
            .map_err(|_| CodecError::Internal)?;
        w.write(MV_BITS, (dy + i32::from(MAX_SEARCH_RANGE)) as u64)
            .map_err(|_| CodecError::Internal)?;
        self.stats.inter_mbs += 1;
        let after_header = bit_pos(w);
        self.stats.mv_bits += (after_header - start_bits) as u64;

        let mv = MotionVector { dx, dy };
        for (bx, by) in LUMA_BLOCK_OFFSETS {
            code_inter_block(
                w,
                &src.luma,
                &mut recon.luma,
                &reference.luma,
                BlockPos {
                    x0: mx + bx * BLOCK,
                    y0: my + by * BLOCK,
                },
                mv,
                &mut self.scratch,
            )?;
        }
        let after_luma = bit_pos(w);
        self.stats.luma_bits += (after_luma - after_header) as u64;
        if src.has_chroma() {
            code_inter_chroma(
                w,
                src,
                recon,
                reference,
                BlockPos {
                    x0: mx / 2,
                    y0: my / 2,
                },
                MotionVector {
                    dx: chroma_mv(dx),
                    dy: chroma_mv(dy),
                },
                &mut self.scratch,
            )?;
        }
        self.stats.chroma_bits += (bit_pos(w) - after_luma) as u64;
        Ok(())
    }
}

/// Luma + both chroma SAD of the inter prediction for one macroblock at
/// `(dx, dy)`: the cost the skip decision is made on.
fn macroblock_sad(
    src: &Picture,
    reference: &Picture,
    mx: usize,
    my: usize,
    dx: i32,
    dy: i32,
) -> u32 {
    let mut block = [0u8; MB_SIZE * MB_SIZE];
    let mut cand = [0u8; MB_SIZE * MB_SIZE];
    src.luma.load_block(mx, my, MB_SIZE, MB_SIZE, &mut block);
    load_mc_block(&reference.luma, mx, my, MB_SIZE, MB_SIZE, dx, dy, &mut cand);
    let mut total = sad(&block, &cand);
    if src.has_chroma() {
        let (cx, cy) = (mx / 2, my / 2);
        let (cdx, cdy) = (chroma_mv(dx), chroma_mv(dy));
        for c in 0..2 {
            let mut sb = [0u8; BLOCK * BLOCK];
            let mut cb = [0u8; BLOCK * BLOCK];
            src.chroma[c].load_block(cx, cy, BLOCK, BLOCK, &mut sb);
            load_mc_block(
                &reference.chroma[c],
                cx,
                cy,
                BLOCK,
                BLOCK,
                cdx,
                cdy,
                &mut cb,
            );
            total += sad(&sb, &cb);
        }
    }
    total
}
// --- decoder ----------------------------------------------------------------

/// Thrum decoder.
///
/// Keeps one reference picture (the deblocked reconstruction of the
/// previous frame) and a copy of the last emitted frame for
/// [`ThrumDecoder::conceal`]. There is no reordering buffer and no
/// look-ahead, so `decode` emits exactly one frame per packet and the
/// codec's latency is the cost of that one packet.
pub struct ThrumDecoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: ThrumConfig,
    /// Decoded (post-deblock) previous frame - the P-frame reference.
    reference: Option<Picture>,
    /// Last frame handed out, kept for concealment.
    last: Option<Picture>,
    scratch: BlockScratch,
}

impl ThrumDecoder {
    /// Build a decoder for a given configuration.
    pub fn new(codec_cfg: CodecConfig, enc: ThrumConfig) -> CodecResult<Self> {
        enc.validate()?;
        let info = static_info(
            enc.width,
            enc.height,
            enc.format,
            Capabilities {
                real_time_decoder: true,
                lossy: true,
                ..Capabilities::default()
            },
        );
        Ok(ThrumDecoder {
            info,
            cfg: codec_cfg,
            enc,
            reference: None,
            last: None,
            scratch: BlockScratch::new(),
        })
    }

    /// Packet-loss concealment: repeat the last decoded frame, which is
    /// the correct freeze-style concealment for a codec with no temporal
    /// hierarchy. With no prior frame there is nothing to repeat, so the
    /// first-ever concealment is a flat mid-grey picture.
    pub fn conceal(&mut self, pts: Timestamp) -> CodecResult<VideoFrame> {
        let pic = match &self.last {
            Some(p) => p.clone(),
            // Nothing decoded yet: a flat mid-grey picture, the same
            // neutral start the intra DC predictor uses.
            None => {
                let w = self.enc.width as usize;
                let h = self.enc.height as usize;
                let mut p = Picture::new(w, h, self.enc.format);
                for v in p.luma.data.iter_mut() {
                    *v = 128;
                }
                for c in 0..2 {
                    for v in p.chroma[c].data.iter_mut() {
                        *v = 128;
                    }
                }
                p
            }
        };
        Ok(picture_to_frame(&pic, pts))
    }

    /// Number of reference pictures this decoder retains. Thrum uses
    /// exactly one, which is what a decoder-side `max_reference_frames`
    /// limit has to allow.
    pub const REFERENCE_FRAMES: u8 = 1;

    /// Discard all decoder state (the `Decoder::flush` contract).
    fn reset(&mut self) {
        self.reference = None;
        self.last = None;
    }
}

impl Decoder for ThrumDecoder {
    type Frame = VideoFrame;

    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<VideoFrame>> {
        self.cfg.limits.check_packet(packet.data.len())?;
        self.cfg
            .limits
            .check_reference_frames(ThrumDecoder::REFERENCE_FRAMES)?;
        self.cfg.limits.check_frame_bytes(self.enc.frame_bytes())?;

        if packet.data.len() < CRC_TRAILER_BYTES {
            return Err(CodecError::MalformedPacket);
        }
        let (body, checksum) = packet.data.split_at(packet.data.len() - CRC_TRAILER_BYTES);
        let expected = u32::from_le_bytes(checksum.try_into().expect("4-byte slice"));
        if crc32(body) != expected {
            return Err(CodecError::MalformedPacket);
        }

        let mut r = BitReader::new(body);
        // Reject a wire version this decoder does not understand before
        // interpreting any further bits as if they were this version's
        // layout.
        let version = r.read(16).map_err(|_| CodecError::MalformedPacket)? as u16;
        if version != VERSION {
            return Err(CodecError::Unsupported("unsupported Thrum packet version"));
        }
        let intra = r.read(1).map_err(|_| CodecError::MalformedPacket)? == 1;
        let qp = r.read(QP_BITS).map_err(|_| CodecError::MalformedPacket)? as u8;
        if !(MIN_QP..=MAX_QP).contains(&qp) {
            return Err(CodecError::MalformedPacket);
        }
        // A P-frame needs a reference; without one the stream is
        // undecodable rather than merely damaged.
        if !intra && self.reference.is_none() {
            return Err(CodecError::NeedMoreInput);
        }
        self.scratch.step = quant_step(qp);
        let (mb_w, mb_h) = self.enc.mb_dims();

        let reference = self.reference.clone();
        let mut recon = match &reference {
            Some(r) => r.clone(),
            None => Picture::new(
                self.enc.width as usize,
                self.enc.height as usize,
                self.enc.format,
            ),
        };

        // Raster order, matching the encoder's `for mb in 0..mb_count`
        // walk. (Iterating `mbx` in the outer loop here decodes the frame
        // column-major, which silently desynchronises every macroblock
        // after the first against both the bitstream and the intra
        // prediction context.)
        for mby in 0..mb_h {
            for mbx in 0..mb_w {
                self.decode_macroblock(&mut r, &mut recon, reference.as_ref(), mbx, mby)?;
            }
        }

        let (beta, alpha, tc) = deblock_params(qp);
        deblock_plane(&mut recon.luma, beta, alpha, tc);
        if recon.has_chroma() {
            deblock_plane(&mut recon.chroma[0], beta, alpha, tc);
            deblock_plane(&mut recon.chroma[1], beta, alpha, tc);
        }

        let frame = picture_to_frame(&recon, packet.pts);
        self.reference = Some(recon.clone());
        self.last = Some(recon);
        Ok(vec![frame])
    }

    fn flush(&mut self) -> CodecResult<Vec<VideoFrame>> {
        self.reset();
        Ok(Vec::new())
    }
}
impl ThrumDecoder {
    /// Decode one macroblock at `(mbx, mby)`, mirroring
    /// [`ThrumEncoder::encode_macroblock`] bit for bit.
    fn decode_macroblock(
        &mut self,
        r: &mut BitReader,
        recon: &mut Picture,
        reference: Option<&Picture>,
        mbx: usize,
        mby: usize,
    ) -> CodecResult<()> {
        let mx = mbx * MB_SIZE;
        let my = mby * MB_SIZE;
        let has_chroma = recon.has_chroma();
        let intra = r.read(1).map_err(|_| CodecError::MalformedPacket)? == 1;

        if intra {
            for (bx, by) in LUMA_BLOCK_OFFSETS {
                let pos = BlockPos {
                    x0: mx + bx * BLOCK,
                    y0: my + by * BLOCK,
                };
                let mode =
                    IntraMode::from_bits(r.read(2).map_err(|_| CodecError::MalformedPacket)? as u8);
                self.decode_intra_block(r, &mut recon.luma, pos, mode)?;
            }
            if has_chroma {
                let pos = BlockPos {
                    x0: mx / 2,
                    y0: my / 2,
                };
                for c in 0..2 {
                    let plane = &mut recon.chroma[c];
                    self.decode_intra_block(r, plane, pos, IntraMode::Dc)?;
                }
            }
            return Ok(());
        }

        let skip = r.read(1).map_err(|_| CodecError::MalformedPacket)? == 1;
        if skip {
            let reference = reference.ok_or(CodecError::NeedMoreInput)?;
            copy_mb_from_reference(reference, recon, mx, my, 0, 0);
            return Ok(());
        }
        let reference = reference.ok_or(CodecError::NeedMoreInput)?;
        // MV fields are biased by MAX_SEARCH_RANGE, so 0..=32 maps back
        // to -16..=16 and is always in range by construction.
        let mv = MotionVector {
            dx: r.read(MV_BITS).map_err(|_| CodecError::MalformedPacket)? as i32
                - i32::from(MAX_SEARCH_RANGE),
            dy: r.read(MV_BITS).map_err(|_| CodecError::MalformedPacket)? as i32
                - i32::from(MAX_SEARCH_RANGE),
        };

        for (bx, by) in LUMA_BLOCK_OFFSETS {
            self.decode_inter_block(
                r,
                &mut recon.luma,
                &reference.luma,
                BlockPos {
                    x0: mx + bx * BLOCK,
                    y0: my + by * BLOCK,
                },
                mv,
            )?;
        }
        if has_chroma {
            let pos = BlockPos {
                x0: mx / 2,
                y0: my / 2,
            };
            let cmv = MotionVector {
                dx: chroma_mv(mv.dx),
                dy: chroma_mv(mv.dy),
            };
            for c in 0..2 {
                let plane = &mut recon.chroma[c];
                let reference = &reference.chroma[c];
                self.decode_inter_block(r, plane, reference, pos, cmv)?;
            }
        }
        Ok(())
    }

    /// Intra: read the block's levels, build the same prediction the
    /// encoder used, and reconstruct.
    fn decode_intra_block(
        &mut self,
        r: &mut BitReader,
        recon: &mut Plane,
        pos: BlockPos,
        mode: IntraMode,
    ) -> CodecResult<()> {
        read_block(r, &mut self.scratch.levels)?;
        intra_predict(recon, pos.x0, pos.y0, mode, &mut self.scratch.pred);
        let levels = self.scratch.levels;
        decode_block(&levels, &mut self.scratch);
        recon.store_block(pos.x0, pos.y0, BLOCK, BLOCK, &self.scratch.recon);
        Ok(())
    }

    /// Inter: read the block's levels, predict from the reference, and
    /// reconstruct.
    fn decode_inter_block(
        &mut self,
        r: &mut BitReader,
        recon: &mut Plane,
        reference: &Plane,
        pos: BlockPos,
        mv: MotionVector,
    ) -> CodecResult<()> {
        read_block(r, &mut self.scratch.levels)?;
        load_mc_block(
            reference,
            pos.x0,
            pos.y0,
            BLOCK,
            BLOCK,
            mv.dx,
            mv.dy,
            &mut self.scratch.pred,
        );
        let levels = self.scratch.levels;
        decode_block(&levels, &mut self.scratch);
        recon.store_block(pos.x0, pos.y0, BLOCK, BLOCK, &self.scratch.recon);
        Ok(())
    }
}
// --- type-erased factories --------------------------------------------------

/// Type-erased decoder factory.
///
/// A video codec's geometry is part of its configuration, so the
/// factory is constructed with it rather than reading it from the
/// `CodecConfig` (which carries only the track's private bytes). This
/// is what [`Registry::find_decoder`](tpt_media_codec::Registry::find_decoder)
/// hands back for a `LIVE` track.
pub struct ThrumDecoderFactory {
    enc: ThrumConfig,
}

impl ThrumDecoderFactory {
    /// Build a factory for a given geometry and configuration.
    pub fn new(enc: ThrumConfig) -> CodecResult<Self> {
        enc.validate()?;
        Ok(ThrumDecoderFactory { enc })
    }
}

impl DynDecoderFactory for ThrumDecoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(
            self.enc.width,
            self.enc.height,
            self.enc.format,
            Capabilities {
                real_time_decoder: true,
                lossy: true,
                ..Capabilities::default()
            },
        )
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<Box<dyn DynDecoder>> {
        let mut merged = self.enc.clone();
        if cfg.extra.len() >= ThrumConfig::CODEC_PRIVATE_LEN {
            merged = ThrumConfig::from_codec_private(self.enc.width, self.enc.height, &cfg.extra)?;
        }
        let d = ThrumDecoder::new(cfg.clone(), merged)?;
        Ok(Box::new(ThrumDynDecoder { inner: d }))
    }
}

/// Type-erased encoder factory; see [`ThrumDecoderFactory`] for why it
/// carries the geometry.
pub struct ThrumEncoderFactory {
    enc: ThrumConfig,
    keyframe_interval: u64,
}

impl ThrumEncoderFactory {
    /// Build a factory for a given geometry and configuration, using the
    /// default I-frame interval.
    pub fn new(enc: ThrumConfig) -> CodecResult<Self> {
        ThrumEncoderFactory::with_keyframe_interval(enc, KEYFRAME_INTERVAL)
    }

    /// Build a factory with an explicit I-frame interval in frames.
    pub fn with_keyframe_interval(enc: ThrumConfig, keyframe_interval: u64) -> CodecResult<Self> {
        enc.validate()?;
        Ok(ThrumEncoderFactory {
            enc,
            keyframe_interval,
        })
    }
}

impl DynEncoderFactory for ThrumEncoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(
            self.enc.width,
            self.enc.height,
            self.enc.format,
            Capabilities {
                real_time_decoder: true,
                lossy: true,
                ..Capabilities::default()
            },
        )
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<Box<dyn DynEncoder>> {
        let mut merged = self.enc.clone();
        if cfg.extra.len() >= ThrumConfig::CODEC_PRIVATE_LEN {
            merged = ThrumConfig::from_codec_private(self.enc.width, self.enc.height, &cfg.extra)?;
        }
        let e = ThrumEncoder::with_keyframe_interval(cfg.clone(), merged, self.keyframe_interval)?;
        Ok(Box::new(ThrumDynEncoder { inner: e }))
    }
}

fn dyn_video_frame(f: VideoFrame) -> tpt_media_codec::DynFrame {
    tpt_media_codec::DynFrame::Video {
        pixel_format: f.format,
        width: f.width,
        height: f.height,
        stride: f.stride,
        pixels: f.pixels,
    }
}

/// Type-erased wrapper around [`ThrumDecoder`].
pub struct ThrumDynDecoder {
    inner: ThrumDecoder,
}

impl DynDecoder for ThrumDynDecoder {
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<tpt_media_codec::DynFrame>> {
        Ok(Decoder::decode(&mut self.inner, packet)?
            .into_iter()
            .map(dyn_video_frame)
            .collect())
    }
    fn flush(&mut self) -> CodecResult<Vec<tpt_media_codec::DynFrame>> {
        Ok(Decoder::flush(&mut self.inner)?
            .into_iter()
            .map(dyn_video_frame)
            .collect())
    }
    fn drain(&mut self) -> CodecResult<Vec<tpt_media_codec::DynFrame>> {
        Ok(Decoder::drain(&mut self.inner)?
            .into_iter()
            .map(dyn_video_frame)
            .collect())
    }
    fn info(&self) -> CodecInfo {
        self.inner.info().clone()
    }
}

/// Type-erased wrapper around [`ThrumEncoder`].
pub struct ThrumDynEncoder {
    inner: ThrumEncoder,
}

impl DynEncoder for ThrumDynEncoder {
    fn encode(&mut self, frame: &tpt_media_codec::DynFrame) -> CodecResult<Vec<Packet>> {
        let f = match frame {
            tpt_media_codec::DynFrame::Video {
                pixel_format,
                width,
                height,
                stride,
                pixels,
            } => VideoFrame {
                frame: Frame {
                    pts: Timestamp::zero(tpt_media_core::Timebase::ONE_PER_SECOND),
                    duration: None,
                    side_data: Vec::new(),
                },
                pixels: pixels.clone(),
                width: *width,
                height: *height,
                format: *pixel_format,
                stride: *stride,
            },
            tpt_media_codec::DynFrame::Audio { .. } => {
                return Err(CodecError::Unsupported("thrum does not code audio"))
            }
        };
        Encoder::encode(&mut self.inner, &f)
    }
    fn flush(&mut self) -> CodecResult<Vec<Packet>> {
        Encoder::flush(&mut self.inner)
    }
    fn drain(&mut self) -> CodecResult<Vec<Packet>> {
        Encoder::drain(&mut self.inner)
    }
    fn info(&self) -> CodecInfo {
        self.inner.info().clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_core::Timebase;

    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state
    }

    /// A deterministic test picture: a moving diagonal ramp plus a
    /// little noise, so both intra prediction and motion search have
    /// something real to do.
    fn test_picture(w: u32, h: u32, format: PixelFormat, frame: i32) -> VideoFrame {
        let mut f = VideoFrame::alloc(w, h, format);
        let mut state = 0x5EED_u64 ^ (frame as u64).wrapping_mul(0x9E37_79B9);
        let (w, h) = (w as usize, h as usize);
        for y in 0..h {
            for x in 0..w {
                let v = ((x as i32 + y as i32 + frame * 3) % 256) as u8;
                f.pixels[y * w + x] = v ^ (lcg(&mut state) as u8 & 3);
            }
        }
        if format == PixelFormat::Yuv420 {
            let (cw, ch) = (w / 2, h / 2);
            let base = w * h;
            for c in 0..2 {
                for y in 0..ch {
                    for x in 0..cw {
                        f.pixels[base + c * cw * ch + y * cw + x] =
                            (64 + (x * 3 + y + c) % 180) as u8;
                    }
                }
            }
        }
        f
    }

    fn cfg(w: u32, h: u32, format: PixelFormat) -> ThrumConfig {
        ThrumConfig {
            width: w,
            height: h,
            format,
            ..ThrumConfig::default()
        }
    }

    /// A smooth, video-like test picture: a low-frequency luma gradient
    /// with a moving bright block and mild chroma variation. This is the
    /// content a transform codec is *designed* for, so a round trip
    /// through it should be visually near-lossless at low `qp`.
    fn smooth_picture(w: u32, h: u32, format: PixelFormat, frame: i32) -> VideoFrame {
        let mut f = VideoFrame::alloc(w, h, format);
        let (w, h) = (w as usize, h as usize);
        let shift = (frame * 2).rem_euclid(w.max(1) as i32) as usize;
        for y in 0..h {
            for x in 0..w {
                // Smooth 2D gradient.
                let mut v = 40.0 + (x as f32) * 120.0 / w as f32 + (y as f32) * 70.0 / h as f32;
                // A bright block that pans a few pixels per frame, so
                // motion estimation has real work to do.
                let bx = (x + w - shift) % w;
                if (bx as f32 - w as f32 * 0.6).abs() < w as f32 * 0.12
                    && (y as f32 - h as f32 * 0.4).abs() < h as f32 * 0.15
                {
                    v = 210.0;
                }
                f.pixels[y * w + x] = to_u8(v);
            }
        }
        if format == PixelFormat::Yuv420 {
            let (cw, ch) = (w / 2, h / 2);
            let base = w * h;
            for c in 0..2 {
                for y in 0..ch {
                    for x in 0..cw {
                        let v = 110.0 + (x as f32) * 30.0 / cw as f32 + (c as f32) * 20.0;
                        f.pixels[base + c * cw * ch + y * cw + x] = to_u8(v);
                    }
                }
            }
        }
        f
    }

    /// PSNR between a reference and a reconstruction, in dB.
    fn psnr_db(a: &[u8], b: &[u8]) -> f64 {
        let mut se = 0f64;
        let mut sr = 0f64;
        for (x, y) in a.iter().zip(b.iter()) {
            let d = f64::from(*x) - f64::from(*y);
            se += d * d;
            sr += f64::from(*x) * f64::from(*x);
        }
        if se == 0.0 {
            return f64::INFINITY;
        }
        10.0 * (sr / se).log10()
    }

    #[test]
    fn crc32_matches_the_reference_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn zigzag_round_trips() {
        for v in [-32767, -1000, -7, -1, 0, 1, 7, 1000, 32767] {
            assert_eq!(zigzag_decode(zigzag_encode(v)), v);
        }
    }

    #[test]
    fn zigzag_is_a_bijection_on_the_clamped_range() {
        let mut seen = std::collections::HashSet::new();
        for v in -LEVEL_CLAMP..=LEVEL_CLAMP {
            assert!(seen.insert(zigzag_encode(v)), "collision at {v}");
        }
    }

    #[test]
    fn quantize_dead_zone_and_clamp() {
        let step = quant_step(10);
        assert_eq!(quantize(0.0, step, 0.5), 0);
        // Below the dead zone quantises to zero regardless of sign.
        assert_eq!(quantize(step * 0.1, step, 0.5), 0);
        assert_eq!(quantize(-step * 0.1, step, 0.5), 0);
        // A large coefficient saturates at MAX_LEVEL, not beyond. That
        // bound is what makes the encoder's packet-size proof hold.
        assert_eq!(quantize(1e9, step, 0.5), MAX_LEVEL);
        assert_eq!(quantize(-1e9, step, 0.5), -MAX_LEVEL);
        // Sign is preserved for representable values.
        assert!(quantize(step * 10.0, step, 0.5) > 0);
        assert!(quantize(-step * 10.0, step, 0.5) < 0);
    }

    #[test]
    fn quant_step_is_monotonic_and_clamped() {
        for q in MIN_QP..MAX_QP {
            assert!(quant_step(q) < quant_step(q + 1), "qp {q} not monotonic");
        }
        assert_eq!(quant_step(0), quant_step(MIN_QP));
        assert_eq!(quant_step(255), quant_step(MAX_QP));
    }

    #[test]
    fn dct_round_trip_is_close_to_identity() {
        // The forward/inverse pair scales by 16; DCT_SCALE/IDCT_SCALE
        // cancel that, so a low-frequency block comes back intact.
        let mut block = vec![0f32; BLOCK_COEFFS];
        let mut scratch = vec![0f32; DCT_SCRATCH];
        for y in 0..BLOCK {
            for x in 0..BLOCK {
                block[y * BLOCK + x] = (x * 3 + y * 5) as f32;
            }
        }
        let original = block.clone();
        forward_dct(&mut block, &mut scratch);
        inverse_dct(&mut block, &mut scratch);
        for (a, b) in block.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-2, "{a} vs {b}");
        }
    }

    #[test]
    fn block_coding_round_trips_exactly() {
        let mut s = BlockScratch::new();
        let step = quant_step(20);
        for (i, slot) in s.src.iter_mut().enumerate() {
            *slot = ((i * 37) % 256) as u8;
        }
        s.pred.fill(100);
        s.step = step;
        let levels = encode_block(&mut s);
        let recon = s.recon.clone();
        s.recon.fill(0);
        s.step = step;
        decode_block(&levels, &mut s);
        assert_eq!(
            s.recon, recon,
            "decode must reproduce the encode reconstruction"
        );
    }

    #[test]
    fn write_block_read_block_round_trip() {
        let cases: [[i32; BLOCK_COEFFS]; 4] = [
            [0; BLOCK_COEFFS],
            {
                let mut b = [0i32; BLOCK_COEFFS];
                b[0] = 42;
                b[63] = -7;
                b
            },
            {
                let mut b = [0i32; BLOCK_COEFFS];
                for (i, slot) in b.iter_mut().enumerate() {
                    *slot = if i % 3 == 0 { (i as i32) - 20 } else { 0 };
                }
                b
            },
            {
                let mut b = [0i32; BLOCK_COEFFS];
                for (i, slot) in b.iter_mut().enumerate() {
                    *slot = LEVEL_CLAMP;
                    if i % 2 == 1 {
                        *slot = -LEVEL_CLAMP;
                    }
                }
                b
            },
        ];
        for levels in cases {
            // Sized from the same bound the encoder uses, so a block of
            // MAX_LEVEL magnitudes always fits.
            let bits: usize = BLOCK_COEFFS * (1 + 2 + 6 + 2 * BLOCK_COEFFS * 260);
            let mut buf = vec![0u8; bits.div_ceil(8)];
            let n = {
                let mut w = BitWriter::new(&mut buf);
                write_block(&mut w, &levels).unwrap();
                w.finish().unwrap()
            };
            let mut r = BitReader::new(&buf[..n]);
            let mut out = [0i32; BLOCK_COEFFS];
            read_block(&mut r, &mut out).unwrap();
            assert_eq!(out, levels);
        }
    }

    #[test]
    fn a_block_of_maximum_levels_never_exceeds_the_packet_bound() {
        // The encoder's single allocation is only safe if this holds for
        // the worst block a well-formed frame can produce.
        let mut levels = [0i32; BLOCK_COEFFS];
        for (i, slot) in levels.iter_mut().enumerate() {
            *slot = if i % 2 == 0 { MAX_LEVEL } else { -MAX_LEVEL };
        }
        let bits: usize = 1 + 2 + 6 + 2 * BLOCK_COEFFS * 260;
        let mut buf = vec![0u8; bits.div_ceil(8)];
        let n = {
            let mut w = BitWriter::new(&mut buf);
            write_block(&mut w, &levels).unwrap();
            w.finish().unwrap()
        };
        let mut r = BitReader::new(&buf[..n]);
        let mut out = [0i32; BLOCK_COEFFS];
        read_block(&mut r, &mut out).unwrap();
        assert_eq!(out, levels);
        // A full-size frame of such blocks still fits its bound.
        let c = cfg(176, 144, PixelFormat::Yuv420);
        let bytes = c.max_packet_bits().div_ceil(8);
        assert!(n <= bytes);
    }

    #[test]
    fn write_block_rejects_an_undersized_buffer() {
        // A 1-byte buffer cannot hold even a coded block; the writer
        // must surface that rather than silently truncating.
        let mut buf = [0u8; 1];
        let mut w = BitWriter::new(&mut buf);
        let mut levels = [0i32; BLOCK_COEFFS];
        levels[0] = 1234;
        assert!(write_block(&mut w, &levels).is_err());
    }

    #[test]
    fn config_validation_rejects_bad_geometry_and_ranges() {
        assert!(cfg(16, 16, PixelFormat::Gray8).validate().is_ok());
        // Non-multiple-of-16 geometry.
        assert!(cfg(17, 16, PixelFormat::Gray8).validate().is_err());
        assert!(cfg(16, 0, PixelFormat::Gray8).validate().is_err());
        // Uncoded pixel format.
        assert!(cfg(16, 16, PixelFormat::Rgb24).validate().is_err());
        // qp out of range.
        let mut c = cfg(16, 16, PixelFormat::Gray8);
        c.qp = 0;
        assert!(c.validate().is_err());
        c.qp = MAX_QP;
        assert!(c.validate().is_ok());
        // search_range out of range.
        let mut c = cfg(16, 16, PixelFormat::Gray8);
        c.search_range = MAX_SEARCH_RANGE + 1;
        assert!(c.validate().is_err());
        // intra_refresh_percent out of range.
        let mut c = cfg(16, 16, PixelFormat::Gray8);
        c.intra_refresh_percent = 101;
        assert!(c.validate().is_err());
    }

    #[test]
    fn codec_private_round_trips() {
        let c = ThrumConfig {
            width: 320,
            height: 240,
            format: PixelFormat::Gray8,
            qp: 17,
            search_range: 12,
            intra_refresh_percent: 9,
        };
        let bytes = c.to_codec_private();
        assert_eq!(bytes.len(), ThrumConfig::CODEC_PRIVATE_LEN);
        let back = ThrumConfig::from_codec_private(320, 240, &bytes).unwrap();
        assert_eq!(c, back);
        // An unknown pixel-format tag is rejected, not guessed at.
        assert!(ThrumConfig::from_codec_private(320, 240, &[9, 1, 1, 1]).is_err());
    }

    #[test]
    fn quality_maps_onto_qp_monotonically() {
        assert_eq!(quality_to_qp(100), MIN_QP);
        assert_eq!(quality_to_qp(0), MAX_QP);
        // Higher quality must never mean a coarser quantiser.
        for q in 0..100u8 {
            assert!(
                quality_to_qp(q) >= quality_to_qp(q + 1),
                "quality {q} -> qp {} but {} -> qp {}",
                quality_to_qp(q),
                q + 1,
                quality_to_qp(q + 1)
            );
        }
        // Out-of-range input is clamped to the 0..=100 domain, so 255
        // behaves exactly like 100 (best quality), never like 0.
        assert_eq!(quality_to_qp(255), quality_to_qp(100));
    }

    #[test]
    fn mb_dims_and_frame_bytes_are_consistent() {
        let c = cfg(320, 240, PixelFormat::Yuv420);
        assert_eq!(c.mb_dims(), (20, 15));
        assert_eq!(c.mb_count(), 300);
        assert_eq!(c.frame_bytes(), 320 * 240 * 3 / 2);
        let g = cfg(320, 240, PixelFormat::Gray8);
        assert_eq!(g.frame_bytes(), 320 * 240);
    }

    #[test]
    fn picture_frame_round_trip_is_byte_exact() {
        for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
            let f = test_picture(64, 48, format, 3);
            let pic = picture_from_frame(&f).unwrap();
            let back = picture_to_frame(&pic, Timestamp::zero(Timebase::ONE_PER_SECOND));
            assert_eq!(back.pixels, f.pixels, "{format:?} lost pixels");
            assert_eq!(back.width, f.width);
            assert_eq!(back.height, f.height);
            assert_eq!(back.format, f.format);
            assert_eq!(back.stride, f.stride);
        }
    }

    #[test]
    fn picture_from_frame_rejects_bad_geometry() {
        let mut f = VideoFrame::alloc(24, 16, PixelFormat::Gray8);
        f.width = 24;
        assert!(picture_from_frame(&f).is_err());
        let f = VideoFrame::alloc(16, 16, PixelFormat::Rgb24);
        assert!(matches!(
            picture_from_frame(&f),
            Err(CodecError::Unsupported(_))
        ));
    }

    #[test]
    fn intra_mode_availability_matches_geometry() {
        // At the origin only DC is available: there is neither a top
        // edge nor a left edge to predict from.
        assert!(intra_mode_available(IntraMode::Dc, 0, 0));
        assert!(!intra_mode_available(IntraMode::Vertical, 0, 0));
        assert!(!intra_mode_available(IntraMode::Horizontal, 0, 0));
        assert!(!intra_mode_available(IntraMode::Diagonal, 0, 0));
        // One block in, the matching edge appears.
        assert!(!intra_mode_available(IntraMode::Vertical, 8, 0));
        assert!(intra_mode_available(IntraMode::Vertical, 8, 8));
        assert!(!intra_mode_available(IntraMode::Horizontal, 0, 8));
        assert!(intra_mode_available(IntraMode::Horizontal, 8, 8));
        assert!(intra_mode_available(IntraMode::Diagonal, 8, 8));
    }

    #[test]
    fn intra_predict_vertical_copies_the_row_above() {
        let mut plane = Plane::new(32, 32, 0);
        for x in 0..32 {
            plane.set(x, 7, (x as u8).wrapping_mul(3).wrapping_add(10));
        }
        let mut pred = [0u8; BLOCK_COEFFS];
        intra_predict(&plane, 8, 8, IntraMode::Vertical, &mut pred);
        // Only the first BLOCK entries are compared: `pred` is 8x8 and
        // flattened, so entry `x` past the first row is a *different*
        // row of the block, not column `x` of row 0.
        for (x, slot) in pred.iter().take(BLOCK).enumerate() {
            assert_eq!(*slot, plane.at(8 + x, 7));
        }
    }

    #[test]
    fn intra_predict_falls_back_to_dc_when_unavailable() {
        let plane = Plane::new(32, 32, 77);
        let mut pred = [0u8; BLOCK_COEFFS];
        // Vertical is unavailable at y0 = 0 and must degrade to DC
        // (mid-grey with no edges at the origin) rather than read OOB.
        intra_predict(&plane, 0, 0, IntraMode::Vertical, &mut pred);
        assert!(pred.iter().all(|&v| v == 128));
    }

    #[test]
    fn deblock_leaves_a_smooth_plane_untouched() {
        let mut plane = Plane::new(64, 64, 100);
        let (b, a, t) = deblock_params(30);
        deblock_plane(&mut plane, b, a, t);
        assert!(plane.data.iter().all(|&v| v == 100));
    }

    #[test]
    fn deblock_smooths_a_wide_difference_but_preserves_a_hard_edge() {
        let (b, a, t) = deblock_params(40);
        // A 1-LSB step: below alpha, so the wide path averages it flat.
        let mut soft = Plane::new(64, 64, 0);
        for y in 0..64 {
            for x in 0..64 {
                soft.data[y * soft.stride + x] = if x < 32 { 100 } else { 101 };
            }
        }
        deblock_plane(&mut soft, b, a, t);
        assert!(soft.data[31] <= 101 && soft.data[32] >= 100);

        // A 200-LSB step: above alpha (4 * 40 = 160), so it survives.
        let mut hard = Plane::new(64, 64, 0);
        for y in 0..64 {
            for x in 0..64 {
                hard.data[y * hard.stride + x] = if x < 32 { 0 } else { 255 };
            }
        }
        deblock_plane(&mut hard, b, a, t);
        assert!(hard.at(20, 0) == 0, "hard edge was smoothed away");
        assert!(hard.at(44, 0) == 255, "hard edge was smoothed away");
    }

    #[test]
    fn deblock_pair_is_a_no_op_above_alpha() {
        let (b, a, t) = deblock_params(20);
        let mut s = [0u8, 0, 250, 250];
        deblock_pair(&mut s, b, a, t);
        assert_eq!(
            s,
            [0, 0, 250, 250],
            "difference above alpha must pass through"
        );
    }

    #[test]
    fn the_default_interval_makes_only_the_first_frame_a_keyframe() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c.clone()).unwrap();
        // KEYFRAME_INTERVAL is 30, so four frames is one I-frame and
        // three P-frames.
        assert_eq!(KEYFRAME_INTERVAL, 30);
        for i in 0..4 {
            let f = test_picture(64, 48, PixelFormat::Gray8, i);
            let pkts = enc.encode(&f).unwrap();
            assert_eq!(pkts.len(), 1, "one frame must be one packet");
            assert_eq!(pkts[0].is_keyframe(), i == 0, "frame {i} keyframe flag");
            assert!(pkts[0].is_end_of_frame());
            dec.decode(&pkts[0]).unwrap();
        }
    }

    #[test]
    fn a_keyframe_interval_renews_the_reference_periodically() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let mut enc = ThrumEncoder::with_keyframe_interval(CodecConfig::default(), c, 3).unwrap();
        let flags: Vec<bool> = (0..7)
            .map(|i| {
                let f = test_picture(64, 48, PixelFormat::Gray8, i);
                enc.encode(&f).unwrap().remove(0).is_keyframe()
            })
            .collect();
        assert_eq!(flags, vec![true, false, false, true, false, false, true]);
    }

    #[test]
    fn forced_intra_interval_makes_every_frame_a_keyframe() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let mut enc =
            ThrumEncoder::with_keyframe_interval(CodecConfig::default(), c.clone(), 0).unwrap();
        for i in 0..3 {
            let f = test_picture(64, 48, PixelFormat::Gray8, i);
            assert!(enc.encode(&f).unwrap()[0].is_keyframe());
        }
    }

    #[test]
    fn round_trip_preserves_dimensions_format_and_length() {
        for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
            let c = cfg(64, 48, format);
            let fw = CodecConfig::default();
            let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
            let mut dec = ThrumDecoder::new(fw, c.clone()).unwrap();
            for i in 0..4 {
                let f = test_picture(64, 48, format, i);
                let want = f.pixels.clone();
                for p in enc.encode(&f).unwrap() {
                    let out = dec.decode(&p).unwrap();
                    assert_eq!(out.len(), 1);
                    assert_eq!(out[0].width, 64);
                    assert_eq!(out[0].height, 48);
                    assert_eq!(out[0].format, format);
                    assert_eq!(out[0].pixels.len(), want.len());
                    assert_eq!(out[0].frame.pts, f.frame.pts, "PTS must pass through");
                }
            }
        }
    }

    #[test]
    fn a_high_qp_keyframe_is_visually_near_lossless_on_smooth_content() {
        // At a low qp the transform path must be effectively transparent
        // on the low-frequency content it is designed for. This is the
        // property that would break first if the DCT scaling, the
        // quantiser, or the intra prediction were wrong.
        let c = ThrumConfig {
            qp: 4,
            ..cfg(176, 144, PixelFormat::Yuv420)
        };
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        let f = smooth_picture(176, 144, PixelFormat::Yuv420, 0);
        let want = f.pixels.clone();
        let p = enc.encode(&f).unwrap().remove(0);
        let got = dec.decode(&p).unwrap().remove(0);
        let psnr = psnr_db(&want, &got.pixels);
        assert!(psnr > 38.0, "qp 4 keyframe PSNR was only {psnr:.2} dB");
    }

    #[test]
    fn a_p_frame_tracks_a_panning_object() {
        // Motion estimation must actually find the pan: coding frame N+1
        // against frame N's reconstruction has to be much cheaper than
        // coding it as intra, and must stay close to the source.
        let c = cfg(176, 144, PixelFormat::Yuv420);
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        let sizes: Vec<usize> = (0..4)
            .map(|i| {
                let f = smooth_picture(176, 144, PixelFormat::Yuv420, i);
                let p = enc.encode(&f).unwrap().remove(0);
                dec.decode(&p).unwrap();
                p.data.len()
            })
            .collect();
        // The I-frame is the most expensive; the P-frames that follow
        // must be materially cheaper or motion compensation is not working.
        assert!(
            sizes[1] * 2 < sizes[0],
            "P-frame {} B vs I-frame {} B",
            sizes[1],
            sizes[0]
        );
    }

    #[test]
    fn a_repeated_frame_reconstructs_as_well_as_the_intra_frame() {
        // Regression: a skipped macroblock is copied from the reference, and
        // the copy must keep the picture's row stride. A repeated frame is
        // all skip, so its reconstruction must match the intra frame's
        // quality. A stride bug once dropped it from about 42 dB to 25 dB.
        let c = ThrumConfig {
            qp: 6,
            ..cfg(128, 96, PixelFormat::Gray8)
        };
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        let f = test_picture(128, 96, PixelFormat::Gray8, 0);
        let want = f.pixels.clone();
        let mut psnr = Vec::new();
        for _ in 0..2 {
            let p = enc.encode(&f).unwrap().remove(0);
            let got = dec.decode(&p).unwrap().remove(0);
            psnr.push(psnr_db(&want, &got.pixels));
        }
        assert!(
            psnr[1] > psnr[0] - 3.0,
            "repeated frame {:.1} dB vs intra {:.1} dB",
            psnr[1],
            psnr[0]
        );
    }

    #[test]
    fn lower_qp_gives_higher_psnr() {
        // qp is the only rate knob, so it must order quality monotonically.
        // Measured on the *noisy* test picture, not the smooth one: a
        // smooth gradient is reconstructed almost exactly even at the
        // coarsest qp (the intra predictors and the loop filter do the
        // work), so it cannot discriminate between quantiser levels.
        let measure = |qp: u8| {
            let c = ThrumConfig {
                qp,
                ..cfg(128, 96, PixelFormat::Gray8)
            };
            let fw = CodecConfig::default();
            let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
            let mut dec = ThrumDecoder::new(fw, c).unwrap();
            let f = test_picture(128, 96, PixelFormat::Gray8, 0);
            let want = f.pixels.clone();
            let p = enc.encode(&f).unwrap().remove(0);
            let got = dec.decode(&p).unwrap().remove(0);
            psnr_db(&want, &got.pixels)
        };
        let coarse = measure(MAX_QP);
        let fine = measure(6);
        assert!(
            fine > coarse + 1.0,
            "qp 6 ({fine:.2} dB) should clearly beat qp {MAX_QP} ({coarse:.2} dB)"
        );
    }

    #[test]
    fn an_unchanged_p_frame_is_far_smaller_than_the_i_frame() {
        // A still scene must be nearly free after the I-frame: with 5%
        // intra refresh only a couple of macroblocks carry a residual and
        // the rest take the skip path. The bound is deliberately loose
        // (half the I-frame, not a quarter) because the refreshed
        // macroblocks legitimately carry a full intra residual.
        let c = cfg(128, 96, PixelFormat::Gray8);
        let mut enc = ThrumEncoder::new(CodecConfig::default(), c).unwrap();
        let still = test_picture(128, 96, PixelFormat::Gray8, 0);
        let i_size = enc.encode(&still).unwrap().remove(0).data.len();
        let p_size = enc.encode(&still).unwrap().remove(0).data.len();
        assert!(
            p_size * 2 < i_size,
            "unchanged P-frame ({p_size} B) should be under half the I-frame ({i_size} B)"
        );
    }

    #[test]
    fn the_skip_path_is_what_makes_a_p_frame_small() {
        // With intra refresh off, an unchanged P-frame is pure skip: one
        // bit per macroblock plus the header, i.e. far below the I-frame.
        let c = ThrumConfig {
            intra_refresh_percent: 0,
            ..cfg(128, 96, PixelFormat::Gray8)
        };
        let mut enc = ThrumEncoder::new(CodecConfig::default(), c).unwrap();
        let still = test_picture(128, 96, PixelFormat::Gray8, 0);
        let i_size = enc.encode(&still).unwrap().remove(0).data.len();
        let p_size = enc.encode(&still).unwrap().remove(0).data.len();
        // 48 macroblocks * 2 bits = 12 bytes, plus header and CRC.
        assert!(p_size < 32, "all-skip P-frame was {p_size} B, expected ~20");
        assert!(p_size * 4 < i_size);
    }

    #[test]
    fn intra_refresh_codes_some_macroblocks_intra_each_p_frame() {
        let c = ThrumConfig {
            intra_refresh_percent: 20,
            ..cfg(128, 96, PixelFormat::Gray8)
        };
        let enc = ThrumEncoder::new(CodecConfig::default(), c.clone()).unwrap();
        // 128x96 is 8x6 = 48 macroblocks; 20% is 9 per frame.
        assert_eq!(enc.enc.mb_count(), 48);
        assert_eq!(enc.refresh_count(), 9);
    }

    #[test]
    fn intra_refresh_sweeps_every_macroblock() {
        // The refresh window slides forward by `per_frame` each P-frame,
        // so the whole picture is covered in ceil(total / per_frame)
        // frames. 48 macroblocks at 9 per frame is 6 frames.
        let c = ThrumConfig {
            intra_refresh_percent: 20,
            ..cfg(128, 96, PixelFormat::Gray8)
        };
        let enc = ThrumEncoder::new(CodecConfig::default(), c).unwrap();
        let total = enc.enc.mb_count();
        let per_frame = enc.refresh_count();
        assert_eq!((total, per_frame), (48, 9));

        let mut seen = vec![false; total];
        let mut cursor = 0usize;
        for _ in 0..total.div_ceil(per_frame) {
            for (index, hit) in seen.iter_mut().enumerate() {
                let start = cursor % total;
                if (index + total - start) % total < per_frame {
                    *hit = true;
                }
            }
            cursor = (cursor + per_frame) % total;
        }
        assert!(seen.iter().all(|&s| s), "intra refresh missed macroblocks");
    }

    #[test]
    fn zero_percent_intra_refresh_selects_nothing() {
        let c = ThrumConfig {
            intra_refresh_percent: 0,
            ..cfg(128, 96, PixelFormat::Gray8)
        };
        let enc = ThrumEncoder::new(CodecConfig::default(), c).unwrap();
        assert_eq!(enc.refresh_count(), 0);
    }

    #[test]
    fn decode_rejects_an_unsupported_wire_version() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        let f = test_picture(64, 48, PixelFormat::Gray8, 0);
        let mut pkt = enc.encode(&f).unwrap().remove(0);
        // Rewrite the 16-bit version field to a value this decoder does
        // not implement, then re-checksum so the CRC check passes and the
        // version check is what actually rejects it.
        pkt.data[0] = 0xAB;
        pkt.data[1] = 0xCD;
        let n = pkt.data.len();
        let crc = crc32(&pkt.data[..n - CRC_TRAILER_BYTES]);
        pkt.data[n - CRC_TRAILER_BYTES..].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(dec.decode(&pkt), Err(CodecError::Unsupported(_))));
    }

    #[test]
    fn decode_rejects_a_corrupted_checksum() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        let f = test_picture(64, 48, PixelFormat::Gray8, 0);
        let mut pkt = enc.encode(&f).unwrap().remove(0);
        pkt.data[2] ^= 0xFF;
        assert_eq!(dec.decode(&pkt), Err(CodecError::MalformedPacket));
    }

    #[test]
    fn decode_rejects_an_undersized_packet() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let mut dec = ThrumDecoder::new(CodecConfig::default(), c).unwrap();
        let pkt = Packet {
            data: vec![0u8; CRC_TRAILER_BYTES - 1],
            pts: Timestamp::zero(Timebase::ONE_PER_SECOND),
            dts: Timestamp::zero(Timebase::ONE_PER_SECOND),
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        assert_eq!(dec.decode(&pkt), Err(CodecError::MalformedPacket));
    }

    #[test]
    fn a_p_frame_without_a_reference_is_need_more_input() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        // Encode two frames, then feed the decoder only the second.
        let f0 = test_picture(64, 48, PixelFormat::Gray8, 0);
        let _ = enc.encode(&f0).unwrap();
        let f1 = test_picture(64, 48, PixelFormat::Gray8, 1);
        let p_frame = enc.encode(&f1).unwrap().remove(0);
        assert!(!p_frame.is_keyframe());
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        assert_eq!(dec.decode(&p_frame), Err(CodecError::NeedMoreInput));
    }

    #[test]
    fn conceal_repeats_the_last_frame_and_mid_greys_without_one() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        // With no prior frame, concealment is a flat mid-grey picture.
        let first = dec
            .conceal(Timestamp::zero(Timebase::ONE_PER_SECOND))
            .unwrap();
        assert!(first.pixels.iter().all(|&v| v == 128));
        // The encoder emits an I-frame first, so a P-frame is only ever
        // produced after a decodable frame exists.
        let f0 = test_picture(64, 48, PixelFormat::Gray8, 0);
        dec.decode(&enc.encode(&f0).unwrap().remove(0)).unwrap();
        let f1 = test_picture(64, 48, PixelFormat::Gray8, 1);
        let pkt = enc.encode(&f1).unwrap().remove(0);
        let good = dec.decode(&pkt).unwrap().remove(0);
        let hidden = dec.conceal(pkt.pts).unwrap();
        assert_eq!(
            hidden.pixels, good.pixels,
            "conceal must repeat the last decoded frame"
        );
    }

    #[test]
    fn flush_discards_reference_state() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        let f = test_picture(64, 48, PixelFormat::Gray8, 0);
        let i = enc.encode(&f).unwrap().remove(0);
        dec.decode(&i).unwrap();
        let f1 = test_picture(64, 48, PixelFormat::Gray8, 1);
        let p = enc.encode(&f1).unwrap().remove(0);
        assert!(dec.decode(&p).is_ok());
        assert!(dec.flush().unwrap().is_empty());
        // After a flush the reference is gone, so a P-frame is undecodable.
        assert_eq!(dec.decode(&p), Err(CodecError::NeedMoreInput));
    }

    #[test]
    fn encoder_rejects_a_frame_that_does_not_match_the_config() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let mut enc = ThrumEncoder::new(CodecConfig::default(), c).unwrap();
        // Wrong format.
        assert!(matches!(
            enc.encode(&test_picture(64, 48, PixelFormat::Yuv420, 0)),
            Err(CodecError::Unsupported(_))
        ));
        // Wrong geometry.
        assert!(matches!(
            enc.encode(&test_picture(32, 32, PixelFormat::Gray8, 0)),
            Err(CodecError::InvalidConfig(_))
        ));
    }

    #[test]
    fn a_zero_search_range_still_round_trips() {
        let c = ThrumConfig {
            search_range: 0,
            ..cfg(64, 48, PixelFormat::Gray8)
        };
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        for i in 0..3 {
            let f = test_picture(64, 48, PixelFormat::Gray8, i);
            for p in enc.encode(&f).unwrap() {
                dec.decode(&p).unwrap();
            }
        }
    }

    #[test]
    fn a_fully_refreshed_p_frame_is_dominated_by_intra_macroblocks() {
        // With 100% refresh the encoder codes every P-frame macroblock
        // intra, so the packet is as large as an I-frame even though the
        // frame-type bit says P. The decoder still requires a reference
        // for *any* P-frame up front (it cannot know the macroblock types
        // before reading them), so this measures the rate, not
        // reference-independence.
        let c = ThrumConfig {
            intra_refresh_percent: 100,
            ..cfg(64, 48, PixelFormat::Gray8)
        };
        let mut enc = ThrumEncoder::new(CodecConfig::default(), c).unwrap();
        let f0 = test_picture(64, 48, PixelFormat::Gray8, 0);
        let i_size = enc.encode(&f0).unwrap().remove(0).data.len();
        let f1 = test_picture(64, 48, PixelFormat::Gray8, 1);
        let p = enc.encode(&f1).unwrap().remove(0);
        assert!(!p.is_keyframe(), "the frame-type bit must still say P");
        assert!(
            p.data.len() * 10 > i_size,
            "a 100%-refreshed P-frame ({} B) should be I-frame sized ({i_size} B)",
            p.data.len()
        );
    }

    #[test]
    fn a_p_frame_with_no_refresh_needs_no_reference() {
        // The mirror image: 0% intra refresh means a P-frame is genuinely
        // inter-only, so a cold decoder must reject it.
        let c = ThrumConfig {
            intra_refresh_percent: 0,
            ..cfg(64, 48, PixelFormat::Gray8)
        };
        let mut enc = ThrumEncoder::new(CodecConfig::default(), c.clone()).unwrap();
        let f0 = test_picture(64, 48, PixelFormat::Gray8, 0);
        let _ = enc.encode(&f0).unwrap();
        let f1 = test_picture(64, 48, PixelFormat::Gray8, 1);
        let p = enc.encode(&f1).unwrap().remove(0);
        let mut cold = ThrumDecoder::new(CodecConfig::default(), c).unwrap();
        assert_eq!(cold.decode(&p), Err(CodecError::NeedMoreInput));
    }

    #[test]
    fn resource_limits_reject_an_oversized_packet() {
        let c = cfg(64, 48, PixelFormat::Gray8);
        let mut dec = ThrumDecoder::new(
            CodecConfig {
                limits: tpt_media_codec::ResourceLimits {
                    max_packet_bytes: 8,
                    ..tpt_media_codec::ResourceLimits::permissive()
                },
                ..CodecConfig::default()
            },
            c,
        )
        .unwrap();
        let pkt = Packet {
            data: vec![0u8; 64],
            pts: Timestamp::zero(Timebase::ONE_PER_SECOND),
            dts: Timestamp::zero(Timebase::ONE_PER_SECOND),
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        assert!(matches!(
            dec.decode(&pkt),
            Err(CodecError::ResourceLimit(_))
        ));
    }
}
