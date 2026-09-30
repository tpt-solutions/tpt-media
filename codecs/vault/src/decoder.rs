//! The Vault decoder.
//!
//! Hostile-input posture (todo.md sections 19, 38, 47):
//!
//! - the packet is size-limited, then CRC-verified, then version-checked
//!   before any further bit is interpreted;
//! - every header field is range-checked; a frame that names more
//!   references than the decoder holds is [`CodecError::NeedMoreInput`],
//!   one that names more than the limits allow is a resource-limit error;
//! - the partition recursion is structurally at most
//!   [`MAX_PARTITION_DEPTH`] + 1 levels (64 -> 8) and is checked against
//!   `limits.max_recursion_depth` up front;
//! - reference indices and motion vectors are range-checked on read;
//!   off-picture vectors are edge-clamped, never indexed;
//! - coefficient walks are bounded by the tile size, levels are clamped;
//! - a failed decode leaves the decoder's reference state untouched, so a
//!   corrupt packet cannot poison the frames after it.

use alloc::boxed::Box;
use alloc::vec::Vec;

use tpt_media_bitstream::BitReader;
use tpt_media_codec::{
    Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, Decoder, DynDecoder,
    DynDecoderFactory,
};
use tpt_media_core::{Packet, Timestamp, VideoFrame};

use crate::filter::{loop_filter, Field, Unit};
use crate::picture::{picture_to_frame, Picture};
use crate::syntax::{
    build_pred, read_leaf, reconstruct_leaf, ref_index_bits, Leaf, LeafCtx, LeafKind,
};
use crate::transform::{Scans, Xform};
use crate::{
    crc32, quant_step, static_info, VaultConfig, CODEC_ID, CRC_TRAILER_BYTES, MAX_PARTITION_DEPTH,
    MAX_QP, MIN_BLOCK, MIN_QP, QP_BITS, SUPERBLOCK, VERSION,
};

/// Vault decoder.
///
/// Keeps up to [`VaultConfig::max_refs`] loop-filtered reference pictures
/// (most recent first) and a copy of the last emitted frame for
/// [`VaultDecoder::conceal`]. There is no reordering: one frame out per
/// packet in.
pub struct VaultDecoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: VaultConfig,
    refs: Vec<Picture>,
    scans: Scans,
    xf: Xform,
    stats: FrameStats,
}

impl VaultDecoder {
    /// Build a decoder for a configuration.
    ///
    /// # Errors
    /// [`CodecError::InvalidConfig`] if `enc` fails validation.
    pub fn new(codec_cfg: CodecConfig, enc: VaultConfig) -> CodecResult<Self> {
        enc.validate()?;
        let info = static_info(
            enc.width,
            enc.height,
            enc.format,
            Capabilities {
                lossy: true,
                ..Capabilities::default()
            },
        );
        Ok(VaultDecoder {
            info,
            cfg: codec_cfg,
            enc,
            refs: Vec::new(),
            scans: Scans::new(),
            xf: Xform::new(),
            stats: FrameStats::default(),
        })
    }

    /// Packet-loss concealment: repeat the last decoded frame (a freeze);
    /// a flat mid-grey picture if nothing has been decoded yet.
    ///
    /// # Errors
    /// Never fails today; the `Result` matches the other codecs' signature.
    pub fn conceal(&mut self, pts: Timestamp) -> CodecResult<VideoFrame> {
        let pic = match self.refs.first() {
            Some(p) => p.clone(),
            None => Picture::new(
                self.enc.width as usize,
                self.enc.height as usize,
                self.enc.format,
                128,
            ),
        };
        Ok(picture_to_frame(&pic, pts))
    }

    /// Number of reference pictures currently held.
    pub fn reference_count(&self) -> usize {
        self.refs.len()
    }

    /// What the most recently decoded frame contained.
    pub fn last_stats(&self) -> &FrameStats {
        &self.stats
    }
}

/// A census of one decoded frame: which coding tools its encoder actually
/// used. Lets tests and benchmarks verify that partitioning, half-pel
/// motion and multiple references are exercised rather than merely present.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Leaf count by edge length `[8, 16, 32, 64]`.
    pub leaves_by_size: [u32; 4],
    /// Skipped leaves.
    pub skip: u32,
    /// Inter (non-skip) leaves.
    pub inter: u32,
    /// Intra leaves.
    pub intra: u32,
    /// Inter or skip leaves whose vector has a half-pel component.
    pub half_pel: u32,
    /// Inter leaves by reference index (skip is always reference 0 and is
    /// not counted here).
    pub ref_hist: [u32; 16],
    /// Leaves with at least one non-zero coefficient.
    pub coded: u32,
}

/// Everything one frame's parse needs.
struct FrameDec<'a> {
    refs: &'a [Picture],
    scans: &'a Scans,
    xf: &'a mut Xform,
    recon: Picture,
    field: Field,
    step: f32,
    ctx: LeafCtx,
    num_refs: usize,
    stats: FrameStats,
}

impl FrameDec<'_> {
    fn block(
        &mut self,
        r: &mut BitReader,
        x: usize,
        y: usize,
        s: usize,
        depth: u8,
    ) -> CodecResult<()> {
        // Structural guarantee, asserted defensively: the recursion is only
        // ever 64 -> 32 -> 16 -> 8.
        if depth > MAX_PARTITION_DEPTH {
            return Err(CodecError::MalformedPacket);
        }
        let inside = x + s <= self.recon.width && y + s <= self.recon.height;
        let split = if s > MIN_BLOCK {
            if inside {
                r.read(1).map_err(|_| CodecError::MalformedPacket)? == 1
            } else {
                true
            }
        } else {
            false
        };
        if split {
            let h = s / 2;
            for (dx, dy) in [(0, 0), (h, 0), (0, h), (h, h)] {
                let (cx, cy) = (x + dx, y + dy);
                if cx >= self.recon.width || cy >= self.recon.height {
                    continue;
                }
                self.block(r, cx, cy, h, depth + 1)?;
            }
            return Ok(());
        }
        let pred_mv = self.field.mv_pred(x / MIN_BLOCK, y / MIN_BLOCK);
        let leaf: Leaf = read_leaf(r, s, &self.ctx, self.num_refs, pred_mv, self.scans)?;
        let np = self.ctx.nplanes;
        let st = &mut self.stats;
        st.leaves_by_size[(s.trailing_zeros() - 3) as usize] += 1;
        match leaf.kind {
            LeafKind::Skip => st.skip += 1,
            LeafKind::Inter => {
                st.inter += 1;
                st.ref_hist[usize::from(leaf.ref_idx) & 15] += 1;
            }
            LeafKind::Intra => st.intra += 1,
        }
        if leaf.kind != LeafKind::Intra && ((leaf.mv.0 | leaf.mv.1) & 1) != 0 {
            st.half_pel += 1;
        }
        if leaf.coded() {
            st.coded += 1;
        }
        let pred = build_pred(
            &self.recon,
            self.refs,
            leaf.kind,
            leaf.ref_idx,
            leaf.mv,
            leaf.mode,
            x,
            y,
            s,
        );
        let recon = reconstruct_leaf(self.xf, &pred, &leaf.coefs, s, np, self.step);
        recon.store(&mut self.recon, x, y, s);
        self.field.set_block(
            x,
            y,
            s,
            Unit {
                intra: leaf.kind == LeafKind::Intra,
                inter: leaf.kind != LeafKind::Intra,
                coded: leaf.coded(),
                ref_idx: leaf.ref_idx,
                mvx: leaf.mv.0 as i16,
                mvy: leaf.mv.1 as i16,
                size: s as u8,
            },
        );
        Ok(())
    }
}

impl Decoder for VaultDecoder {
    type Frame = VideoFrame;

    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<VideoFrame>> {
        let limits = self.cfg.limits;
        limits.check_packet(packet.data.len())?;
        limits.check_reference_frames(self.enc.max_refs)?;
        limits.check_recursion_depth(MAX_PARTITION_DEPTH + 1)?;
        let frame_bytes = self.enc.frame_bytes();
        limits.check_frame_bytes(frame_bytes)?;
        limits.check_allocation(frame_bytes * (usize::from(self.enc.max_refs) + 2))?;

        if packet.data.len() < CRC_TRAILER_BYTES {
            return Err(CodecError::MalformedPacket);
        }
        let (body, checksum) = packet.data.split_at(packet.data.len() - CRC_TRAILER_BYTES);
        let mut c4 = [0u8; 4];
        c4.copy_from_slice(checksum);
        if crc32(body) != u32::from_le_bytes(c4) {
            return Err(CodecError::MalformedPacket);
        }

        let mut r = BitReader::new(body);
        let bad = |_| CodecError::MalformedPacket;
        let version = r.read(16).map_err(bad)? as u16;
        if version != VERSION {
            return Err(CodecError::Unsupported("unsupported Vault packet version"));
        }
        let intra = r.read(1).map_err(bad)? == 1;
        let qp = r.read(QP_BITS).map_err(bad)? as u8;
        if !(MIN_QP..=MAX_QP).contains(&qp) {
            return Err(CodecError::MalformedPacket);
        }
        let num_refs = r.read(5).map_err(bad)? as usize;
        let lf = r.read(1).map_err(bad)? == 1;
        if intra != (num_refs == 0) {
            return Err(CodecError::MalformedPacket);
        }
        // A frame may not claim more references than the codec supports,
        // and the limits knob is honoured against what the frame claims.
        if num_refs > usize::from(crate::MAX_REFERENCE_FRAMES) {
            return Err(CodecError::MalformedPacket);
        }
        limits.check_reference_frames(num_refs as u8)?;
        if num_refs > usize::from(self.enc.max_refs) {
            return Err(CodecError::MalformedPacket);
        }
        // A P-frame whose references we do not hold (lost packets, or a
        // stream joined mid-way) is undecodable, not damaged.
        if num_refs > self.refs.len() {
            return Err(CodecError::NeedMoreInput);
        }

        let (w, h) = (self.enc.width as usize, self.enc.height as usize);
        let mut fd = FrameDec {
            refs: &self.refs[..num_refs],
            scans: &self.scans,
            xf: &mut self.xf,
            recon: Picture::new(w, h, self.enc.format, 0),
            field: Field::new(w, h),
            step: quant_step(qp),
            ctx: LeafCtx {
                intra_frame: intra,
                ref_bits: ref_index_bits(num_refs),
                nplanes: if self.enc.format == tpt_media_core::PixelFormat::Yuv420 {
                    3
                } else {
                    1
                },
            },
            num_refs,
            stats: FrameStats::default(),
        };
        for sy in (0..h).step_by(SUPERBLOCK) {
            for sx in (0..w).step_by(SUPERBLOCK) {
                fd.block(&mut r, sx, sy, SUPERBLOCK, 0)?;
            }
        }
        let stats = fd.stats;
        let mut recon = fd.recon;
        let field = fd.field;
        if lf {
            loop_filter(&mut recon, &field, qp);
        }

        self.stats = stats;
        let frame = picture_to_frame(&recon, packet.pts);
        if intra {
            self.refs.clear();
        }
        self.refs.insert(0, recon);
        self.refs.truncate(usize::from(self.enc.max_refs));
        Ok(alloc::vec![frame])
    }

    fn flush(&mut self) -> CodecResult<Vec<VideoFrame>> {
        self.refs.clear();
        Ok(Vec::new())
    }
}

// --- type-erased factory ----------------------------------------------------

/// Type-erased decoder factory. Carries the geometry (see
/// [`crate::VaultEncoderFactory`]).
pub struct VaultDecoderFactory {
    enc: VaultConfig,
}

impl VaultDecoderFactory {
    /// Build a factory for a geometry and configuration.
    ///
    /// # Errors
    /// [`CodecError::InvalidConfig`] if `enc` fails validation.
    pub fn new(enc: VaultConfig) -> CodecResult<Self> {
        enc.validate()?;
        Ok(VaultDecoderFactory { enc })
    }
}

impl DynDecoderFactory for VaultDecoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(
            self.enc.width,
            self.enc.height,
            self.enc.format,
            Capabilities {
                lossy: true,
                ..Capabilities::default()
            },
        )
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<Box<dyn DynDecoder>> {
        let mut merged = self.enc.clone();
        if cfg.extra.len() >= VaultConfig::CODEC_PRIVATE_LEN {
            merged = VaultConfig::from_codec_private(self.enc.width, self.enc.height, &cfg.extra)?;
        }
        let d = VaultDecoder::new(cfg.clone(), merged)?;
        Ok(Box::new(VaultDynDecoder { inner: d }))
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

/// Type-erased wrapper around [`VaultDecoder`].
pub struct VaultDynDecoder {
    inner: VaultDecoder,
}

impl DynDecoder for VaultDynDecoder {
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
