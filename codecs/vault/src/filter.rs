//! The per-8x8-unit side information the decoder reconstructs as it goes,
//! and the loop filter that consumes it.
//!
//! Both are needed identically by the encoder and decoder, so both live
//! here and are called from both: nothing in this module can make the two
//! sides disagree, because there is only one implementation.

use alloc::vec;
use alloc::vec::Vec;

use crate::picture::Picture;
use crate::{MAX_TRANSFORM, MIN_BLOCK};

/// What the loop filter and the motion-vector predictor need to know about
/// one 8x8 luma unit.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct Unit {
    /// Intra-coded block.
    pub intra: bool,
    /// Inter-coded (or skipped) block: `mvx/mvy/ref_idx` are meaningful.
    pub inter: bool,
    /// The block carries at least one non-zero coefficient.
    pub coded: bool,
    pub ref_idx: u8,
    /// Half-pel motion vector.
    pub mvx: i16,
    pub mvy: i16,
    /// Edge length in luma samples of the leaf block containing this unit.
    pub size: u8,
}

/// The unit grid of one frame.
pub(crate) struct Field {
    pub uw: usize,
    pub uh: usize,
    pub units: Vec<Unit>,
}

impl Field {
    pub fn new(width: usize, height: usize) -> Self {
        let (uw, uh) = (width / MIN_BLOCK, height / MIN_BLOCK);
        Field {
            uw,
            uh,
            units: vec![Unit::default(); uw * uh],
        }
    }

    /// Stamp `u` over the `s x s` block at luma position `(x, y)`.
    pub fn set_block(&mut self, x: usize, y: usize, s: usize, u: Unit) {
        let (ux0, uy0) = (x / MIN_BLOCK, y / MIN_BLOCK);
        let n = s / MIN_BLOCK;
        for uy in uy0..(uy0 + n).min(self.uh) {
            for ux in ux0..(ux0 + n).min(self.uw) {
                self.units[uy * self.uw + ux] = u;
            }
        }
    }

    /// Motion-vector predictor for the block whose top-left unit is
    /// `(ux, uy)`: the left neighbour's vector if it is inter, else the
    /// top neighbour's, else zero. Left and top are always earlier in
    /// z-order, so both sides have them final when they ask.
    pub fn mv_pred(&self, ux: usize, uy: usize) -> (i32, i32) {
        if ux > 0 {
            let l = &self.units[uy * self.uw + ux - 1];
            if l.inter {
                return (i32::from(l.mvx), i32::from(l.mvy));
            }
        }
        if uy > 0 {
            let t = &self.units[(uy - 1) * self.uw + ux];
            if t.inter {
                return (i32::from(t.mvx), i32::from(t.mvy));
            }
        }
        (0, 0)
    }
}

/// Boundary strength between two adjacent units: 2 if either is intra,
/// 1 if either carries residual or the motion differs by a full pel or
/// more (or the references differ), else 0 (nothing to smooth).
fn strength(p: &Unit, q: &Unit) -> u8 {
    let motion_differs = p.ref_idx != q.ref_idx
        || (i32::from(p.mvx) - i32::from(q.mvx)).abs() >= 2
        || (i32::from(p.mvy) - i32::from(q.mvy)).abs() >= 2;
    if p.intra || q.intra {
        2
    } else if p.coded || q.coded || motion_differs {
        1
    } else {
        0
    }
}

/// Deblocking strength `S` for a quantiser index and boundary strength.
///
/// Grows with the quantiser step (a coarse quantiser makes bigger block
/// steps), is at least 1, and saturates at 12, the largest value of the
/// H.263 Annex J table. An intra edge uses the full `S`, a weaker inter
/// edge two thirds of it.
fn strength_s(qp: u8, bs: u8) -> i32 {
    let step = crate::quant_step(qp);
    let s = ((step * 0.2 + 0.5) as i32).clamp(1, 12);
    if bs >= 2 {
        s
    } else {
        ((s * 2) / 3).max(1)
    }
}

/// The Annex J "up-down ramp": passes `x` unchanged up to `s`, then ramps
/// back to zero by `2 * s`, so a genuine large edge is left alone while a
/// small blocking step is corrected.
#[inline]
fn up_down_ramp(x: i32, s: i32) -> i32 {
    let a = x.abs();
    let keep = (a - (2 * (a - s)).max(0)).max(0);
    if x < 0 {
        -keep
    } else {
        keep
    }
}

/// Filter one 4-sample window `A B | C D` across an edge, after the
/// H.263 Annex J deblocking filter (a public ITU-T recommendation):
///
/// ```text
/// d  = (A - 4B + 4C - D) / 8
/// d1 = up_down_ramp(d, S)
/// B' = B + d1     C' = C - d1
/// d2 = clamp((A - D) / 4, +/- |d1| / 2)
/// A' = A - d2     D' = D + d2
/// ```
///
/// On a linear gradient `d` is nearly zero, so ramps are not flattened; on
/// a blocking step it moves the two inner samples together.
fn filter_window(s: &mut [u8; 4], strength: i32) {
    let (a, b, c, d) = (
        i32::from(s[0]),
        i32::from(s[1]),
        i32::from(s[2]),
        i32::from(s[3]),
    );
    let delta = (a - 4 * b + 4 * c - d) / 8;
    let d1 = up_down_ramp(delta, strength);
    let lim = d1.abs() / 2;
    let d2 = ((a - d) / 4).clamp(-lim, lim);
    s[0] = (a - d2).clamp(0, 255) as u8;
    s[1] = (b + d1).clamp(0, 255) as u8;
    s[2] = (c - d1).clamp(0, 255) as u8;
    s[3] = (d + d2).clamp(0, 255) as u8;
}

/// Run the loop filter over the whole reconstructed picture, in place.
///
/// Edges are only those that are real discontinuities of the coding
/// structure: leaf-block boundaries, plus (luma only) the internal 32x32
/// transform-tile seams of a 64x64 block. Vertical edges first, then
/// horizontal, both in raster order. Chroma edges sit on leaf-block
/// boundaries (a chroma block is never split into tiles: `64/2 = 32` is
/// the largest transform).
pub(crate) fn loop_filter(pic: &mut Picture, field: &Field, qp: u8) {
    for pl in 0..pic.nplanes() {
        let g = if pl == 0 { MIN_BLOCK } else { MIN_BLOCK / 2 };
        let w = pic.planes[pl].w;
        let is_edge = |pos: usize, size: usize| -> bool {
            // `pos` is in luma samples.
            if pl == 0 {
                pos % size.min(MAX_TRANSFORM) == 0
            } else {
                pos % size == 0
            }
        };
        // Vertical edges.
        for uy in 0..field.uh {
            for ux in 1..field.uw {
                let q = &field.units[uy * field.uw + ux];
                let p = &field.units[uy * field.uw + ux - 1];
                if !is_edge(ux * MIN_BLOCK, usize::from(q.size).max(MIN_BLOCK)) {
                    continue;
                }
                let bs = strength(p, q);
                if bs == 0 {
                    continue;
                }
                let strength_val = strength_s(qp, bs);
                let x = ux * g;
                let plane = &mut pic.planes[pl];
                for r in 0..g {
                    let i = (uy * g + r) * w + x;
                    let mut s = [
                        plane.data[i - 2],
                        plane.data[i - 1],
                        plane.data[i],
                        plane.data[i + 1],
                    ];
                    filter_window(&mut s, strength_val);
                    plane.data[i - 2] = s[0];
                    plane.data[i - 1] = s[1];
                    plane.data[i] = s[2];
                    plane.data[i + 1] = s[3];
                }
            }
        }
        // Horizontal edges.
        for uy in 1..field.uh {
            for ux in 0..field.uw {
                let q = &field.units[uy * field.uw + ux];
                let p = &field.units[(uy - 1) * field.uw + ux];
                if !is_edge(uy * MIN_BLOCK, usize::from(q.size).max(MIN_BLOCK)) {
                    continue;
                }
                let bs = strength(p, q);
                if bs == 0 {
                    continue;
                }
                let strength_val = strength_s(qp, bs);
                let y = uy * g;
                let plane = &mut pic.planes[pl];
                for c in 0..g {
                    let x = ux * g + c;
                    let i = y * w + x;
                    let mut s = [
                        plane.data[i - 2 * w],
                        plane.data[i - w],
                        plane.data[i],
                        plane.data[i + w],
                    ];
                    filter_window(&mut s, strength_val);
                    plane.data[i - 2 * w] = s[0];
                    plane.data[i - w] = s[1];
                    plane.data[i] = s[2];
                    plane.data[i + w] = s[3];
                }
            }
        }
    }
}
