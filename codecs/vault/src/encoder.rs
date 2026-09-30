//! The Vault encoder: recursive-partition rate-distortion optimisation.
//!
//! ## What the RDO actually does
//!
//! For each 64x64 superblock the encoder walks the quadtree top-down and
//! decides, at every node, between **coding the node as one leaf** and
//! **splitting it into four children**, by comparing
//!
//! ```text
//! J = D + lambda * R
//! ```
//!
//! - `D` is the sum of squared errors between the source and the
//!   *reconstruction the decoder will produce* (luma + chroma), so the
//!   quantiser's real behaviour is inside the objective;
//! - `R` is the exact number of bits the real syntax writer emits for the
//!   candidate (it is priced by running the writer against a bit counter);
//! - `lambda = LAMBDA_SCALE * step^2`, an empirical constant, not derived.
//!
//! The leaf itself is the minimum-`J` of a small **candidate set**: skip,
//! the best inter candidate, and the best intra candidate. That set is
//! chosen by cheaper proxies (SAD plus a vector-cost term for motion,
//! SAD for the intra mode), and the partition recursion is pruned by an
//! early-termination rule for nodes whose best leaf codes no residual and
//! already fits the source closely.
//!
//! **None of this finds the global minimum of `J`.** It is a greedy
//! per-node comparison over a heuristically pruned candidate set:
//! candidate generation is not exhaustive, the motion search is a pattern
//! search, quantisation is not rate-distortion optimised per coefficient,
//! decisions ignore their effect on later blocks' predictors and
//! neighbours, and `lambda` is not tuned. The claim is only that each
//! decision was the cheapest *among the alternatives that were priced*.

use alloc::boxed::Box;
use alloc::vec::Vec;

use tpt_media_bitstream::rice::WriteLike;
use tpt_media_bitstream::WriteError;
use tpt_media_codec::{
    Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, DynEncoder, DynEncoderFactory,
    Encoder,
};
use tpt_media_core::{Frame, Packet, PacketFlags, Timebase, Timestamp, VideoFrame};

use crate::bits::{rice_len_signed, BitCounter, PacketBits};
use crate::filter::{loop_filter, Field, Unit};
use crate::picture::{picture_from_frame, picture_to_frame, Picture, Plane};
use crate::pred::{intra_mode_available, intra_predict, mc_block, sad, ssd, IntraMode};
use crate::syntax::{
    build_pred, quantise_leaf, reconstruct_leaf, ref_index_bits, write_leaf, Blk, Leaf, LeafCtx,
    LeafKind, MVD_K,
};
use crate::transform::{Scans, Xform};
use crate::{
    crc32, quant_step, static_info, VaultConfig, CODEC_ID, CRC_TRAILER_BYTES, KEYFRAME_INTERVAL,
    MAX_PARTITION_DEPTH, MIN_BLOCK, MV_LIMIT, QP_BITS, SUPERBLOCK, VERSION,
};

/// `lambda = LAMBDA_SCALE * step^2`. An empirical starting point of the
/// same order as the constants used in the video-coding literature; it is
/// *not* tuned for this codec.
pub const LAMBDA_SCALE: f64 = 0.15;

/// Weight of the vector-cost term in the SAD-based motion search, per unit
/// of quantiser step (`lambda_sad = LAMBDA_SAD_SCALE * step`, which is
/// `sqrt(lambda)` for `lambda = 0.16 * step^2`, without needing a `sqrt`).
const LAMBDA_SAD_SCALE: f32 = 0.4;

/// Early termination: a node whose best leaf codes *no* coefficient and
/// whose distortion is at most this many `step^2` per sample is not split.
const EARLY_TERMINATE_MSE: f64 = 0.10;

/// Rotating neighbourhood of the pattern search (8-connected).
const NEIGHBOURS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// One syntax element of a superblock, in coding order.
enum Sym {
    Split(bool),
    Leaf(Box<Leaf>),
}

/// A fully evaluated leaf candidate.
struct Cand {
    leaf: Leaf,
    recon: Blk,
    unit: Unit,
    d: u64,
    j: f64,
}

/// Vault encoder.
///
/// Holds the reconstructed reference pictures (the *loop-filtered* ones,
/// most recent first, at most [`VaultConfig::max_refs`]) so encoder and
/// decoder stay in lock-step. The first frame is always an I-frame; after
/// that one is emitted every `keyframe_interval` frames (an I-frame clears
/// the reference list) and the rest are P-frames.
pub struct VaultEncoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: VaultConfig,
    refs: Vec<Picture>,
    scans: Scans,
    frames: u64,
    keyframe_interval: u64,
}

impl VaultEncoder {
    /// Build an encoder with the default I-frame interval.
    ///
    /// # Errors
    /// [`CodecError::InvalidConfig`] if `enc` fails validation.
    pub fn new(codec_cfg: CodecConfig, enc: VaultConfig) -> CodecResult<Self> {
        VaultEncoder::with_keyframe_interval(codec_cfg, enc, KEYFRAME_INTERVAL)
    }

    /// Build an encoder that emits an I-frame every `keyframe_interval`
    /// frames; `0` forces every frame intra.
    ///
    /// # Errors
    /// [`CodecError::InvalidConfig`] if `enc` fails validation.
    pub fn with_keyframe_interval(
        codec_cfg: CodecConfig,
        enc: VaultConfig,
        keyframe_interval: u64,
    ) -> CodecResult<Self> {
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
        Ok(VaultEncoder {
            info,
            cfg: codec_cfg,
            enc,
            refs: Vec::new(),
            scans: Scans::new(),
            frames: 0,
            keyframe_interval,
        })
    }

    /// Whether the next frame will be coded as an I-frame.
    pub fn next_is_keyframe(&self) -> bool {
        self.keyframe_interval == 0 || self.frames % self.keyframe_interval == 0
    }

    /// Number of reference pictures currently held.
    pub fn reference_count(&self) -> usize {
        self.refs.len()
    }

    /// The most recent reconstruction (exactly what a decoder outputs for
    /// the last packet), or `None` before the first frame.
    pub fn reconstruction(&self) -> Option<VideoFrame> {
        self.refs
            .first()
            .map(|p| picture_to_frame(p, Timestamp::zero(Timebase::ONE_PER_SECOND)))
    }
}

impl Encoder for VaultEncoder {
    type Frame = VideoFrame;

    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn encode(&mut self, frame: &VideoFrame) -> CodecResult<Vec<Packet>> {
        if frame.format != self.enc.format {
            return Err(CodecError::Unsupported("vault pixel format"));
        }
        if frame.width != self.enc.width || frame.height != self.enc.height {
            return Err(CodecError::InvalidConfig(
                "vault frame dimensions do not match the configured geometry",
            ));
        }
        let frame_bytes = self.enc.frame_bytes();
        self.cfg.limits.check_frame_bytes(frame_bytes)?;
        self.cfg
            .limits
            .check_allocation(frame_bytes * (usize::from(self.enc.max_refs) + 2))?;
        let src = picture_from_frame(frame)?;

        let intra = self.next_is_keyframe() || self.refs.is_empty();
        if intra {
            self.refs.clear();
        }
        let num_refs = if intra {
            0
        } else {
            self.refs.len().min(usize::from(self.enc.max_refs))
        };

        let step = quant_step(self.enc.qp);
        let mut fe = FrameEnc {
            src: &src,
            refs: &self.refs[..num_refs],
            scans: &self.scans,
            xf: Xform::new(),
            recon: Picture::new(src.width, src.height, src.format, 0),
            field: Field::new(src.width, src.height),
            syms: Vec::new(),
            step,
            lambda: LAMBDA_SCALE * f64::from(step) * f64::from(step),
            lambda_sad: LAMBDA_SAD_SCALE * step,
            max_block: self.enc.max_block_size,
            search_range: i32::from(self.enc.search_range),
            ctx: LeafCtx {
                intra_frame: intra,
                ref_bits: ref_index_bits(num_refs),
                nplanes: src.nplanes(),
            },
            scratch: Vec::new(),
        };

        let mut w = PacketBits::new();
        let werr = |_: WriteError| CodecError::Internal;
        w.write_bits(16, u64::from(VERSION)).map_err(werr)?;
        w.write_bits(1, u64::from(intra)).map_err(werr)?;
        w.write_bits(QP_BITS, u64::from(self.enc.qp))
            .map_err(werr)?;
        w.write_bits(5, num_refs as u64).map_err(werr)?;
        w.write_bits(1, u64::from(self.enc.loop_filter))
            .map_err(werr)?;

        for sy in (0..src.height).step_by(SUPERBLOCK) {
            for sx in (0..src.width).step_by(SUPERBLOCK) {
                fe.rdo_block(sx, sy, SUPERBLOCK, 0);
                for sym in fe.syms.drain(..) {
                    match sym {
                        Sym::Split(b) => w.write_bits(1, u64::from(b)).map_err(werr)?,
                        Sym::Leaf(leaf) => {
                            write_leaf(&mut w, &leaf, &fe.ctx, fe.scans).map_err(werr)?
                        }
                    }
                }
            }
        }

        let mut recon = fe.recon;
        let field = fe.field;
        if self.enc.loop_filter {
            loop_filter(&mut recon, &field, self.enc.qp);
        }

        // The proof of the packet-size bound is `VaultConfig::max_packet_bits`;
        // this is the runtime check that the encoder honours it.
        if w.bits() > self.enc.max_packet_bits() as u64 {
            return Err(CodecError::Internal);
        }
        let mut data = w.finish();
        let crc = crc32(&data);
        data.extend_from_slice(&crc.to_le_bytes());
        debug_assert!(data.len() <= self.enc.max_packet_bits().div_ceil(8) + CRC_TRAILER_BYTES);

        self.refs.insert(0, recon);
        self.refs.truncate(usize::from(self.enc.max_refs));
        self.frames += 1;

        let flags = PacketFlags::END_OF_FRAME
            | if intra {
                PacketFlags::KEYFRAME
            } else {
                PacketFlags::empty()
            };
        Ok(alloc::vec![Packet {
            data,
            pts: frame.frame.pts,
            dts: frame.frame.pts,
            flags,
            track_id: 0,
            duration: None,
        }])
    }
}

/// Per-frame encoder state. Borrows the source and the references so the
/// recursion can mutate its own scratch freely.
struct FrameEnc<'a> {
    src: &'a Picture,
    refs: &'a [Picture],
    scans: &'a Scans,
    xf: Xform,
    recon: Picture,
    field: Field,
    syms: Vec<Sym>,
    step: f32,
    lambda: f64,
    lambda_sad: f32,
    max_block: usize,
    search_range: i32,
    ctx: LeafCtx,
    /// Motion-compensation scratch (largest block: 64x64).
    scratch: Vec<u8>,
}

/// Cost of one integer-pel or half-pel vector candidate.
struct Searcher<'b> {
    plane: &'b Plane,
    src: &'b [u8],
    x: usize,
    y: usize,
    s: usize,
    pmv: (i32, i32),
    lam: f32,
    buf: &'b mut [u8],
}

impl Searcher<'_> {
    fn cost(&mut self, mv: (i32, i32)) -> f32 {
        mc_block(
            self.plane, self.x, self.y, self.s, self.s, mv.0, mv.1, self.buf,
        );
        let bits =
            rice_len_signed(mv.0 - self.pmv.0, MVD_K) + rice_len_signed(mv.1 - self.pmv.1, MVD_K);
        sad(self.src, &self.buf[..self.s * self.s]) as f32 + self.lam * bits as f32
    }

    /// Full-pel pattern search then half-pel refinement. Returns the best
    /// half-pel vector and its cost. Not exhaustive: it can settle in a
    /// local minimum.
    fn run(&mut self, range: i32) -> ((i32, i32), f32) {
        let clampr = |v: i32| v.clamp(-range, range);
        let mut best_f = (0i32, 0i32);
        let mut best_c = self.cost((0, 0));
        let pf = (clampr((self.pmv.0 + 1) >> 1), clampr((self.pmv.1 + 1) >> 1));
        if pf != (0, 0) {
            let c = self.cost((pf.0 * 2, pf.1 * 2));
            if c < best_c {
                best_c = c;
                best_f = pf;
            }
        }
        let mut step = 1i32;
        while step * 2 <= range {
            step *= 2;
        }
        if range > 0 {
            while step >= 1 {
                let mut iters = 0;
                loop {
                    let centre = best_f;
                    let mut moved = false;
                    for (dx, dy) in NEIGHBOURS {
                        let cand = (centre.0 + dx * step, centre.1 + dy * step);
                        if cand.0.abs() > range || cand.1.abs() > range {
                            continue;
                        }
                        let c = self.cost((cand.0 * 2, cand.1 * 2));
                        if c < best_c {
                            best_c = c;
                            best_f = cand;
                            moved = true;
                        }
                    }
                    iters += 1;
                    if !moved || iters >= 32 {
                        break;
                    }
                }
                step /= 2;
            }
        }
        let centre = (best_f.0 * 2, best_f.1 * 2);
        let mut best = centre;
        for (dx, dy) in NEIGHBOURS {
            let cand = (centre.0 + dx, centre.1 + dy);
            if cand.0.abs() > MV_LIMIT || cand.1.abs() > MV_LIMIT {
                continue;
            }
            let c = self.cost(cand);
            if c < best_c {
                best_c = c;
                best = cand;
            }
        }
        (best, best_c)
    }
}

impl FrameEnc<'_> {
    /// Price a leaf: quantise, reconstruct, measure `D` and `R`.
    fn make_cand(&mut self, mut leaf: Leaf, src: &Blk, pred: Blk, flag_bits: u64) -> Cand {
        let np = self.ctx.nplanes;
        let recon = if leaf.kind == LeafKind::Skip {
            pred
        } else {
            leaf.coefs = quantise_leaf(&mut self.xf, src, &pred, leaf.size, np, self.step);
            reconstruct_leaf(&mut self.xf, &pred, &leaf.coefs, leaf.size, np, self.step)
        };
        let d: u64 = (0..np).map(|pl| ssd(&src.p[pl], &recon.p[pl])).sum();
        let mut counter = BitCounter::default();
        // A BitCounter cannot fail.
        let _ = write_leaf(&mut counter, &leaf, &self.ctx, self.scans);
        let bits = counter.bits + flag_bits;
        let unit = Unit {
            intra: leaf.kind == LeafKind::Intra,
            inter: leaf.kind != LeafKind::Intra,
            coded: leaf.coded(),
            ref_idx: leaf.ref_idx,
            mvx: leaf.mv.0 as i16,
            mvy: leaf.mv.1 as i16,
            size: leaf.size as u8,
        };
        Cand {
            leaf,
            recon,
            unit,
            d,
            j: d as f64 + self.lambda * bits as f64,
        }
    }

    fn base_leaf(size: usize, kind: LeafKind) -> Leaf {
        Leaf {
            size,
            kind,
            ref_idx: 0,
            mv: (0, 0),
            mvd: (0, 0),
            mode: IntraMode::Dc,
            coefs: Vec::new(),
        }
    }

    /// Evaluate the leaf candidates of the `s x s` block at `(x, y)` and
    /// return the cheapest by `J`. Ties keep the earlier of skip < inter <
    /// intra.
    fn eval_leaf(&mut self, x: usize, y: usize, s: usize, flag_bits: u64) -> Cand {
        let src = Blk::load(self.src, x, y, s);
        let pmv = self.field.mv_pred(x / MIN_BLOCK, y / MIN_BLOCK);

        // Intra first: it exists in every frame, so it seeds `best`. The
        // later candidates replace it on `<=`, so ties resolve
        // skip > inter > intra.
        let mut mode = IntraMode::Dc;
        let mut mode_sad = u32::MAX;
        let mut tmp = alloc::vec![0u8; s * s];
        for m in IntraMode::ALL {
            if !intra_mode_available(m, x, y) {
                continue;
            }
            intra_predict(&self.recon.planes[0], x, y, s, m, &mut tmp);
            let c = sad(&src.p[0], &tmp);
            if c < mode_sad {
                mode_sad = c;
                mode = m;
            }
        }
        let mut leaf = Self::base_leaf(s, LeafKind::Intra);
        leaf.mode = mode;
        let pred = build_pred(
            &self.recon,
            self.refs,
            LeafKind::Intra,
            0,
            (0, 0),
            mode,
            x,
            y,
            s,
        );
        let mut best = self.make_cand(leaf, &src, pred, flag_bits);
        if self.ctx.intra_frame {
            return best;
        }

        // Inter: best (reference, vector) by SAD + vector cost.
        if self.scratch.len() < s * s {
            self.scratch.resize(s * s, 0);
        }
        let mut pick: Option<(u8, (i32, i32), f32)> = None;
        let refs = self.refs;
        for (r, rp) in refs.iter().enumerate() {
            let ref_cost = self.lambda_sad * ref_index_bits(refs.len()) as f32;
            let (mv, cost) = {
                let mut se = Searcher {
                    plane: &rp.planes[0],
                    src: &src.p[0],
                    x,
                    y,
                    s,
                    pmv,
                    lam: self.lambda_sad,
                    buf: &mut self.scratch,
                };
                se.run(self.search_range)
            };
            let cost = cost + ref_cost;
            if pick.is_none_or(|p| cost < p.2) {
                pick = Some((r as u8, mv, cost));
            }
        }
        if let Some((r, mv, _)) = pick {
            let mut leaf = Self::base_leaf(s, LeafKind::Inter);
            leaf.ref_idx = r;
            leaf.mv = mv;
            leaf.mvd = (mv.0 - pmv.0, mv.1 - pmv.1);
            let pred = build_pred(
                &self.recon,
                self.refs,
                LeafKind::Inter,
                r,
                mv,
                IntraMode::Dc,
                x,
                y,
                s,
            );
            let c = self.make_cand(leaf, &src, pred, flag_bits);
            if c.j <= best.j {
                best = c;
            }
        }

        // Skip: predicted vector, reference 0, no residual.
        let mut leaf = Self::base_leaf(s, LeafKind::Skip);
        leaf.mv = pmv;
        let pred = build_pred(
            &self.recon,
            self.refs,
            LeafKind::Skip,
            0,
            pmv,
            IntraMode::Dc,
            x,
            y,
            s,
        );
        let c = self.make_cand(leaf, &src, pred, flag_bits);
        if c.j <= best.j {
            best = c;
        }
        best
    }

    /// Commit a leaf candidate's reconstruction and unit info.
    fn commit(&mut self, x: usize, y: usize, c: &Cand) {
        c.recon.store(&mut self.recon, x, y, c.leaf.size);
        self.field.set_block(x, y, c.leaf.size, c.unit);
    }

    /// Decide the `s x s` node at `(x, y)`: leaf or split, by `J`. Pushes
    /// the winning syntax onto `self.syms`, leaves the winning
    /// reconstruction in `self.recon` and the unit info in `self.field`,
    /// and returns the winning `J`.
    fn rdo_block(&mut self, x: usize, y: usize, s: usize, depth: u8) -> f64 {
        debug_assert!(depth <= MAX_PARTITION_DEPTH);
        let inside = x + s <= self.src.width && y + s <= self.src.height;
        let flag_present = s > MIN_BLOCK && inside;
        let flag_bits = u64::from(flag_present);
        let can_leaf = inside && s <= self.max_block;
        let can_split = s > MIN_BLOCK;
        let mark = self.syms.len();

        let leaf = if can_leaf {
            Some(self.eval_leaf(x, y, s, flag_bits))
        } else {
            None
        };

        // Luma plus (4:2:0) two quarter-size chroma planes.
        let samples = if self.ctx.nplanes == 3 {
            (s * s * 3 / 2) as f64
        } else {
            (s * s) as f64
        };
        let try_split = can_split
            && match &leaf {
                None => true,
                Some(c) => {
                    let flat = !c.leaf.coded()
                        && (c.d as f64)
                            <= EARLY_TERMINATE_MSE
                                * f64::from(self.step)
                                * f64::from(self.step)
                                * samples;
                    !flat
                }
            };

        if !try_split {
            // `try_split` is true whenever there is no leaf candidate and the
            // node can split; a node that cannot split is always a leaf
            // (`can_leaf` holds for every 8x8 node, which is always inside).
            if let Some(c) = leaf {
                self.commit(x, y, &c);
                if flag_present {
                    self.syms.push(Sym::Split(false));
                }
                let j = c.j;
                self.syms.push(Sym::Leaf(Box::new(c.leaf)));
                return j;
            }
            return 0.0;
        }

        if flag_present {
            self.syms.push(Sym::Split(true));
        }
        let mut split_j = if flag_present { self.lambda } else { 0.0 };
        let h = s / 2;
        for (dx, dy) in [(0, 0), (h, 0), (0, h), (h, h)] {
            let (cx, cy) = (x + dx, y + dy);
            if cx >= self.src.width || cy >= self.src.height {
                continue;
            }
            split_j += self.rdo_block(cx, cy, h, depth + 1);
        }

        if let Some(c) = leaf {
            if c.j <= split_j {
                self.syms.truncate(mark);
                self.commit(x, y, &c);
                if flag_present {
                    self.syms.push(Sym::Split(false));
                }
                let j = c.j;
                self.syms.push(Sym::Leaf(Box::new(c.leaf)));
                return j;
            }
        }
        split_j
    }
}

// --- type-erased factory ----------------------------------------------------

/// Type-erased encoder factory. Carries the geometry because geometry is
/// part of a video codec's configuration and `CodecConfig` only carries the
/// track's private bytes.
pub struct VaultEncoderFactory {
    enc: VaultConfig,
    keyframe_interval: u64,
}

impl VaultEncoderFactory {
    /// Build a factory for a geometry and configuration, using the default
    /// I-frame interval.
    ///
    /// # Errors
    /// [`CodecError::InvalidConfig`] if `enc` fails validation.
    pub fn new(enc: VaultConfig) -> CodecResult<Self> {
        VaultEncoderFactory::with_keyframe_interval(enc, KEYFRAME_INTERVAL)
    }

    /// Build a factory with an explicit I-frame interval in frames.
    ///
    /// # Errors
    /// [`CodecError::InvalidConfig`] if `enc` fails validation.
    pub fn with_keyframe_interval(enc: VaultConfig, keyframe_interval: u64) -> CodecResult<Self> {
        enc.validate()?;
        Ok(VaultEncoderFactory {
            enc,
            keyframe_interval,
        })
    }
}

impl DynEncoderFactory for VaultEncoderFactory {
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
    fn build(&self, cfg: &CodecConfig) -> CodecResult<Box<dyn DynEncoder>> {
        let mut merged = self.enc.clone();
        if cfg.extra.len() >= VaultConfig::CODEC_PRIVATE_LEN {
            merged = VaultConfig::from_codec_private(self.enc.width, self.enc.height, &cfg.extra)?;
            merged.max_block_size = self.enc.max_block_size;
        }
        let e = VaultEncoder::with_keyframe_interval(cfg.clone(), merged, self.keyframe_interval)?;
        Ok(Box::new(VaultDynEncoder { inner: e }))
    }
}

/// Type-erased wrapper around [`VaultEncoder`].
pub struct VaultDynEncoder {
    inner: VaultEncoder,
}

impl DynEncoder for VaultDynEncoder {
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
                    pts: Timestamp::zero(Timebase::ONE_PER_SECOND),
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
                return Err(CodecError::Unsupported("vault does not code audio"))
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
