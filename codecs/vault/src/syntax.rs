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
    let (skip, intra) = if ctx.intra_frame {
        (false, true)
    } else if r.read(1).map_err(bad)? == 1 {
        (true, false)
    } else {
        (false, r.read(1).map_err(bad)? == 1)
    };
    let mut leaf = Leaf {
        size,
        kind: LeafKind::Skip,
        ref_idx: 0,
        mv: pred_mv,
        mvd: (0, 0),
        mode: IntraMode::Dc,
        coefs: Vec::new(),
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
    mode: IntraMode,
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
        }
    }
    out
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
