//! `tpt-video-mote` - Mote: TPT extreme edge / IoT video codec.
//!
//! ## Design (v0.1)
//!
//! Mote is the small end of the native video family: a **dictionary
//! (vector-quantisation) codec** for low-resolution, mostly-static
//! camera feeds on constrained devices. A frame is cut into 8x8 *patches*;
//! each patch is replaced by the index of its nearest entry in a shared
//! **dictionary** of up to 4096 patches (a 12-bit index at full size).
//! There is no transform, no motion search, no entropy-coder state: the
//! decoder is a table lookup and a copy, which is the point.
//!
//! - **Supported dimensions** (todo.md 34): `Gray8` or planar `Yuv420`;
//!   width and height non-zero multiples of 16, with the long edge at most
//!   [`MAX_LONG_EDGE`] (848) and the short edge at most [`MAX_SHORT_EDGE`]
//!   (480) - "<=480p", in either orientation. Multiples of 16 make every
//!   plane (including 4:2:0 chroma) tile exactly into 8x8 patches, so
//!   there is no padding path. Anything else is refused with
//!   [`CodecError::InvalidConfig`] / [`CodecError::Unsupported`], never
//!   silently converted.
//! - **Patch representation**: an 8x8 block of 8-bit samples (dimension
//!   [`PATCH_DIM`] = 64) taken from one plane. Patches are numbered
//!   plane by plane (Y, then U, then V), raster order inside a plane.
//!   All planes share **one** dictionary.
//! - **Dictionary**: 1..=[`MAX_DICTIONARY_ENTRIES`] entries of 64 bytes,
//!   index width `ceil(log2(entries))` bits ([`index_bits`]): 12 bits at
//!   the 4096-entry target. See [`Dictionary`] for training and search.
//!   The dictionary is stream state, not per-frame data: it is sent once
//!   (a *dictionary packet*) or provisioned out of band
//!   ([`MoteDecoder::with_dictionary`]), and its 16-bit id rides in every
//!   frame packet so a mismatch is rejected rather than rendered.
//! - **VQ**: nearest-entry search under squared error, delegated to
//!   `tpt_dsp_core::nearest_vector_accelerated` (todo.md 8). Distances are
//!   exact integers in `f32`, so the encoder is bit-reproducible.
//! - **Reconstruction model**: the decoded picture is the concatenation
//!   of dictionary entries, with no post-filter. A *skipped* patch keeps
//!   the previous decoded patch bit-for-bit. Encoder and decoder run the
//!   identical copy, so they cannot drift.
//! - **Temporal prediction** (investigated; see docs/codec-design.md):
//!   conditional replenishment with a single reference frame. A delta
//!   frame codes only the patches whose previous reconstruction is worse
//!   than the dictionary can do (or above `skip_mse`), and signals them
//!   as a Rice-coded skip-run followed by the index. No motion
//!   compensation: the codec targets static cameras.
//!
//! ## Bitstream (version 1)
//!
//! Every packet is `body ++ CRC-32(body) (LE, 4 bytes)`; the CRC is
//! verified before any other parsing (todo.md 35). The body is MSB-first
//! bits:
//!
//! ```text
//! common:  version:16  type:2          type 0 dictionary, 1 key, 2 delta
//! dict:    entries-1:12  pad:2 (=0)  entries*64 bytes (byte aligned)
//! key:     dict_id:16   then  patches * index_bits
//! delta:   dict_id:16  k:3  count:16   then count * ( rice_k(run)  index )
//! ```
//!
//! `run` is the number of skipped patches since the previous coded patch
//! (or the frame start); trailing skips are implicit. The encoder picks
//! `k` in `0..=7` to minimise the frame's run cost.
//!
//! ## Provable size bounds
//!
//! [`MoteConfig::max_packet_bits`] derives the worst-case frame packet
//! from the geometry and dictionary size; the encoder allocates exactly
//! that once. See its docs for the derivation.
//!
//! ## Hostile input
//!
//! The decoder verifies the CRC first, honours `ResourceLimits`
//! (packet size, frame bytes, allocation, reference frames), bounds every
//! loop by the patch count or the packet length, validates every index
//! against the dictionary size, and commits state only after a packet
//! decodes fully - a rejected packet never corrupts the reference.
//!
//! ## Determinism (todo.md 39)
//!
//! Pure function of (frame bytes, dictionary, config). Training is seeded
//! integer Lloyd with no hash-order dependence.
//!
//! See `docs/codec-design.md` (Mote section), `docs/provenance.md` and
//! `docs/performance.md` for design, algorithm sources and measurements.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use tpt_dsp_core::{nearest_vector_accelerated, DistanceMetric};
use tpt_media_bitstream::{BitReader, BitWriter};
use tpt_media_codec::{
    Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, Decoder, DynDecoder,
    DynDecoderFactory, DynEncoder, DynEncoderFactory, Encoder, SupportedFormat,
};
use tpt_media_core::{Frame, Packet, PacketFlags, PixelFormat, Timestamp, VideoFrame};

mod dictionary;
mod picture;
mod quality;
#[cfg(test)]
mod tests;

pub use dictionary::{index_bits, Dictionary, TrainConfig};
pub use picture::is_coded_format;
#[cfg(feature = "std")]
pub use quality::psnr_from_mse;
pub use quality::{compare, QualityReport, PSNR_CAP_DB};

use dictionary::sse;
use picture::{validate_dimensions, PatchGrid, Picture};

/// Codec id for Mote.
pub const CODEC_ID: [u8; 4] = *b"MOTE";

/// Bitstream version.
pub const VERSION: u16 = 1;

/// Size in bytes of the per-packet CRC-32 trailer.
pub const CRC_TRAILER_BYTES: usize = 4;

/// Edge length of a patch, in samples.
pub const PATCH: usize = 8;

/// Samples per patch (the VQ dimension).
pub const PATCH_DIM: usize = PATCH * PATCH;

/// Largest dictionary, and the initial-target size (todo.md 34).
pub const MAX_DICTIONARY_ENTRIES: usize = 4096;

/// Index width at [`MAX_DICTIONARY_ENTRIES`]: the 12-bit codebook index.
pub const MAX_INDEX_BITS: u32 = 12;

/// Longest supported frame edge (480p is 848x480 rounded down to a
/// multiple of 16).
pub const MAX_LONG_EDGE: usize = 848;

/// Longest supported *short* frame edge.
pub const MAX_SHORT_EDGE: usize = 480;

/// Default I-frame interval in frames (0 = every frame is a key frame).
/// Long by design: a key frame codes every patch, so on a static scene
/// it costs orders of magnitude more than a delta frame.
pub const KEYFRAME_INTERVAL: u64 = 120;

/// Default skip threshold: per-sample mean squared error (grey levels
/// squared) at or below which a patch is not replaced.
pub const DEFAULT_SKIP_MSE: u16 = 4;

/// Bits of the packet-type field.
const TYPE_BITS: u32 = 2;
const TYPE_DICTIONARY: u64 = 0;
const TYPE_KEY: u64 = 1;
const TYPE_DELTA: u64 = 2;
/// Bits of the dictionary id carried by frame packets.
const ID_BITS: u32 = 16;
/// Bits of the Rice parameter selector, and its largest value.
const RICE_K_BITS: u32 = 3;
const RICE_K_MAX: u32 = 7;
/// Bits of the delta frame's coded-patch count.
const COUNT_BITS: u32 = 16;
/// Header bits of a dictionary packet (version, type, entries-1, pad).
const DICT_HEADER_BITS: usize = 16 + TYPE_BITS as usize + 12 + 2;

/// CRC-32 (IEEE 802.3, reflected `0xEDB88320`), the family's per-packet
/// integrity convention, kept local so the codec layer does not depend on
/// the container crate.
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

/// The CRC-32 used for Mote's per-packet trailer, exposed so tests can
/// build well-formed hostile packets.
pub fn packet_crc(bytes: &[u8]) -> u32 {
    crc32(bytes)
}

/// Rice cost in bits of `value` with parameter `k` (unsigned).
#[inline]
fn rice_cost(value: u32, k: u32) -> u64 {
    u64::from(value >> k) + 1 + u64::from(k)
}

fn write_rice(w: &mut BitWriter, k: u32, value: u32) -> CodecResult<()> {
    let mut q = value >> k;
    while q > 0 {
        let n = q.min(32);
        w.write(n, 0).map_err(|_| CodecError::Internal)?;
        q -= n;
    }
    w.write(1, 1).map_err(|_| CodecError::Internal)?;
    if k > 0 {
        w.write(k, u64::from(value & ((1u32 << k) - 1)))
            .map_err(|_| CodecError::Internal)?;
    }
    Ok(())
}

/// Read one Rice code. `max_value` bounds the decoded value so a hostile
/// unary prefix cannot spin.
fn read_rice(r: &mut BitReader, k: u32, max_value: u32) -> CodecResult<u32> {
    let cap = max_value >> k;
    let mut q = 0u32;
    while r.read(1).map_err(|_| CodecError::MalformedPacket)? == 0 {
        q += 1;
        if q > cap {
            return Err(CodecError::MalformedPacket);
        }
    }
    let rem = if k > 0 {
        r.read(k).map_err(|_| CodecError::MalformedPacket)? as u32
    } else {
        0
    };
    Ok((q << k) | rem)
}

// --- configuration ----------------------------------------------------------

/// Mote configuration.
///
/// Frame geometry is part of the configuration (and of the track header)
/// rather than of every packet, so a delta packet stays tiny. Only
/// `format` is carried in the codec-private bytes; the remaining fields
/// are encoder policy the decoder does not need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoteConfig {
    /// Coded frame width in luma pixels (multiple of 16, see crate docs).
    pub width: u32,
    /// Coded frame height in luma pixels (multiple of 16, see crate docs).
    pub height: u32,
    /// Coded pixel format: [`PixelFormat::Gray8`] or [`PixelFormat::Yuv420`].
    pub format: PixelFormat,
    /// Encoder only: per-sample MSE at or below which a patch is skipped
    /// in a delta frame. `0` skips only bit-identical patches.
    pub skip_mse: u16,
    /// Encoder only: enable delta frames (temporal conditional
    /// replenishment). When false every frame is a key frame.
    pub temporal: bool,
    /// Encoder only: target dictionary size when the encoder must train
    /// its own (`1..=4096`).
    pub dictionary_entries: u16,
    /// Encoder only: Lloyd iterations for self-training.
    pub train_iterations: u32,
    /// Encoder only: training seed for self-training.
    pub train_seed: u64,
}

impl Default for MoteConfig {
    fn default() -> Self {
        let t = TrainConfig::default();
        MoteConfig {
            width: 320,
            height: 240,
            format: PixelFormat::Gray8,
            skip_mse: DEFAULT_SKIP_MSE,
            temporal: true,
            dictionary_entries: MAX_DICTIONARY_ENTRIES as u16,
            train_iterations: t.iterations,
            train_seed: t.seed,
        }
    }
}

impl MoteConfig {
    /// Number of codec-private bytes [`MoteConfig::to_codec_private`]
    /// produces.
    pub const CODEC_PRIVATE_LEN: usize = 1;

    /// Number of 8x8 patches in one frame of this geometry.
    pub fn patch_count(&self) -> usize {
        PatchGrid::new(self.width as usize, self.height as usize, self.format).total()
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

    /// Upper bound, in bits, on one frame packet's body (key or delta)
    /// for a dictionary of `dictionary_entries`.
    ///
    /// Provable rather than estimated. With `T` patches and index width
    /// `b = index_bits(entries)`:
    ///
    /// - a key frame is `16 + 2 + 16` header bits plus exactly `T * b`;
    /// - a delta frame is `16 + 2 + 16 + 3 + 16` header bits plus, per
    ///   coded patch, one index (`b`) and one Rice run. Each run is at
    ///   most `T - 1`, the encoder picks the `k` that minimises the
    ///   frame's total, so that total is at most the total at `k = 7`,
    ///   and each run costs at most `((T-1) >> 7) + 1 + 7` bits there.
    ///   At most `T` patches are coded.
    ///
    /// The bound is the larger of the two. (The CRC trailer is extra.)
    pub fn max_packet_bits(&self, dictionary_entries: usize) -> usize {
        let t = self.patch_count();
        let b = index_bits(dictionary_entries.max(1)) as usize;
        let key = 16 + TYPE_BITS as usize + ID_BITS as usize + t * b;
        let run_max = ((t.saturating_sub(1)) >> RICE_K_MAX) + 1 + RICE_K_MAX as usize;
        let delta = 16
            + TYPE_BITS as usize
            + ID_BITS as usize
            + RICE_K_BITS as usize
            + COUNT_BITS as usize
            + t * (run_max + b);
        key.max(delta)
    }

    /// Exact size in bytes (CRC included) of a dictionary packet for
    /// `entries` entries.
    pub fn dictionary_packet_bytes(entries: usize) -> usize {
        (DICT_HEADER_BITS / 8) + entries * PATCH_DIM + CRC_TRAILER_BYTES
    }

    fn validate(&self) -> CodecResult<()> {
        if !is_coded_format(self.format) {
            return Err(CodecError::InvalidConfig(
                "mote supports only Gray8 and Yuv420",
            ));
        }
        validate_dimensions(self.width, self.height)?;
        if self.dictionary_entries == 0
            || usize::from(self.dictionary_entries) > MAX_DICTIONARY_ENTRIES
        {
            return Err(CodecError::InvalidConfig(
                "mote dictionary_entries must be 1..=4096",
            ));
        }
        Ok(())
    }

    /// Encode the codec-side config into the track's private bytes
    /// (just the pixel format; geometry lives in the track header).
    pub fn to_codec_private(&self) -> Vec<u8> {
        vec![self.format as u8]
    }

    /// Inverse of [`MoteConfig::to_codec_private`], with geometry from
    /// the track header.
    pub fn from_codec_private(width: u32, height: u32, private: &[u8]) -> CodecResult<MoteConfig> {
        let mut cfg = MoteConfig {
            width,
            height,
            ..MoteConfig::default()
        };
        if private.len() >= MoteConfig::CODEC_PRIVATE_LEN {
            cfg.format = match private[0] {
                0 => PixelFormat::Gray8,
                3 => PixelFormat::Yuv420,
                _ => return Err(CodecError::InvalidConfig("mote pixel format")),
            };
        }
        cfg.validate()?;
        Ok(cfg)
    }
}

/// Dictionary size a self-training encoder uses: the configured target,
/// clamped so the dictionary is well below the number of training
/// patches (a dictionary as large as its training set just memorises the
/// frame), and never below 2.
pub fn self_trained_entries(target: usize, training_patches: usize) -> usize {
    target
        .min((training_patches / 4).max(2))
        .min(training_patches)
        .clamp(1, MAX_DICTIONARY_ENTRIES)
}

fn static_info(width: u32, height: u32, format: PixelFormat, caps: Capabilities) -> CodecInfo {
    CodecInfo {
        name: "Mote",
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

fn capabilities() -> Capabilities {
    Capabilities {
        // Decode is a table lookup per patch; the encoder's VQ search is
        // not claimed real-time.
        real_time_decoder: true,
        lossy: true,
        ..Capabilities::default()
    }
}

fn collect_patches(pic: &Picture, grid: &PatchGrid) -> Vec<u8> {
    let mut out = Vec::with_capacity(grid.total() * PATCH_DIM);
    let mut p = [0u8; PATCH_DIM];
    for g in 0..grid.total() {
        grid.load(pic, g, &mut p);
        out.extend_from_slice(&p);
    }
    out
}

// --- encoder ----------------------------------------------------------------

/// Mote encoder.
///
/// Holds the dictionary, its `f32` search copy, and the reconstruction of
/// the previous frame (the delta-frame reference).
pub struct MoteEncoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: MoteConfig,
    grid: PatchGrid,
    dict: Option<Dictionary>,
    /// `dict` widened to `f32` for the search (built with the dictionary).
    dict_f32: Vec<f32>,
    /// Whether the dictionary still has to be sent in-band.
    embed_dictionary: bool,
    dict_pending: bool,
    reference: Option<Picture>,
    /// Coded patches of the frame being built: `(run, index)`.
    coded: Vec<(u32, u16)>,
    /// Reused worst-case packet buffer (see `encode`).
    packet_buf: Vec<u8>,
    frames: u64,
    keyframe_interval: u64,
}

impl MoteEncoder {
    /// Build an encoder that trains its own dictionary on the first
    /// frame it sees and sends it in-band. See [`self_trained_entries`]
    /// for why that dictionary is smaller than `dictionary_entries` on a
    /// small frame; for the full 4096-entry target, train offline
    /// ([`Dictionary::train_from_frames`]) and use
    /// [`MoteEncoder::with_dictionary`].
    pub fn new(codec_cfg: CodecConfig, enc: MoteConfig) -> CodecResult<Self> {
        MoteEncoder::build(codec_cfg, enc, None, true, KEYFRAME_INTERVAL)
    }

    /// Like [`MoteEncoder::new`] with an explicit key-frame interval
    /// (`0` = every frame is a key frame).
    pub fn with_keyframe_interval(
        codec_cfg: CodecConfig,
        enc: MoteConfig,
        keyframe_interval: u64,
    ) -> CodecResult<Self> {
        MoteEncoder::build(codec_cfg, enc, None, true, keyframe_interval)
    }

    /// Build an encoder around a supplied (typically offline-trained)
    /// dictionary. With `embed = true` the first frame is preceded by a
    /// dictionary packet; with `false` the dictionary is assumed to be
    /// provisioned at the decoder out of band (its id is still checked).
    pub fn with_dictionary(
        codec_cfg: CodecConfig,
        enc: MoteConfig,
        dict: Dictionary,
        embed: bool,
    ) -> CodecResult<Self> {
        MoteEncoder::build(codec_cfg, enc, Some(dict), embed, KEYFRAME_INTERVAL)
    }

    /// [`MoteEncoder::with_dictionary`] with an explicit key-frame interval.
    pub fn with_dictionary_and_keyframe_interval(
        codec_cfg: CodecConfig,
        enc: MoteConfig,
        dict: Dictionary,
        embed: bool,
        keyframe_interval: u64,
    ) -> CodecResult<Self> {
        MoteEncoder::build(codec_cfg, enc, Some(dict), embed, keyframe_interval)
    }

    fn build(
        codec_cfg: CodecConfig,
        enc: MoteConfig,
        dict: Option<Dictionary>,
        embed: bool,
        keyframe_interval: u64,
    ) -> CodecResult<Self> {
        enc.validate()?;
        let info = static_info(enc.width, enc.height, enc.format, capabilities());
        let grid = PatchGrid::new(enc.width as usize, enc.height as usize, enc.format);
        let mut e = MoteEncoder {
            info,
            cfg: codec_cfg,
            enc,
            grid,
            dict: None,
            dict_f32: Vec::new(),
            embed_dictionary: embed,
            dict_pending: embed,
            reference: None,
            coded: Vec::new(),
            packet_buf: Vec::new(),
            frames: 0,
            keyframe_interval,
        };
        if let Some(d) = dict {
            e.install_dictionary(d)?;
        }
        Ok(e)
    }

    fn install_dictionary(&mut self, d: Dictionary) -> CodecResult<()> {
        self.cfg.limits.check_allocation(d.len() * PATCH_DIM * 5)?;
        self.dict_f32 = d.to_f32();
        self.packet_buf =
            vec![0u8; self.enc.max_packet_bits(d.len()).div_ceil(8) + CRC_TRAILER_BYTES];
        self.dict = Some(d);
        Ok(())
    }

    /// The dictionary in use, once one exists (immediately for
    /// [`MoteEncoder::with_dictionary`], after the first frame otherwise).
    pub fn dictionary(&self) -> Option<&Dictionary> {
        self.dict.as_ref()
    }

    /// Whether the next frame will be coded as a key frame.
    pub fn next_is_keyframe(&self) -> bool {
        !self.enc.temporal
            || self.reference.is_none()
            || self.keyframe_interval == 0
            || self.frames % self.keyframe_interval == 0
    }

    /// Build a dictionary packet for the current dictionary, e.g. to let
    /// a late-joining receiver catch up. `None` before a dictionary
    /// exists.
    pub fn dictionary_packet(&self, pts: Timestamp) -> Option<Packet> {
        let d = self.dict.as_ref()?;
        Some(Packet {
            data: encode_dictionary_packet(d),
            pts,
            dts: pts,
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        })
    }
}

/// Serialise a dictionary packet (CRC included).
fn encode_dictionary_packet(d: &Dictionary) -> Vec<u8> {
    let mut buf = vec![0u8; MoteConfig::dictionary_packet_bytes(d.len())];
    let body_len = buf.len() - CRC_TRAILER_BYTES;
    {
        let mut w = BitWriter::new(&mut buf[..body_len]);
        // The writes cannot fail: the buffer is sized exactly.
        let _ = w.write(16, u64::from(VERSION));
        let _ = w.write(TYPE_BITS, TYPE_DICTIONARY);
        let _ = w.write(12, (d.len() - 1) as u64);
        let _ = w.write(2, 0);
        for &b in d.as_bytes() {
            let _ = w.write(8, u64::from(b));
        }
        let _ = w.finish();
    }
    let crc = crc32(&buf[..body_len]);
    buf[body_len..].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// Choose each patch's code for one frame.
///
/// Key frames code every patch. Delta frames skip a patch when its
/// previous reconstruction is within `skip_sse`, or is at least as good
/// as the best dictionary entry (coding it would cost bits and buy
/// nothing). `recon` is updated exactly as the decoder will update its.
#[allow(clippy::too_many_arguments)]
fn code_frame(
    grid: &PatchGrid,
    dict: &Dictionary,
    dict_f32: &[f32],
    src: &Picture,
    recon: &mut Picture,
    intra: bool,
    skip_sse: u32,
    coded: &mut Vec<(u32, u16)>,
) {
    coded.clear();
    let mut s = [0u8; PATCH_DIM];
    let mut sf = [0f32; PATCH_DIM];
    let mut prev = [0u8; PATCH_DIM];
    let mut run = 0u32;
    for g in 0..grid.total() {
        grid.load(src, g, &mut s);
        if !intra {
            grid.load(recon, g, &mut prev);
            let e_skip = sse(&s, &prev);
            if e_skip <= skip_sse {
                run += 1;
                continue;
            }
            for (f, &b) in sf.iter_mut().zip(s.iter()) {
                *f = f32::from(b);
            }
            let (idx, d) = nearest_vector_accelerated(
                dict_f32,
                PATCH_DIM,
                &sf,
                DistanceMetric::SquaredEuclidean,
            );
            if e_skip <= d as u32 {
                run += 1;
                continue;
            }
            grid.store(recon, g, dict.entry(idx));
            coded.push((run, idx as u16));
            run = 0;
        } else {
            for (f, &b) in sf.iter_mut().zip(s.iter()) {
                *f = f32::from(b);
            }
            let (idx, _) = nearest_vector_accelerated(
                dict_f32,
                PATCH_DIM,
                &sf,
                DistanceMetric::SquaredEuclidean,
            );
            grid.store(recon, g, dict.entry(idx));
            coded.push((0, idx as u16));
        }
    }
}

impl Encoder for MoteEncoder {
    type Frame = VideoFrame;

    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn encode(&mut self, frame: &VideoFrame) -> CodecResult<Vec<Packet>> {
        if frame.format != self.enc.format {
            return Err(CodecError::Unsupported("mote pixel format"));
        }
        if frame.width != self.enc.width || frame.height != self.enc.height {
            return Err(CodecError::InvalidConfig(
                "mote frame dimensions do not match the configured geometry",
            ));
        }
        self.cfg.limits.check_frame_bytes(self.enc.frame_bytes())?;
        self.cfg
            .limits
            .check_allocation(self.enc.frame_bytes() * 2)?;
        let src = Picture::from_frame(frame)?;

        // No dictionary yet: train one on this very frame.
        if self.dict.is_none() {
            let patches = collect_patches(&src, &self.grid);
            let n = patches.len() / PATCH_DIM;
            let tc = TrainConfig {
                entries: self_trained_entries(usize::from(self.enc.dictionary_entries), n),
                iterations: self.enc.train_iterations,
                seed: self.enc.train_seed,
                ..TrainConfig::default()
            };
            let d = Dictionary::train_patches(&patches, &tc)?;
            self.install_dictionary(d)?;
        }

        let mut packets = Vec::with_capacity(2);
        if self.dict_pending && self.embed_dictionary {
            if let Some(p) = self.dictionary_packet(frame.frame.pts) {
                packets.push(p);
            }
        }
        self.dict_pending = false;

        let intra = self.next_is_keyframe();
        let dict = self.dict.as_ref().ok_or(CodecError::Internal)?;
        let mut recon = match (&self.reference, intra) {
            (Some(r), false) => r.clone(),
            _ => Picture::new(src.width, src.height, src.format, 128),
        };
        let skip_sse = u32::from(self.enc.skip_mse) * PATCH_DIM as u32;
        code_frame(
            &self.grid,
            dict,
            &self.dict_f32,
            &src,
            &mut recon,
            intra,
            skip_sse,
            &mut self.coded,
        );

        // Serialise into the reused worst-case buffer (sized by the
        // provable bound in `max_packet_bits`, so it cannot overflow).
        let ib = dict.index_bits();
        let total = self.grid.total();
        let mut buf = core::mem::take(&mut self.packet_buf);
        debug_assert!(
            buf.len() >= self.enc.max_packet_bits(dict.len()).div_ceil(8) + CRC_TRAILER_BYTES
        );
        let written = {
            let mut w = BitWriter::new(&mut buf);
            let map = |_| CodecError::Internal;
            w.write(16, u64::from(VERSION)).map_err(map)?;
            w.write(TYPE_BITS, if intra { TYPE_KEY } else { TYPE_DELTA })
                .map_err(map)?;
            w.write(ID_BITS, u64::from(dict.id())).map_err(map)?;
            if intra {
                debug_assert_eq!(self.coded.len(), total);
                for &(_, idx) in &self.coded {
                    w.write(ib, u64::from(idx)).map_err(map)?;
                }
            } else {
                // Cheapest Rice parameter for this frame's runs.
                let mut best_k = 0u32;
                let mut best_cost = u64::MAX;
                for k in 0..=RICE_K_MAX {
                    let cost: u64 = self.coded.iter().map(|&(run, _)| rice_cost(run, k)).sum();
                    if cost < best_cost {
                        best_cost = cost;
                        best_k = k;
                    }
                }
                w.write(RICE_K_BITS, u64::from(best_k)).map_err(map)?;
                w.write(COUNT_BITS, self.coded.len() as u64).map_err(map)?;
                for &(run, idx) in &self.coded {
                    write_rice(&mut w, best_k, run)?;
                    w.write(ib, u64::from(idx)).map_err(map)?;
                }
            }
            w.finish().map_err(map)?
        };
        let crc = crc32(&buf[..written]);
        let mut data = Vec::with_capacity(written + CRC_TRAILER_BYTES);
        data.extend_from_slice(&buf[..written]);
        data.extend_from_slice(&crc.to_le_bytes());
        self.packet_buf = buf;

        self.reference = Some(recon);
        self.frames += 1;

        packets.push(Packet {
            data,
            pts: frame.frame.pts,
            dts: frame.frame.pts,
            flags: PacketFlags::END_OF_FRAME
                | if intra {
                    PacketFlags::KEYFRAME
                } else {
                    PacketFlags::empty()
                },
            track_id: 0,
            duration: None,
        });
        Ok(packets)
    }
}

// --- decoder ----------------------------------------------------------------

/// Mote decoder.
///
/// Holds the dictionary (from a dictionary packet or provisioned via
/// [`MoteDecoder::with_dictionary`]), one reference picture, and a copy
/// of the last emitted frame for [`MoteDecoder::conceal`]. One frame
/// packet yields one frame; a dictionary packet yields none.
pub struct MoteDecoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: MoteConfig,
    grid: PatchGrid,
    dict: Option<Dictionary>,
    reference: Option<Picture>,
}

impl MoteDecoder {
    /// Number of reference pictures this decoder retains.
    pub const REFERENCE_FRAMES: u8 = 1;

    /// Build a decoder that learns its dictionary from the stream.
    pub fn new(codec_cfg: CodecConfig, enc: MoteConfig) -> CodecResult<Self> {
        enc.validate()?;
        let info = static_info(enc.width, enc.height, enc.format, capabilities());
        let grid = PatchGrid::new(enc.width as usize, enc.height as usize, enc.format);
        Ok(MoteDecoder {
            info,
            cfg: codec_cfg,
            enc,
            grid,
            dict: None,
            reference: None,
        })
    }

    /// Build a decoder with a dictionary provisioned out of band.
    pub fn with_dictionary(
        codec_cfg: CodecConfig,
        enc: MoteConfig,
        dict: Dictionary,
    ) -> CodecResult<Self> {
        let mut d = MoteDecoder::new(codec_cfg, enc)?;
        d.cfg.limits.check_allocation(dict.memory_bytes())?;
        d.dict = Some(dict);
        Ok(d)
    }

    /// The dictionary currently held, if any.
    pub fn dictionary(&self) -> Option<&Dictionary> {
        self.dict.as_ref()
    }

    /// Packet-loss concealment: repeat the last decoded frame; mid-grey
    /// before the first frame.
    pub fn conceal(&mut self, pts: Timestamp) -> CodecResult<VideoFrame> {
        let pic = match &self.reference {
            Some(p) => p.clone(),
            None => Picture::new(
                self.enc.width as usize,
                self.enc.height as usize,
                self.enc.format,
                128,
            ),
        };
        Ok(pic.to_frame(pts))
    }

    fn decode_dictionary(&mut self, r: &mut BitReader, body_len: usize) -> CodecResult<()> {
        let n = r.read(12).map_err(|_| CodecError::MalformedPacket)? as usize + 1;
        if r.read(2).map_err(|_| CodecError::MalformedPacket)? != 0 {
            return Err(CodecError::MalformedPacket);
        }
        // Reject a body that cannot hold `n` entries before allocating
        // anything sized by the attacker-controlled count.
        if body_len != DICT_HEADER_BITS / 8 + n * PATCH_DIM {
            return Err(CodecError::MalformedPacket);
        }
        self.cfg.limits.check_allocation(n * PATCH_DIM)?;
        let mut bytes = Vec::with_capacity(n * PATCH_DIM);
        for _ in 0..n * PATCH_DIM {
            bytes.push(r.read(8).map_err(|_| CodecError::MalformedPacket)? as u8);
        }
        self.dict = Some(Dictionary::from_entries(bytes)?);
        Ok(())
    }

    fn decode_frame(
        &mut self,
        r: &mut BitReader,
        intra: bool,
        pts: Timestamp,
    ) -> CodecResult<VideoFrame> {
        let dict = self.dict.as_ref().ok_or(CodecError::NeedMoreInput)?;
        let id = r.read(ID_BITS).map_err(|_| CodecError::MalformedPacket)? as u16;
        if id != dict.id() {
            return Err(CodecError::Unsupported("mote dictionary id mismatch"));
        }
        if !intra && self.reference.is_none() {
            return Err(CodecError::NeedMoreInput);
        }
        let total = self.grid.total();
        let (ib, n) = (dict.index_bits(), dict.len());
        let mut recon = match (&self.reference, intra) {
            (Some(p), false) => p.clone(),
            _ => Picture::new(
                self.enc.width as usize,
                self.enc.height as usize,
                self.enc.format,
                128,
            ),
        };
        if intra {
            for g in 0..total {
                let idx = r.read(ib).map_err(|_| CodecError::MalformedPacket)? as usize;
                if idx >= n {
                    return Err(CodecError::MalformedPacket);
                }
                self.grid.store(&mut recon, g, dict.entry(idx));
            }
        } else {
            let k = r
                .read(RICE_K_BITS)
                .map_err(|_| CodecError::MalformedPacket)? as u32;
            let count = r
                .read(COUNT_BITS)
                .map_err(|_| CodecError::MalformedPacket)? as usize;
            if count > total {
                return Err(CodecError::MalformedPacket);
            }
            let mut pos = 0usize;
            for _ in 0..count {
                let run = read_rice(r, k, total as u32)? as usize;
                pos += run;
                if pos >= total {
                    return Err(CodecError::MalformedPacket);
                }
                let idx = r.read(ib).map_err(|_| CodecError::MalformedPacket)? as usize;
                if idx >= n {
                    return Err(CodecError::MalformedPacket);
                }
                self.grid.store(&mut recon, pos, dict.entry(idx));
                pos += 1;
            }
        }
        let frame = recon.to_frame(pts);
        self.reference = Some(recon);
        Ok(frame)
    }
}

impl Decoder for MoteDecoder {
    type Frame = VideoFrame;

    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<VideoFrame>> {
        self.cfg.limits.check_packet(packet.data.len())?;
        self.cfg
            .limits
            .check_reference_frames(MoteDecoder::REFERENCE_FRAMES)?;
        self.cfg.limits.check_frame_bytes(self.enc.frame_bytes())?;
        self.cfg
            .limits
            .check_allocation(self.enc.frame_bytes() * 2)?;

        if packet.data.len() < CRC_TRAILER_BYTES {
            return Err(CodecError::MalformedPacket);
        }
        let (body, checksum) = packet.data.split_at(packet.data.len() - CRC_TRAILER_BYTES);
        let expected = u32::from_le_bytes(checksum.try_into().map_err(|_| CodecError::Internal)?);
        if crc32(body) != expected {
            return Err(CodecError::MalformedPacket);
        }

        let mut r = BitReader::new(body);
        let version = r.read(16).map_err(|_| CodecError::MalformedPacket)? as u16;
        if version != VERSION {
            return Err(CodecError::Unsupported("unsupported Mote packet version"));
        }
        let ty = r.read(TYPE_BITS).map_err(|_| CodecError::MalformedPacket)?;
        match ty {
            TYPE_DICTIONARY => {
                self.decode_dictionary(&mut r, body.len())?;
                Ok(Vec::new())
            }
            TYPE_KEY | TYPE_DELTA => {
                let f = self.decode_frame(&mut r, ty == TYPE_KEY, packet.pts)?;
                Ok(vec![f])
            }
            _ => Err(CodecError::MalformedPacket),
        }
    }

    fn flush(&mut self) -> CodecResult<Vec<VideoFrame>> {
        // Drop the reference; keep the dictionary, which is stream
        // configuration rather than decode state.
        self.reference = None;
        Ok(Vec::new())
    }
}

// --- factories and type-erased wrappers -------------------------------------

/// Type-erased decoder factory. Geometry is part of the configuration, so
/// the factory carries it (as every video codec's factory must), plus an
/// optional out-of-band dictionary.
pub struct MoteDecoderFactory {
    enc: MoteConfig,
    dict: Option<Dictionary>,
}

impl MoteDecoderFactory {
    /// Factory whose decoders learn the dictionary from the stream.
    pub fn new(enc: MoteConfig) -> CodecResult<Self> {
        enc.validate()?;
        Ok(MoteDecoderFactory { enc, dict: None })
    }

    /// Factory whose decoders start with a provisioned dictionary.
    pub fn with_dictionary(enc: MoteConfig, dict: Dictionary) -> CodecResult<Self> {
        enc.validate()?;
        Ok(MoteDecoderFactory {
            enc,
            dict: Some(dict),
        })
    }
}

impl DynDecoderFactory for MoteDecoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(
            self.enc.width,
            self.enc.height,
            self.enc.format,
            capabilities(),
        )
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<Box<dyn DynDecoder>> {
        let mut merged = self.enc.clone();
        if cfg.extra.len() >= MoteConfig::CODEC_PRIVATE_LEN {
            merged = MoteConfig::from_codec_private(self.enc.width, self.enc.height, &cfg.extra)?;
        }
        let d = match &self.dict {
            Some(dict) => MoteDecoder::with_dictionary(cfg.clone(), merged, dict.clone())?,
            None => MoteDecoder::new(cfg.clone(), merged)?,
        };
        Ok(Box::new(MoteDynDecoder { inner: d }))
    }
}

/// Type-erased encoder factory; see [`MoteDecoderFactory`].
pub struct MoteEncoderFactory {
    enc: MoteConfig,
    keyframe_interval: u64,
    dict: Option<(Dictionary, bool)>,
}

impl MoteEncoderFactory {
    /// Factory whose encoders self-train and embed their dictionary.
    pub fn new(enc: MoteConfig) -> CodecResult<Self> {
        MoteEncoderFactory::with_keyframe_interval(enc, KEYFRAME_INTERVAL)
    }

    /// As [`MoteEncoderFactory::new`] with an explicit key-frame interval.
    pub fn with_keyframe_interval(enc: MoteConfig, keyframe_interval: u64) -> CodecResult<Self> {
        enc.validate()?;
        Ok(MoteEncoderFactory {
            enc,
            keyframe_interval,
            dict: None,
        })
    }

    /// Factory whose encoders use a supplied dictionary (`embed` chooses
    /// whether it is sent in-band).
    pub fn with_dictionary(enc: MoteConfig, dict: Dictionary, embed: bool) -> CodecResult<Self> {
        enc.validate()?;
        Ok(MoteEncoderFactory {
            enc,
            keyframe_interval: KEYFRAME_INTERVAL,
            dict: Some((dict, embed)),
        })
    }
}

impl DynEncoderFactory for MoteEncoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(
            self.enc.width,
            self.enc.height,
            self.enc.format,
            capabilities(),
        )
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<Box<dyn DynEncoder>> {
        let mut merged = self.enc.clone();
        if cfg.extra.len() >= MoteConfig::CODEC_PRIVATE_LEN {
            merged = MoteConfig::from_codec_private(self.enc.width, self.enc.height, &cfg.extra)?;
        }
        let e = match &self.dict {
            Some((d, embed)) => MoteEncoder::with_dictionary_and_keyframe_interval(
                cfg.clone(),
                merged,
                d.clone(),
                *embed,
                self.keyframe_interval,
            )?,
            None => {
                MoteEncoder::with_keyframe_interval(cfg.clone(), merged, self.keyframe_interval)?
            }
        };
        Ok(Box::new(MoteDynEncoder { inner: e }))
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

/// Type-erased wrapper around [`MoteDecoder`].
pub struct MoteDynDecoder {
    inner: MoteDecoder,
}

impl DynDecoder for MoteDynDecoder {
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

/// Type-erased wrapper around [`MoteEncoder`].
pub struct MoteDynEncoder {
    inner: MoteEncoder,
}

impl DynEncoder for MoteDynEncoder {
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
                return Err(CodecError::Unsupported("mote does not code audio"))
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
