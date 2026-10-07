//! Leaf-block syntax (write and read) and the prediction/reconstruction
//! steps both sides share.
//!
//! A *leaf* is one undivided block: its prediction (skip / inter / intra)
//! plus its coefficients. The partition tree above it is just split flags
//! (see `encoder.rs` / `decoder.rs`).
//!
//! Wire layout of a leaf, in order:
//!
//! ```text
//! P-frame only:  1 bit skip; if not skip, 1 bit intra
//! skip:          (nothing: predicted vector, reference 0, no residual)
//! inter:         ref index (ceil(log2(num_refs)) bits, none if 1 ref)
//!                mvd.x, mvd.y   signed Rice, k = MVD_K (vector minus predictor)
//!                coefficients
//! intra:         2 bits mode, then coefficients
//! coefficients:  luma tiles, then U tiles, then V tiles (4:2:0 only);
//!                each tile is a `write_coefs` block
//! ```

use alloc::vec;
use alloc::vec::Vec;

use tpt_media_bitstream::rice::{decode_riced, encode_riced, WriteLike};
use tpt_media_bitstream::{BitReader, WriteError};
use tpt_media_codec::{CodecError, CodecResult};

use crate::bits::{rice_len_signed, rice_len_unsigned, RICE_K_MAX};
use crate::picture::Picture;
use crate::pred::{chroma_mv, intra_predict, mc_block, IntraMode};
use crate::transform::{read_coefs, tile_size, write_coefs, Scans, Xform};
use crate::MV_LIMIT;

/// Rice parameter for motion-vector differences.
pub(crate) const MVD_K: u32 = 1;

/// How a leaf is predicted.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LeafKind {
    /// Inter, predicted vector, reference 0, no residual.
    Skip,
    Inter,
    Intra,
    /// Bi-predictive (B frames only): the weighted average of the
    /// motion-compensated past and future references (the frame's
    /// header weight). No intra mode — a B leaf that cannot predict
    /// well codes a single-sided residual instead.
    Bi,
    /// Forward-only (B frames only): predicted from the past reference
    /// (`refs[1]`) alone.
    Fwd,
    /// Backward-only (B frames only): predicted from the future
    /// reference (`refs[0]`) alone.
    Bwd,
}

/// One decoded (or about-to-be-encoded) leaf block.
#[derive(Clone, Debug)]
pub(crate) struct Leaf {
    /// Edge length in luma samples.
    pub size: usize,
    pub kind: LeafKind,
    pub ref_idx: u8,
    /// Absolute half-pel vector (inter/skip).
    pub mv: (i32, i32),
    /// Vector minus predictor, as coded (inter only).
    pub mvd: (i32, i32),
    /// Future-reference vector (Bi leaves; absolute half-pel).
    pub mv_b: (i32, i32),
    pub mode: IntraMode,
    /// `Y` (`size^2`), then `U`, `V` (`(size/2)^2` each, 4:2:0): tiles in
    /// raster order, each tile's levels in raster order.
    pub coefs: Vec<i32>,
}

impl Leaf {
    /// Whether any coefficient is non-zero.
    pub fn coded(&self) -> bool {
        self.coefs.iter().any(|&c| c != 0)
    }
}

/// Everything the leaf syntax depends on besides the leaf itself.
#[derive(Copy, Clone)]
pub(crate) struct LeafCtx {
    pub intra_frame: bool,
    /// B frame: non-skip leaves carry two motion vectors against the
    /// fixed reference pair `refs[1]` (past) / `refs[0]` (future), then
    /// coefficient tiles like any other non-skip leaf.
    pub b_frame: bool,
    /// Weight of the *past* reference in the B-frame bi-prediction, in
    /// quarters (`1..=3`; `2` is the plain average). The future gets the
    /// complement. Signalled once per B frame in the v3 header; ignored
    /// outside B frames.
    pub b_weight_past: u8,
    /// Bits of the reference index field.
    pub ref_bits: u32,
    pub nplanes: usize,
}

/// Bits needed for a reference index among `num_refs` references.
pub(crate) fn ref_index_bits(num_refs: usize) -> u32 {
    if num_refs <= 1 {
        0
    } else {
        usize::BITS - (num_refs - 1).leading_zeros()
    }
}

/// Edge of plane `pl`'s block for a luma leaf of edge `size`.
#[inline]
pub(crate) fn plane_edge(size: usize, pl: usize) -> usize {
    if pl == 0 {
        size
    } else {
        size / 2
    }
}

/// Total coefficient count of a leaf of edge `size`.
pub(crate) fn coef_count(size: usize, nplanes: usize) -> usize {
    (0..nplanes)
        .map(|pl| plane_edge(size, pl) * plane_edge(size, pl))
        .sum()
}

/// Write one leaf. The same function prices candidates (against a
/// `BitCounter`) and emits the packet (against a `PacketBits`).
pub(crate) fn write_leaf<W: WriteLike>(
    w: &mut W,
    leaf: &Leaf,
    ctx: &LeafCtx,
    scans: &Scans,
) -> Result<(), WriteError> {
    if ctx.b_frame {
        // B leaves: the skip bit is the zero-vector reference-0 (future)
        // prediction; otherwise a 2-bit combine mode — 0 = weighted bi
        // (both vectors, the frame's header weight), 1 = forward-only
        // (past vector), 2 = backward-only (future vector); 3 is
        // reserved and never written — then the mode's motion vectors
        // against a (0, 0) predictor, then the coefficient tiles exactly
        // like any other non-skip leaf. No intra modes, no reference
        // indices: the reference pair is fixed for the whole frame.
        let skip = leaf.kind == LeafKind::Skip;
        w.write_bits(1, u64::from(skip))?;
        if skip {
            return Ok(());
        }
        let mode = match leaf.kind {
            LeafKind::Bi => 0u64,
            LeafKind::Fwd => 1,
            LeafKind::Bwd => 2,
            _ => unreachable!("B leaves are skip, bi, forward- or backward-only"),
        };
        w.write_bits(2, mode)?;
        match leaf.kind {
            LeafKind::Bi => {
                encode_riced(w, MVD_K, i64::from(leaf.mv.0))?;
                encode_riced(w, MVD_K, i64::from(leaf.mv.1))?;
                encode_riced(w, MVD_K, i64::from(leaf.mv_b.0))?;
                encode_riced(w, MVD_K, i64::from(leaf.mv_b.1))?;
            }
            LeafKind::Fwd => {
                encode_riced(w, MVD_K, i64::from(leaf.mv.0))?;
                encode_riced(w, MVD_K, i64::from(leaf.mv.1))?;
            }
            LeafKind::Bwd => {
                encode_riced(w, MVD_K, i64::from(leaf.mv_b.0))?;
                encode_riced(w, MVD_K, i64::from(leaf.mv_b.1))?;
            }
            _ => unreachable!("B leaves are skip, bi, forward- or backward-only"),
        }
    } else {
        if !ctx.intra_frame {
            let skip = leaf.kind == LeafKind::Skip;
            w.write_bits(1, u64::from(skip))?;
            if skip {
                return Ok(());
            }
            w.write_bits(1, u64::from(leaf.kind == LeafKind::Intra))?;
        }
        match leaf.kind {
            LeafKind::Inter => {
                if ctx.ref_bits > 0 {
                    w.write_bits(ctx.ref_bits, u64::from(leaf.ref_idx))?;
                }
                encode_riced(w, MVD_K, i64::from(leaf.mvd.0))?;
                encode_riced(w, MVD_K, i64::from(leaf.mvd.1))?;
            }
            LeafKind::Intra => w.write_bits(2, leaf.mode as u64)?,
            LeafKind::Skip => {}
            LeafKind::Bi | LeafKind::Fwd | LeafKind::Bwd => {
                unreachable!("B leaves only exist in B frames")
            }
        }
    }
    let mut off = 0usize;
    for pl in 0..ctx.nplanes {
        let m = plane_edge(leaf.size, pl);
        let t = tile_size(m);
        for _ in 0..(m / t) * (m / t) {
            write_coefs(w, &leaf.coefs[off..off + t * t], t, scans.get(t))?;
            off += t * t;
        }
    }
    Ok(())
}

/// Read one leaf of edge `size`, given the vector predictor and how many
/// references this frame declared.
pub(crate) fn read_leaf(
    r: &mut BitReader,
    size: usize,
    ctx: &LeafCtx,
    num_refs: usize,
    pred_mv: (i32, i32),
    scans: &Scans,
) -> CodecResult<Leaf> {
    let bad = |_| CodecError::MalformedPacket;
    let mut leaf = Leaf {
        size,
        kind: LeafKind::Skip,
        ref_idx: 0,
        mv: pred_mv,
        mvd: (0, 0),
        mv_b: (0, 0),
        mode: IntraMode::Dc,
        coefs: Vec::new(),
    };
    if ctx.b_frame {
        // B leaves: skip bit, then the 2-bit combine mode with its
        // motion vectors (absolute, against a (0, 0) predictor), then
        // the coefficient tiles like any other non-skip leaf.
        if r.read(1).map_err(bad)? == 1 {
            // A B skip decodes as the zero-vector reference-0 (future)
            // prediction — the exact prediction the encoder prices.
            leaf.mv = (0, 0);
            leaf.mv_b = (0, 0);
            return Ok(leaf);
        }
        // Absolute vectors, bounded exactly like the I/P ones (the same
        // `MV_LIMIT` the search clamps to); anything wider is hostile.
        let lim = i64::from(MV_LIMIT);
        let read_v = |r: &mut BitReader| -> Result<(i32, i32), CodecError> {
            let dx = decode_riced(r, MVD_K).map_err(bad)?;
            let dy = decode_riced(r, MVD_K).map_err(bad)?;
            if dx.abs() > lim || dy.abs() > lim {
                return Err(CodecError::MalformedPacket);
            }
            Ok((dx as i32, dy as i32))
        };
        match r.read(2).map_err(bad)? {
            0 => {
                leaf.kind = LeafKind::Bi;
                leaf.mv = read_v(r)?;
                leaf.mv_b = read_v(r)?;
            }
            1 => {
                leaf.kind = LeafKind::Fwd;
                leaf.mv = read_v(r)?;
            }
            2 => {
                leaf.kind = LeafKind::Bwd;
                leaf.mv_b = read_v(r)?;
            }
            _ => return Err(CodecError::MalformedPacket),
        }
    } else {
        let (skip, intra) = if ctx.intra_frame {
            (false, true)
        } else if r.read(1).map_err(bad)? == 1 {
            (true, false)
        } else {
            (false, r.read(1).map_err(bad)? == 1)
        };
        if skip {
            return Ok(leaf);
        }
        if intra {
            leaf.kind = LeafKind::Intra;
            leaf.mode = IntraMode::from_bits(r.read(2).map_err(bad)? as u8);
        } else {
            leaf.kind = LeafKind::Inter;
            if ctx.ref_bits > 0 {
                let idx = r.read(ctx.ref_bits).map_err(bad)? as usize;
                if idx >= num_refs {
                    return Err(CodecError::MalformedPacket);
                }
                leaf.ref_idx = idx as u8;
            }
            let dx = decode_riced(r, MVD_K).map_err(bad)?;
            let dy = decode_riced(r, MVD_K).map_err(bad)?;
            // Anything that could overflow or exceed the legal vector range is
            // hostile; reject before doing arithmetic on it.
            let lim = i64::from(2 * MV_LIMIT);
            if dx.abs() > lim || dy.abs() > lim {
                return Err(CodecError::MalformedPacket);
            }
            let mv = (pred_mv.0 + dx as i32, pred_mv.1 + dy as i32);
            if mv.0.abs() > MV_LIMIT || mv.1.abs() > MV_LIMIT {
                return Err(CodecError::MalformedPacket);
            }
            leaf.mvd = (dx as i32, dy as i32);
            leaf.mv = mv;
        }
    }
    leaf.coefs = vec![0i32; coef_count(size, ctx.nplanes)];
    let mut off = 0usize;
    for pl in 0..ctx.nplanes {
        let m = plane_edge(size, pl);
        let t = tile_size(m);
        for _ in 0..(m / t) * (m / t) {
            read_coefs(r, &mut leaf.coefs[off..off + t * t], t, scans.get(t))?;
            off += t * t;
        }
    }
    Ok(leaf)
}

/// Per-plane dense blocks of one leaf: `[Y (s*s), U, V ((s/2)^2)]`.
#[derive(Clone, Debug)]
pub(crate) struct Blk {
    pub p: [Vec<u8>; 3],
}

impl Blk {
    pub fn new(size: usize, nplanes: usize) -> Self {
        let mk = |pl: usize| {
            if pl < nplanes {
                let m = plane_edge(size, pl);
                vec![0u8; m * m]
            } else {
                Vec::new()
            }
        };
        Blk {
            p: [mk(0), mk(1), mk(2)],
        }
    }

    /// Load the source block at luma `(x, y)` of edge `size`.
    pub fn load(pic: &Picture, x: usize, y: usize, size: usize) -> Self {
        let mut b = Blk::new(size, pic.nplanes());
        for pl in 0..pic.nplanes() {
            let m = plane_edge(size, pl);
            let (px, py) = if pl == 0 { (x, y) } else { (x / 2, y / 2) };
            pic.planes[pl].load_block(px, py, m, m, &mut b.p[pl]);
        }
        b
    }

    /// Store into `pic` at luma `(x, y)`.
    pub fn store(&self, pic: &mut Picture, x: usize, y: usize, size: usize) {
        for pl in 0..pic.nplanes() {
            let m = plane_edge(size, pl);
            let (px, py) = if pl == 0 { (x, y) } else { (x / 2, y / 2) };
            pic.planes[pl].store_block(px, py, m, m, &self.p[pl]);
        }
    }
}

/// Build the prediction of a leaf. `recon` is the picture being
/// reconstructed (intra reads its causal neighbours); `refs[ref_idx]` is the
/// reference for inter/skip. The caller has validated `ref_idx < refs.len()`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_pred(
    recon: &Picture,
    refs: &[Picture],
    kind: LeafKind,
    ref_idx: u8,
    mv: (i32, i32),
    mv_b: (i32, i32),
    mode: IntraMode,
    b_weight_past: u8,
    x: usize,
    y: usize,
    size: usize,
) -> Blk {
    let np = recon.nplanes();
    let mut out = Blk::new(size, np);
    for pl in 0..np {
        let m = plane_edge(size, pl);
        let (px, py) = if pl == 0 { (x, y) } else { (x / 2, y / 2) };
        match kind {
            LeafKind::Intra => intra_predict(&recon.planes[pl], px, py, m, mode, &mut out.p[pl]),
            // Single-sided B leaves are inter prediction from one fixed
            // side of the pair: `Fwd` from the past (`refs[1]`), `Bwd`
            // from the future (`refs[0]`).
            LeafKind::Fwd | LeafKind::Bwd => {
                let (mx, my) = if pl == 0 {
                    if kind == LeafKind::Fwd { mv } else { mv_b }
                } else if kind == LeafKind::Fwd {
                    (chroma_mv(mv.0), chroma_mv(mv.1))
                } else {
                    (chroma_mv(mv_b.0), chroma_mv(mv_b.1))
                };
                let r = if kind == LeafKind::Fwd { 1usize } else { 0usize };
                mc_block(
                    &refs[r].planes[pl],
                    px,
                    py,
                    m,
                    m,
                    mx,
                    my,
                    &mut out.p[pl],
                );
            }
            LeafKind::Skip | LeafKind::Inter => {
                let (mx, my) = if pl == 0 {
                    mv
                } else {
                    (chroma_mv(mv.0), chroma_mv(mv.1))
                };
                mc_block(
                    &refs[usize::from(ref_idx)].planes[pl],
                    px,
                    py,
                    m,
                    m,
                    mx,
                    my,
                    &mut out.p[pl],
                );
            }
            LeafKind::Bi => {
                // B-frame leaves reference exactly two pictures, stored
                // most-recent-first: `refs[0]` is the *future* reference,
                // `refs[1]` the past one. `mv` is the past vector,
                // `mv_b` the future one; the prediction is their plain
                // average (the only v0.2 combine mode).
                let (px0, py0) = if pl == 0 {
                    mv
                } else {
                    (chroma_mv(mv.0), chroma_mv(mv.1))
                };
                let (px1, py1) = if pl == 0 {
                    mv_b
                } else {
                    (chroma_mv(mv_b.0), chroma_mv(mv_b.1))
                };
                let mut a = alloc::vec![0u8; m * m];
                let mut b = alloc::vec![0u8; m * m];
                mc_block(&refs[1].planes[pl], px, py, m, m, px0, py0, &mut a);
                mc_block(&refs[0].planes[pl], px, py, m, m, px1, py1, &mut b);
                // Weighted combine, quarters of the past reference
                // (`b_weight_past` 1..=3; 2 is the plain average),
                // rounded to nearest. The weight is the frame's own
                // header field, so both sides weight identically by
                // construction.
                let w = u16::from(b_weight_past.clamp(1, 3));
                let wf = 4 - w;
                for (slot, pair) in out.p[pl].iter_mut().zip(a.iter().zip(&b)) {
                    *slot = ((w * u16::from(*pair.0) + wf * u16::from(*pair.1) + 2) >> 2) as u8;
                }
            }
        }
    }
    out
}

/// Rate-distortion optimised quantisation over one tile's levels: for
/// every non-zero level, in scan order from the block's last down,
/// decide between keeping it and zeroing it by comparing what the
/// Rice-coded syntax *actually saves* against the distortion zeroing
/// would add.
///
/// By Parseval on the orthonormal coefficient scaling, zeroing a level
/// adds distortion `(level·step)²` to the pixel-domain SSE the leaf RDO
/// measures, which against `J = D + λ·R` (`λ = LAMBDA_SCALE·step²`)
/// makes the quantiser step cancel: a level is dropped when its rate
/// saving in bits exceeds `level² / LAMBDA_SCALE`. The saving is exact
/// for the block's *current* shape — the level's own code, the run it
/// merges away, and, when the coefficient is the block's last, every
/// code that disappears with the shortened span. The Rice parameter the
/// model uses is the block's cheapest at sweep start; `write_coefs`
/// re-derives the real `k` at write time, so this pass is a heuristic
/// on top of an exact writer, never a contract.
///
/// Single backward sweep, O(tile²) worst case: zeroing the current last
/// shortens the coded span (possibly exposing a new last, which the
/// sweep then visits), and zeroing an interior level merges its two
/// flanking runs.
pub(crate) fn rdoq_tile(levels: &mut [i32], scan: &[u16], step: f32, lambda_scale: f64) {
    let t2 = scan.len();
    let mut last = match (0..t2).rev().find(|&p| levels[scan[p] as usize] != 0) {
        Some(p) => p,
        None => return,
    };
    if lambda_scale <= 0.0 {
        return;
    }
    // The block's cheapest Rice parameter over its current shape — the
    // same criterion `write_coefs` applies at write time.
    let mut best_k = 0u32;
    let mut best_cost = u64::MAX;
    for k in 0..=RICE_K_MAX {
        let mut cost = 0u64;
        let mut run = 0u32;
        for p in 0..=last {
            let v = levels[scan[p] as usize];
            if v == 0 {
                run += 1;
            } else {
                cost += u64::from(rice_len_unsigned(run, k) + rice_len_signed(v, k));
                run = 0;
            }
        }
        if cost < best_cost {
            best_cost = cost;
            best_k = k;
        }
    }

    let mut p = last;
    loop {
        let r = scan[p] as usize;
        let level = levels[r];
        let is_last = p == last;
        if level != 0 {
            // Previous non-zero position (or "before the block").
            let prev = (0..p).rev().find(|&q| levels[scan[q] as usize] != 0);
            let run_before = match prev {
                Some(q) => (p - q) as u32,
                None => (p + 1) as u32,
            };
            let saved = if is_last {
                // The whole last symbol disappears with the span.
                u64::from(rice_len_signed(level, best_k) + rice_len_unsigned(run_before, best_k))
            } else {
                // Interior: the level goes and its flanking runs merge.
                let next = (p + 1..=last).find(|&q| levels[scan[q] as usize] != 0);
                let run_after = match next {
                    Some(q) => (q - p) as u32,
                    None => 0,
                };
                let merged = run_before + 1 + run_after;
                u64::from(rice_len_signed(level, best_k) + rice_len_unsigned(run_before, best_k))
                    + u64::from(rice_len_unsigned(run_after, best_k))
                    - u64::from(rice_len_unsigned(merged, best_k))
            };
            let distortion = f64::from(level) * f64::from(step);
            if lambda_scale * saved as f64 > distortion * distortion {
                levels[r] = 0;
                if is_last {
                    // Shorten the coded span; the sweep visits whatever
                    // last it exposes.
                    match (0..p).rev().find(|&q| levels[scan[q] as usize] != 0) {
                        Some(q) => last = q,
                        None => {
                            // The block just went all-zero: its `coded`
                            // flag drops at write time.
                            levels[..t2].fill(0);
                            return;
                        }
                    }
                }
            }
        }
        if p == 0 {
            break;
        }
        p -= 1;
    }
}

/// RDOQ over a whole leaf: every plane, every tile.
pub(crate) fn rdoq_leaf(
    coefs: &mut [i32],
    size: usize,
    nplanes: usize,
    step: f32,
    lambda_scale: f64,
    scans: &Scans,
) {
    // The sweep is O(tile²) per tile, and on the measured corpus it only
    // ever fires at the sizes where cheap levels live; 32×32 tiles are
    // skipped both because their sweep dominates the cost (the high-motion
    // scene paid +8 ms/frame for zero decisions) and because a 32×32
    // transform's energy concentrates where zeroing never pays.
    const MAX_RDOQ_TILE: usize = 16;
    let mut off = 0usize;
    for pl in 0..nplanes {
        let m = plane_edge(size, pl);
        let t = tile_size(m);
        let scan = scans.get(t);
        let pass = t <= MAX_RDOQ_TILE;
        for _ in 0..(m / t) * (m / t) {
            if pass {
                rdoq_tile(&mut coefs[off..off + t * t], scan, step, lambda_scale);
            }
            off += t * t;
        }
    }
}

/// Quantise the residual `src - pred` of every plane and tile; returns the
/// leaf's coefficient vector.
pub(crate) fn quantise_leaf(
    xf: &mut Xform,
    src: &Blk,
    pred: &Blk,
    size: usize,
    nplanes: usize,
    step: f32,
) -> Vec<i32> {
    let mut coefs = vec![0i32; coef_count(size, nplanes)];
    let mut off = 0usize;
    for pl in 0..nplanes {
        let m = plane_edge(size, pl);
        let t = tile_size(m);
        for ty in 0..m / t {
            for tx in 0..m / t {
                xf.quantise_tile(
                    &src.p[pl],
                    &pred.p[pl],
                    m,
                    tx * t,
                    ty * t,
                    t,
                    step,
                    &mut coefs[off..off + t * t],
                );
                off += t * t;
            }
        }
    }
    coefs
}

/// Reconstruct a leaf: `pred + IDCT(dequant(coefs))`, clamped. Shared by
/// the encoder (which then knows exactly what the decoder will produce)
/// and the decoder.
pub(crate) fn reconstruct_leaf(
    xf: &mut Xform,
    pred: &Blk,
    coefs: &[i32],
    size: usize,
    nplanes: usize,
    step: f32,
) -> Blk {
    let mut out = pred.clone();
    if coefs.is_empty() {
        return out;
    }
    let mut off = 0usize;
    for pl in 0..nplanes {
        let m = plane_edge(size, pl);
        let t = tile_size(m);
        for ty in 0..m / t {
            for tx in 0..m / t {
                xf.reconstruct_tile(
                    &coefs[off..off + t * t],
                    &pred.p[pl],
                    m,
                    tx * t,
                    ty * t,
                    t,
                    step,
                    &mut out.p[pl],
                );
                off += t * t;
            }
        }
    }
    out
}
